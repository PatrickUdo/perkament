//! Regressietests voor de bevindingen uit het review van `render`.
use lopdf::{dictionary, Document, Object, Stream};
use perkament::render::{self, Kind, Response};
use std::time::{Duration, Instant};

/// Document met per pagina de gegeven inhoud en MediaBox.
fn build(pages: &[(&[u8], [f32; 4])]) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = vec![];
    for (content, mb) in pages {
        let c = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let p = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c, "Resources" => dictionary! {},
            "MediaBox" => vec![mb[0].into(), mb[1].into(), mb[2].into(), mb[3].into()],
        });
        kids.push(Object::Reference(p));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    out
}

const A4: [f32; 4] = [0.0, 0.0, 595.0, 842.0];
/// Streeppatronen waarop de renderbibliotheek eindeloos doorloopt.
const HANGS: &[u8] = b"[0 0] 0 d 0 0 m 100 100 l S [-5 3] -9999999 d 0 0 m 100 100 l S";

fn wait_for(r: &render::Renderer, limit: Duration, mut want: impl FnMut(&Response) -> bool) -> Option<Response> {
    let start = Instant::now();
    while start.elapsed() < limit {
        match r.try_recv() {
            Some(resp) if want(&resp) => return Some(resp),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    None
}

#[test]
fn stuck_page_is_abandoned_and_the_rest_keeps_working() {
    let bytes = build(&[(HANGS, A4), (b"0 0 100 100 re f", A4)]);
    let mut r = render::spawn(|| {});
    r.load(1, bytes);
    assert!(wait_for(&r, Duration::from_secs(5), |x| matches!(x, Response::Loaded { .. })).is_some());
    r.request(1, 0, 1.0, Kind::Main);

    // De bewaker meldt de vastgelopen pagina zodra de tijd om is.
    let start = Instant::now();
    let mut stuck = None;
    while stuck.is_none() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
        stuck = r.check_stuck(Duration::from_millis(400));
        if let Some(Response::Image { page: 0, .. }) = r.try_recv() {
            panic!("pagina 0 liep niet vast; deze test heeft dan een andere vastloper nodig");
        }
    }
    assert_eq!(stuck, Some((1, 0, Kind::Main)));

    // Daarna werkt de renderer weer, en de zieke pagina wordt niet opnieuw geprobeerd.
    r.request(1, 1, 1.0, Kind::Main);
    assert!(wait_for(&r, Duration::from_secs(10), |x| matches!(x, Response::Image { page: 1, .. })).is_some(), "renderer herstelt niet");
    r.request(1, 0, 1.0, Kind::Main);
    assert!(wait_for(&r, Duration::from_secs(2), |x| matches!(x, Response::Failed { page: 0, .. })).is_some());
    assert_eq!(r.check_stuck(Duration::from_millis(400)), None);
}

#[test]
fn export_with_timeout_does_not_block_forever() {
    let bytes = build(&[(HANGS, A4)]);
    let start = Instant::now();
    assert!(render::render_png_with_timeout(bytes, 0, 1.0, Duration::from_millis(500)).is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn degenerate_and_huge_pages_do_not_crash() {
    let cases: &[[f32; 4]] = &[
        [0.0, 0.0, 0.2, 800.0],       // bijna geen breedte
        [0.0, 0.0, 800.0, 0.3],       // bijna geen hoogte
        [0.0, 0.0, 400000.0, 300000.0], // enorm
        [0.0, 0.0, 2.0, 2.0],         // piepklein
    ];
    for mb in cases {
        let bytes = build(&[(b"0 0 1 1 re f", *mb)]);
        let geoms = render::page_geometries(bytes.clone()).unwrap();
        assert!(geoms[0].w >= 1.0 && geoms[0].h >= 1.0 && geoms[0].w.is_finite(), "{geoms:?}");
        for scale in [0.001, 1.0, 50.0] {
            // Mag slagen of netjes mislukken, maar nooit crashen of een lege afbeelding geven.
            if let Ok(png) = render::render_png(bytes.clone(), 0, scale) {
                assert!(png.len() > 50);
            }
        }
        let mut r = render::spawn(|| {});
        r.load(1, bytes);
        r.request(1, 0, 3.0, Kind::Thumb);
        let resp = wait_for(&r, Duration::from_secs(20), |x| !matches!(x, Response::Loaded { .. })).expect("geen antwoord");
        if let Response::Image { size, rgba, .. } = resp {
            assert!(size[0] >= 1 && size[1] >= 1 && size[0] <= 6001 && size[1] <= 6001, "{size:?}");
            assert_eq!(rgba.len(), size[0] * size[1] * 4);
        }
    }
}

#[test]
fn newest_main_request_is_served_first_and_stale_ones_are_dropped() {
    let pages: Vec<(&[u8], [f32; 4])> = (0..12).map(|_| (b"0 0 100 100 re f".as_slice(), A4)).collect();
    let mut r = render::spawn(|| {});
    r.load(1, build(&pages));
    assert!(wait_for(&r, Duration::from_secs(5), |x| matches!(x, Response::Loaded { .. })).is_some());
    for page in 0..12 {
        r.request(1, page, 3.0, Kind::Main);
    }
    // Alleen de laatste twee zijn nog in beeld.
    r.keep_main(&[10, 11]);
    let mut seen = vec![];
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) && !(seen.contains(&10) && seen.contains(&11)) {
        if let Some(Response::Image { page, .. }) = r.try_recv() {
            seen.push(page);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(seen.contains(&10) && seen.contains(&11), "{seen:?}");
    // Hooguit een pagina die al bezig was mag er nog tussendoor komen.
    assert!(seen.len() <= 4, "verlaten verzoeken zijn toch gerenderd: {seen:?}");
}
