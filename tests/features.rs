//! Tests voor de Excel-export en het plaatsen van afbeeldingen.
use lopdf::{dictionary, Document, Object, Stream};
use perkament::{excel, pdfops, render};
use std::io::Cursor;

fn build(content: &[u8], rotate: i64) -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let c = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => c, "Rotate" => rotate,
        "MediaBox" => vec![0.into(), 0.into(), 400.into(), 600.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    pdfops::load(&out).unwrap()
}

const TABLE: &[u8] = b"BT /F1 14 Tf 40 560 Td (Offerte machines voor de klant in Zwolle) Tj ET \
BT /F1 10 Tf 40 520 Td (Artikel) Tj 160 0 Td (Aantal) Tj 120 0 Td (Prijs) Tj ET \
BT /F1 10 Tf 40 500 Td (Machine A) Tj 172 0 Td (2) Tj 96 0 Td (1.250,00) Tj ET \
BT /F1 10 Tf 40 480 Td (Onderdeel) Tj 167 0 Td (10) Tj 113 0 Td (35,50) Tj ET";

#[test]
fn pdf_table_becomes_spreadsheet_rows_and_columns() {
    let doc = build(TABLE, 0);
    let table = excel::page_table(&doc, 0);
    assert_eq!(table.len(), 4, "{table:?}");
    assert_eq!(table[0][0], "Offerte machines voor de klant in Zwolle");
    assert_eq!(table[1], vec!["Artikel", "Aantal", "Prijs"]);
    assert_eq!(table[2], vec!["Machine A", "2", "1.250,00"]);
    assert_eq!(table[3], vec!["Onderdeel", "10", "35,50"]);

    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export.xlsx");
    let cells = excel::export_xlsx(&doc, &path).unwrap();
    assert_eq!(cells, 10);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"PK"), "geen geldig xlsx-bestand");
    assert!(bytes.len() > 1000);
}

#[test]
fn rotated_page_exports_in_reading_order() {
    // Dezelfde tabel op een pagina die een kwartslag gedraaid wordt getoond.
    let doc = build(TABLE, 90);
    assert_eq!(excel::page_table(&doc, 0), excel::page_table(&build(TABLE, 0), 0));

    // En tekst die zelf een kwartslag gedraaid op de pagina staat (liggend gezet).
    let sideways: &[u8] = b"BT /F1 10 Tf 0 1 -1 0 300 40 Tm (Artikel) Tj 160 0 Td (Aantal) Tj ET \
BT /F1 10 Tf 0 1 -1 0 320 40 Tm (Machine A) Tj 160 0 Td (2) Tj ET";
    let table = excel::page_table(&build(sideways, 0), 0);
    assert_eq!(table, vec![vec!["Artikel", "Aantal"], vec!["Machine A", "2"]], "{table:?}");
}

#[test]
fn export_without_text_is_an_error() {
    let doc = build(b"0 0 100 100 re f", 0);
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("leeg.xlsx");
    assert!(excel::export_xlsx(&doc, &path).is_err());
    assert!(!path.exists(), "er is toch een leeg bestand geschreven");
}

/// Een PNG van `w` bij `h` pixels, half doorzichtig als `alpha` waar is.
fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 200, if alpha && x < w / 2 { 80 } else { 255 }])
    });
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn rect_of(doc: &Document, id: lopdf::ObjectId) -> Vec<f32> {
    doc.get_dictionary(id).unwrap().get(b"Rect").unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect()
}

#[test]
fn image_keeps_its_proportions_and_stays_upright_on_any_rotation() {
    for rotate in [0i64, 90, 180, 270] {
        let mut doc = build(b"0 g", rotate);
        let g = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap()[0];
        // Sleep in de weergave een vlak van 200 x 200 vanaf (50, 60); de afbeelding is 2:1.
        let a = g.view_to_pdf(50.0, 60.0);
        let b = g.view_to_pdf(250.0, 260.0);
        let rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
        let before = render::render_png(pdfops::to_bytes(&mut doc).unwrap(), 0, 1.0).unwrap();
        let id = pdfops::add_image(&mut doc, 0, rect, &png(200, 100, false)).unwrap();
        let r = rect_of(&doc, id);
        let (p, q) = (g.pdf_to_view(r[0], r[1]), g.pdf_to_view(r[2], r[3]));
        let (left, top, w, h) = (p.0.min(q.0), p.1.min(q.1), (p.0 - q.0).abs(), (p.1 - q.1).abs());
        assert!((left - 50.0).abs() < 0.5 && (top - 60.0).abs() < 0.5, "rotatie {rotate}: hoek ({left},{top})");
        assert!((w - 200.0).abs() < 0.5 && (h - 100.0).abs() < 0.5, "rotatie {rotate}: {w} x {h}");
        let after = render::render_png(pdfops::to_bytes(&mut doc).unwrap(), 0, 1.0).unwrap();
        assert_ne!(before, after, "rotatie {rotate}: de afbeelding is niet zichtbaar");
        if rotate == 90 {
            std::fs::write(std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("image-rot90.png"), &after).unwrap();
        }
    }
}

#[test]
fn clicked_image_gets_a_default_size_inside_the_page() {
    let mut doc = build(b"0 g", 0);
    // Klik vlak bij de rechteronderhoek.
    let id = pdfops::add_image(&mut doc, 0, [390.0, 10.0, 390.0, 10.0], &png(300, 300, true)).unwrap();
    let r = rect_of(&doc, id);
    assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 400.0 && r[3] <= 600.0, "{r:?}");
    assert!((r[2] - r[0] - 200.0).abs() < 0.5 && (r[3] - r[1] - 200.0).abs() < 0.5, "{r:?}");

    // Doorzichtigheid blijft behouden, en de afbeelding staat in de lijst en is te verwijderen.
    let saved = pdfops::to_bytes(&mut doc).unwrap();
    assert!(saved.windows(6).any(|w| w == b"/SMask"));
    let listed = pdfops::list_annotations(&doc);
    assert_eq!(listed.len(), 1);
    pdfops::delete_annotation(&mut doc, 0, listed[0].id).unwrap();
    assert!(pdfops::list_annotations(&doc).is_empty());
    let reloaded = pdfops::load(&pdfops::to_bytes_for_saving(&doc).unwrap()).unwrap();
    assert!(!pdfops::to_bytes(&mut reloaded.clone()).unwrap().windows(6).any(|w| w == b"/SMask"), "verwijderde afbeelding staat nog in het bestand");
}

#[test]
fn unreadable_image_is_refused_without_changing_the_document() {
    let mut doc = build(b"0 g", 0);
    let before = doc.objects.len();
    assert!(pdfops::add_image(&mut doc, 0, [10.0, 10.0, 100.0, 100.0], b"dit is geen afbeelding").is_err());
    assert_eq!(doc.objects.len(), before);
}

#[test]
fn huge_image_is_scaled_down() {
    let mut doc = build(b"0 g", 0);
    pdfops::add_image(&mut doc, 0, [10.0, 10.0, 300.0, 300.0], &png(5000, 50, false)).unwrap();
    let widths: Vec<i64> = doc.objects.values().filter_map(|o| match o {
        Object::Stream(s) if s.dict.get(b"Subtype").ok().and_then(|n| n.as_name().ok()) == Some(b"Image".as_slice()) => s.dict.get(b"Width").ok().and_then(|w| w.as_i64().ok()),
        _ => None,
    }).collect();
    assert_eq!(widths, vec![2400]);
}
