use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use perkament::pdfops::{self, AnnotKind, NewAnnot};
use perkament::render;

fn sample(pages: usize, label: &str) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let res = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids = vec![];
    for i in 0..pages {
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 36.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal(format!("{label} pagina {}", i + 1))]),
            Operation::new("ET", vec![]),
            Operation::new("rg", vec![0.2.into(), 0.4.into(), 0.8.into()]),
            Operation::new("re", vec![72.into(), 500.into(), 200.into(), 100.into()]),
            Operation::new("f", vec![]),
        ];
        let c = doc.add_object(Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap()));
        let p = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => c });
        kids.push(Object::Reference(p));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Kids" => kids, "Count" => pages as i64,
        "Resources" => res, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn end_to_end() {
    let out = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let a = sample(3, "Alfa");
    std::fs::write(out.join("sample.pdf"), &a).unwrap();
    let mut doc = pdfops::load(&a).unwrap();
    assert_eq!(pdfops::page_count(&doc), 3);

    assert_eq!(pdfops::insert_pdf(&mut doc, &sample(2, "Beta"), 1).unwrap(), 2);
    assert_eq!(pdfops::page_count(&doc), 5);
    let texts: Vec<String> = (0..5).map(|i| pdfops::page_text(&doc, i)).collect();
    assert!(texts[0].contains("Alfa pagina 1"), "{texts:?}");
    assert!(texts[1].contains("Beta pagina 1"), "{texts:?}");
    assert!(texts[3].contains("Alfa pagina 2"), "{texts:?}");
    let hits = pdfops::search(&texts, "beta");
    assert_eq!(hits.iter().map(|h| h.page).collect::<Vec<_>>(), vec![1, 2]);

    pdfops::move_page(&mut doc, 4, 0).unwrap();
    assert!(pdfops::page_text(&doc, 0).contains("Alfa pagina 3"));
    pdfops::delete_pages(&mut doc, &[2]).unwrap();
    assert_eq!(pdfops::page_count(&doc), 4);
    pdfops::rotate_page(&mut doc, 1, 90).unwrap();
    pdfops::insert_blank_page(&mut doc, 4).unwrap();
    assert_eq!(pdfops::page_count(&doc), 5);

    let mk = |kind, rect, text: &str| NewAnnot { kind, rect, color: [1.0, 0.85, 0.1], text: text.into(), points: vec![] };
    pdfops::add_annotation(&mut doc, 0, &mk(AnnotKind::Highlight, [70.0, 690.0, 420.0, 735.0], "")).unwrap();
    pdfops::add_annotation(&mut doc, 0, &mk(AnnotKind::Rectangle, [60.0, 490.0, 290.0, 610.0], "")).unwrap();
    pdfops::add_annotation(&mut doc, 0, &mk(AnnotKind::Note, [400.0, 580.0, 400.0, 600.0], "Controleer dit \u{2014} één €")).unwrap();
    pdfops::add_annotation(&mut doc, 0, &mk(AnnotKind::FreeText, [300.0, 380.0, 520.0, 400.0],
        "Dit is een tekstvak met genoeg woorden om over meerdere regels te lopen: café, naïef, €5.")).unwrap();
    let mut ink = mk(AnnotKind::Ink, [0.0; 4], "");
    ink.color = [0.9, 0.1, 0.1];
    ink.points = (0..60).map(|i| (80.0 + i as f32 * 5.0, 300.0 + (i as f32 * 0.4).sin() * 30.0)).collect();
    let ink_id = pdfops::add_annotation(&mut doc, 0, &ink).unwrap();
    let annots = pdfops::list_annotations(&doc);
    assert_eq!(annots.len(), 5);
    assert_eq!(annots[2].contents, "Controleer dit \u{2014} één €");

    pdfops::add_watermark(&mut doc, &Default::default()).unwrap();
    pdfops::add_page_numbers(&mut doc, "Pagina {n} van {N}").unwrap();
    let mut meta = pdfops::get_meta(&doc);
    meta.title = "Tëst".into();
    meta.author = "Perkament".into();
    pdfops::set_meta(&mut doc, &meta).unwrap();

    let saved = pdfops::to_bytes_for_saving(&doc).unwrap();
    std::fs::write(out.join("edited.pdf"), &saved).unwrap();
    let mut again = pdfops::load(&saved).unwrap();
    assert_eq!(pdfops::page_count(&again), 5);
    assert_eq!(pdfops::get_meta(&again).title, "Tëst");
    assert_eq!(pdfops::list_annotations(&again).len(), 5);
    let _ = ink_id;
    let ann = pdfops::list_annotations(&again)[4].clone();
    pdfops::delete_annotation(&mut again, 0, ann.id).unwrap();
    assert_eq!(pdfops::list_annotations(&again).len(), 4);

    let part = pdfops::extract_pages(&doc, &[0, 1]).unwrap();
    assert_eq!(pdfops::page_count(&pdfops::load(&part).unwrap()), 2);

    let geoms = render::page_geometries(saved.clone()).unwrap();
    assert_eq!(geoms.len(), 5);
    assert!((geoms[0].w - 595.0).abs() < 1.0 && (geoms[1].w - 842.0).abs() < 1.0, "{geoms:?}");
    let (x, y) = geoms[1].view_to_pdf(10.0, 20.0);
    let (vx, vy) = geoms[1].pdf_to_view(x, y);
    assert!((vx - 10.0).abs() < 0.01 && (vy - 20.0).abs() < 0.01);
    for i in 0..2 {
        let png = render::render_png(saved.clone(), i, 1.0).unwrap();
        std::fs::write(out.join(format!("page{i}.png")), png).unwrap();
    }
}

#[test]
fn edit_existing_text() {
    use perkament::textedit::{self, Replaced};
    let out = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let mut doc = pdfops::load(&sample(2, "Alfa")).unwrap();
    pdfops::rotate_page(&mut doc, 1, 90).unwrap();
    pdfops::add_page_numbers(&mut doc, "Pagina {n} van {N}").unwrap();

    let lines = textedit::text_lines(&doc, 0).unwrap();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, vec!["Alfa pagina 1", "Pagina 1 van 2"]);
    let q = lines[0].quad;
    assert!((q[0].0 - 72.0).abs() < 0.5 && q[0].1 < 700.0 && q[2].1 > 720.0, "{q:?}");
    assert!(q[1].0 > 250.0 && q[1].0 < 330.0, "breedte: {q:?}");

    let r = textedit::replace_line(&mut doc, 0, &lines[0], "Nieuwe titel hier").unwrap();
    assert_eq!(r, Replaced::OriginalFont);
    let lines = textedit::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].text, "Nieuwe titel hier");
    assert!(pdfops::page_text(&doc, 0).contains("Nieuwe titel hier"));
    // Een verouderde regel wordt geweigerd.
    let stale = lines[1].clone();
    textedit::replace_line(&mut doc, 0, &lines[1], "Blad 1").unwrap();
    assert!(textedit::replace_line(&mut doc, 0, &stale, "x").is_err());

    let saved = pdfops::to_bytes_for_saving(&doc).unwrap();
    std::fs::write(out.join("textedit.pdf"), &saved).unwrap();
    std::fs::write(out.join("textedit0.png"), render::render_png(saved, 0, 1.0).unwrap()).unwrap();
}
