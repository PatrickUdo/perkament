//! Regressietests voor de bevindingen uit het review van `pdfops`.
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use perkament::pdfops::{self, AnnotKind, NewAnnot};
use perkament::render;

/// Document met `n` pagina's; elke pagina toont "Geheim <i>" in Helvetica.
/// `page_extra` kan per pagina sleutels toevoegen (CropBox, Rotate, ...).
fn build(n: usize, page_extra: impl Fn(usize) -> Dictionary) -> (Document, Vec<ObjectId>) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let mut ids = vec![];
    for i in 0..n {
        let content = format!("BT /F1 24 Tf 72 700 Td (Geheim {}) Tj ET", i + 1);
        let c = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        };
        for (k, v) in page_extra(i).iter() {
            page.set(k.clone(), v.clone());
        }
        ids.push(doc.add_object(page));
    }
    let kids: Vec<Object> = ids.iter().map(|&id| id.into()).collect();
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    (doc, ids)
}

fn bytes(doc: &mut Document) -> Vec<u8> {
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    // Opgeslagen streams zijn gecomprimeerd: opnieuw laden en platte inhoud doorzoeken.
    let doc = Document::load_mem(haystack).unwrap();
    doc.objects.values().any(|o| match o {
        Object::Stream(s) => {
            let data = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
            data.windows(needle.len()).any(|w| w == needle.as_bytes())
        }
        _ => false,
    }) || haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

fn catalog_id(doc: &Document) -> ObjectId {
    doc.trailer.get(b"Root").unwrap().as_reference().unwrap()
}

#[test]
fn removed_pages_do_not_survive_in_saved_file() {
    let (mut doc, ids) = build(2, |_| Dictionary::new());
    // Bladwijzer en link die naar pagina 2 wijzen houden die pagina anders in leven.
    let item = doc.add_object(dictionary! { "Title" => Object::string_literal("Twee"), "Dest" => vec![ids[1].into(), "Fit".into()] });
    let outlines = doc.add_object(dictionary! { "Type" => "Outlines", "First" => item, "Last" => item, "Count" => 1 });
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        "A" => dictionary! { "S" => "GoTo", "D" => vec![ids[1].into(), "Fit".into()] },
    });
    doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", vec![Object::Reference(link)]);
    let cat = catalog_id(&doc);
    doc.get_dictionary_mut(cat).unwrap().set("Outlines", outlines);
    doc.get_dictionary_mut(cat).unwrap().set("OpenAction", vec![ids[1].into(), "Fit".into()]);
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();

    let extracted = pdfops::extract_pages(&doc, &[0]).unwrap();
    assert!(contains(&extracted, "Geheim 1"));
    assert!(!contains(&extracted, "Geheim 2"), "geëxtraheerd bestand bevat nog pagina 2");

    pdfops::delete_pages(&mut doc, &[1]).unwrap();
    let saved = pdfops::to_bytes_for_saving(&doc).unwrap();
    assert!(!contains(&saved, "Geheim 2"), "opgeslagen bestand bevat nog de verwijderde pagina");
    let again = pdfops::load(&saved).unwrap();
    assert_eq!(pdfops::page_count(&again), 1);
    // Geen half gestripte bestemmingen zoals [/Fit].
    for obj in again.objects.values() {
        if let Ok(d) = obj.as_dict() {
            for key in [b"Dest".as_slice(), b"OpenAction"] {
                if let Ok(Object::Array(a)) = d.get(key) {
                    assert!(a.first().is_some_and(|o| o.as_reference().is_ok()), "kapotte bestemming: {a:?}");
                }
            }
        }
    }
    // Ongedaan maken in de app werkt op het document in geheugen: dat is niet aangetast.
    assert_eq!(pdfops::page_count(&doc), 1);
}

#[test]
fn deleting_a_comment_keeps_inline_annotations_and_removes_popup() {
    let (mut doc, ids) = build(1, |_| Dictionary::new());
    let note = doc.new_object_id();
    let popup = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Popup", "Parent" => note, "Open" => true,
        "Rect" => vec![100.into(), 100.into(), 200.into(), 200.into()],
    });
    doc.objects.insert(note, Object::Dictionary(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "Popup" => popup,
        "Contents" => Object::string_literal("vertrouwelijke opmerking"),
        "Rect" => vec![50.into(), 50.into(), 70.into(), 70.into()],
    }));
    let reply = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "IRT" => note, "Contents" => Object::string_literal("antwoord erop"),
        "Rect" => vec![50.into(), 50.into(), 70.into(), 70.into()],
    });
    let inline = Object::Dictionary(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()],
        "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.org/inline") },
    });
    doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", vec![note.into(), popup.into(), reply.into(), inline]);
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();

    let listed = pdfops::list_annotations(&doc);
    assert_eq!(listed.len(), 2);
    let target = listed.iter().find(|a| a.contents == "vertrouwelijke opmerking").unwrap().id;
    pdfops::delete_annotation(&mut doc, 0, target).unwrap();
    assert!(pdfops::list_annotations(&doc).is_empty(), "reactie op de verwijderde opmerking bleef staan");
    let saved = pdfops::to_bytes_for_saving(&doc).unwrap();
    assert!(!contains(&saved, "vertrouwelijke opmerking"));
    assert!(!contains(&saved, "antwoord erop"));
    assert!(!contains(&saved, "/Popup"));
    assert!(contains(&saved, "example.org/inline"), "direct opgenomen link is verdwenen");
}

#[test]
fn overlays_use_the_visible_crop_box() {
    let crop = |_| dictionary! { "CropBox" => vec![100.into(), 200.into(), 500.into(), 800.into()] };
    let (mut doc, _) = build(1, crop);
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    let before = render::render_png(pdfops::to_bytes(&mut doc).unwrap(), 0, 1.0).unwrap();
    pdfops::add_page_numbers(&mut doc, "Pagina {n} van {N}").unwrap();
    let after = render::render_png(pdfops::to_bytes(&mut doc).unwrap(), 0, 1.0).unwrap();
    assert_ne!(before, after, "paginanummer valt buiten het zichtbare deel van de pagina");

    pdfops::insert_blank_page(&mut doc, 1).unwrap();
    let geoms = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap();
    assert!((geoms[0].w - geoms[1].w).abs() < 0.5 && (geoms[0].h - geoms[1].h).abs() < 0.5, "{geoms:?}");
}

#[test]
fn zero_area_page_gives_no_nan() {
    let (mut doc, _) = build(1, |_| dictionary! { "MediaBox" => vec![0.into(), 0.into(), 0.into(), 0.into()] });
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    pdfops::add_watermark(&mut doc, &pdfops::Watermark { text: "X".into(), ..Default::default() }).unwrap();
    let saved = pdfops::to_bytes(&mut doc).unwrap();
    assert!(!contains(&saved, "NaN") && !contains(&saved, "inf"));
}

#[test]
fn pdfdoc_encoded_metadata_survives_saving_untouched() {
    let (mut doc, _) = build(1, |_| Dictionary::new());
    // PDFDocEncoding voor: A, en-streepje, B, spatie, dubbele aanhalingstekens om q, spatie, euro, 5.
    let raw = vec![0x41, 0x85, 0x42, 0x20, 0x8D, 0x71, 0x8E, 0x20, 0xA0, 0x35];
    let info = doc.add_object(dictionary! { "Title" => Object::String(raw.clone(), StringFormat::Literal) });
    doc.trailer.set("Info", info);
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    assert_eq!(pdfops::get_meta(&doc).title, "A\u{2013}B \u{201C}q\u{201D} \u{20AC}5");

    let saved = pdfops::load(&pdfops::to_bytes_for_saving(&doc).unwrap()).unwrap();
    let info = saved.trailer.get(b"Info").unwrap().as_reference().unwrap();
    assert_eq!(saved.get_dictionary(info).unwrap().get(b"Title").unwrap().as_str().unwrap(), &raw[..], "titel is herschreven");

    // Een ander veld wijzigen laat de titel met rust.
    let mut meta = pdfops::get_meta(&doc);
    meta.author = "Zoë".into();
    pdfops::set_meta(&mut doc, &meta).unwrap();
    let info = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
    assert_eq!(doc.get_dictionary(info).unwrap().get(b"Title").unwrap().as_str().unwrap(), &raw[..]);
    assert_eq!(pdfops::get_meta(&doc).author, "Zoë");
}

#[test]
fn editing_metadata_drops_contradicting_xmp() {
    let (mut doc, _) = build(1, |_| Dictionary::new());
    let xmp = doc.add_object(Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, b"<x>Oude titel</x>".to_vec()));
    let cat = catalog_id(&doc);
    doc.get_dictionary_mut(cat).unwrap().set("Metadata", xmp);
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    let mut meta = pdfops::get_meta(&doc);
    meta.title = "Nieuwe titel".into();
    pdfops::set_meta(&mut doc, &meta).unwrap();
    let saved = pdfops::to_bytes_for_saving(&doc).unwrap();
    assert!(!contains(&saved, "Oude titel"));
    assert_eq!(pdfops::get_meta(&pdfops::load(&saved).unwrap()).title, "Nieuwe titel");
}

#[test]
fn unsupported_characters_fail_cleanly_and_winansi_works() {
    let (mut doc, _) = build(2, |_| Dictionary::new());
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    let before = doc.objects.len();
    let greek = pdfops::Watermark { text: "\u{0395}\u{03BB}\u{03BB}".into(), ..Default::default() };
    let err = pdfops::add_watermark(&mut doc, &greek).unwrap_err().to_string();
    assert!(err.contains('\u{0395}'), "{err}");
    assert_eq!(doc.objects.len(), before, "mislukte bewerking heeft het document toch gewijzigd");
    assert!(pdfops::add_page_numbers(&mut doc, "\u{0141} {n}").is_err());
    assert_eq!(doc.objects.len(), before);

    // Tekens uit het bovenste WinAnsi-blok (OE-ligatuur, S met haček, handelsmerk) werken wel.
    let ok = pdfops::Watermark { text: "\u{0152}uvre \u{0160}\u{2122}".into(), ..Default::default() };
    pdfops::add_watermark(&mut doc, &ok).unwrap();
    assert!(!contains(&pdfops::to_bytes(&mut doc).unwrap(), "?uvre"));
}

#[test]
fn search_folds_case_the_same_way_on_both_sides() {
    let texts = vec!["\u{039F}\u{0394}\u{039F}\u{03A3} \u{0391}\u{0398}\u{0397}\u{039D}\u{0391}\u{03A3}".to_string(), "Welkom in \u{0130}stanbul vandaag".to_string()];
    assert_eq!(pdfops::search(&texts, "\u{039F}\u{0394}\u{039F}\u{03A3}").len(), 1);
    assert_eq!(pdfops::search(&texts, "\u{03BF}\u{03B4}\u{03BF}\u{03C2}").len(), 1);
    let hits = pdfops::search(&texts, "\u{0130}stanbul");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("stanbul vandaag"));
    assert_eq!(pdfops::search(&texts, "WELKOM")[0].page, 1);
}

#[test]
fn page_tree_with_dangling_and_duplicate_kids() {
    let (mut doc, ids) = build(3, |i| dictionary! { "MediaBox" => vec![0.into(), 0.into(), (200 + 100 * i as i64).into(), 300.into()] });
    let root = doc.get_dictionary(catalog_id(&doc)).unwrap().get(b"Pages").unwrap().as_reference().unwrap();
    let kids: Vec<Object> = vec![ids[0].into(), ids[0].into(), Object::Reference((9999, 0)), ids[1].into(), ids[2].into()];
    let pages = doc.get_dictionary_mut(root).unwrap();
    pages.set("Kids", kids);
    pages.set("Count", 5);
    let mut doc = pdfops::load(&bytes(&mut doc)).expect("onoplosbare /Kids mag het laden niet blokkeren");
    let geoms = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap();
    assert_eq!(pdfops::page_count(&doc), geoms.len());
    assert_eq!(geoms.iter().map(|g| g.w.round() as i32).collect::<Vec<_>>(), vec![200, 300, 400]);
    // De laatst getoonde pagina verwijderen raakt ook echt die pagina.
    pdfops::delete_pages(&mut doc, &[2]).unwrap();
    let geoms = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap();
    assert_eq!(geoms.iter().map(|g| g.w.round() as i32).collect::<Vec<_>>(), vec![200, 300]);
}

#[test]
fn rotation_stored_as_real_or_invalid() {
    let (mut doc, _) = build(2, |i| if i == 0 { dictionary! { "Rotate" => 90.0 } } else { dictionary! { "Rotate" => 45 } });
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    let geoms = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap();
    assert!((geoms[0].w - 842.0).abs() < 1.0, "reële /Rotate 90 wordt genegeerd: {geoms:?}");
    assert!((geoms[1].w - 595.0).abs() < 1.0, "ongeldige /Rotate moet als 0 tellen: {geoms:?}");
    pdfops::rotate_page(&mut doc, 0, 90).unwrap();
    let geoms = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap();
    assert!((geoms[0].w - 595.0).abs() < 1.0);
}

fn annot_rect(doc: &Document, id: ObjectId) -> Vec<f32> {
    doc.get_dictionary(id).unwrap().get(b"Rect").unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect()
}

#[test]
fn text_box_and_note_are_upright_on_rotated_pages() {
    for rotate in [0i64, 90, 180, 270] {
        let (mut doc, _) = build(1, |_| dictionary! { "Rotate" => rotate, "MediaBox" => vec![0.into(), 0.into(), 400.into(), 600.into()] });
        let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
        let g = render::page_geometries(pdfops::to_bytes(&mut doc).unwrap()).unwrap()[0];
        // Klik in de weergave op (100, 80): daar hoort de linkerbovenhoek te komen.
        let (x, y) = g.view_to_pdf(100.0, 80.0);
        let mk = |kind| NewAnnot { kind, rect: [x, y, x, y], color: [1.0, 0.8, 0.0], text: "Een tekstvak met een paar woorden".into(), points: vec![] };
        for kind in [AnnotKind::FreeText, AnnotKind::Note] {
            let id = pdfops::add_annotation(&mut doc, 0, &mk(kind)).unwrap();
            let r = annot_rect(&doc, id);
            let (a, b) = (g.pdf_to_view(r[0], r[1]), g.pdf_to_view(r[2], r[3]));
            let (left, top, right, bottom) = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
            assert!((left - 100.0).abs() < 0.5 && (top - 80.0).abs() < 0.5, "rotatie {rotate} {kind:?}: hoek op ({left},{top})");
            if kind == AnnotKind::FreeText {
                assert!((right - left - 200.0).abs() < 0.5, "rotatie {rotate}: breedte {}", right - left);
                assert!(bottom - top < 80.0);
            } else {
                assert!((right - left - 22.0).abs() < 0.5 && (bottom - top - 22.0).abs() < 0.5);
            }
        }
    }
}

#[test]
fn click_placed_text_box_stays_on_the_page() {
    let (mut doc, _) = build(1, |_| Dictionary::new());
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    let a = NewAnnot { kind: AnnotKind::FreeText, rect: [560.0, 30.0, 560.0, 30.0], color: [1.0, 0.0, 0.0], text: "regel een\nregel twee\nregel drie".into(), points: vec![] };
    let id = pdfops::add_annotation(&mut doc, 0, &a).unwrap();
    let r = annot_rect(&doc, id);
    assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 595.0 && r[3] <= 842.0, "tekstvak valt buiten de pagina: {r:?}");
}

#[test]
fn insert_pdf_keeps_form_fields_and_named_destinations() {
    let (mut other, ids) = build(2, |_| Dictionary::new());
    let widget = other.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("naam"),
        "Rect" => vec![10.into(), 10.into(), 100.into(), 30.into()], "P" => ids[0],
    });
    let link = other.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()],
        "Dest" => Object::string_literal("sec2"),
    });
    other.get_dictionary_mut(ids[0]).unwrap().set("Annots", vec![Object::Reference(widget), Object::Reference(link)]);
    let cat = catalog_id(&other);
    let catalog = other.get_dictionary_mut(cat).unwrap();
    catalog.set("AcroForm", dictionary! { "Fields" => vec![Object::Reference(widget)], "DA" => Object::string_literal("/Helv 0 Tf 0 g") });
    catalog.set("Names", dictionary! { "Dests" => dictionary! {
        "Names" => vec![Object::string_literal("sec2"), Object::Array(vec![ids[1].into(), "Fit".into()])],
    } });
    let other_bytes = bytes(&mut other);

    let (mut doc, _) = build(1, |_| Dictionary::new());
    let mut doc = pdfops::load(&bytes(&mut doc)).unwrap();
    assert_eq!(pdfops::insert_pdf(&mut doc, &other_bytes, 1).unwrap(), 2);
    let saved = pdfops::load(&pdfops::to_bytes_for_saving(&doc).unwrap()).unwrap();
    let pages = pdfops::page_ids(&saved);
    let form = saved.get_dictionary(catalog_id(&saved)).unwrap().get(b"AcroForm").unwrap();
    let form = saved.dereference(form).unwrap().1.as_dict().unwrap();
    assert_eq!(form.get(b"Fields").unwrap().as_array().unwrap().len(), 1);
    assert!(form.has(b"DA"));
    // De link wijst nu expliciet naar de tweede ingevoegde pagina.
    let dest = saved.objects.values().filter_map(|o| o.as_dict().ok())
        .find(|d| d.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) == Some(b"Link".as_slice()))
        .and_then(|d| d.get(b"Dest").ok()).and_then(|o| o.as_array().ok()).expect("bestemming is niet expliciet gemaakt");
    assert_eq!(dest[0].as_reference().unwrap(), pages[2]);
}
