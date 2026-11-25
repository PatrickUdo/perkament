//! Kwaadaardige PDF's: decompressiebommen, reuzenafbeeldingen, absurde nesting.
//! Niets hiervan mag het geheugen opblazen, vastlopen of de app laten crashen.
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use perkament::{pdfops, render, textedit};
use std::io::Write;
use std::time::{Duration, Instant};

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// `n` bytes aan spaties, gecomprimeerd zonder ze ooit in het geheugen te hebben.
fn bomb(n: usize) -> Vec<u8> {
    let chunk = vec![b' '; 1 << 20];
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    for _ in 0..n >> 20 {
        e.write_all(&chunk).unwrap();
    }
    e.finish().unwrap()
}

fn flate(dict: lopdf::Dictionary, data: Vec<u8>) -> Stream {
    let mut d = dict;
    d.set("Filter", "FlateDecode");
    Stream::new(d, data)
}

/// Eén pagina met de gegeven inhoud en eventueel een afbeelding als /Im1.
fn build(content: Stream, image: Option<Stream>) -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let mut resources = dictionary! { "Font" => dictionary! { "F1" => font } };
    if let Some(image) = image {
        let im = doc.add_object(image);
        resources.set("XObject", dictionary! { "Im1" => im });
    }
    let c = doc.add_object(content);
    let p = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => c, "Resources" => resources,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![p.into()], "Count" => 1 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    // Zoals de app: opslaan en opnieuw laden via pdfops.
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    pdfops::load(&out).unwrap()
}

fn content_id(doc: &Document) -> ObjectId {
    doc.get_page_contents(pdfops::page_ids(doc)[0])[0]
}

fn image(dict_size: (i64, i64), filter: &str, data: Vec<u8>) -> Stream {
    Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => dict_size.0, "Height" => dict_size.1,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => filter,
        },
        data,
    )
}

#[test]
fn decompression_bomb_is_not_unpacked_for_drawing_or_reading() {
    // 1 GB aan inhoud in ongeveer 1 MB.
    let mut doc = build(flate(dictionary! {}, bomb(1 << 30)), None);
    let id = content_id(&doc);
    let Ok(Object::Stream(s)) = doc.get_object(id) else { panic!() };
    assert!(pdfops::stream_is_unsafe(&doc, s));

    let start = Instant::now();
    assert!(textedit::text_lines(&doc, 0).is_err());
    assert_eq!(pdfops::page_text(&doc, 0), "");
    assert_eq!(textedit::repair_overlay_colors(&mut doc), 0);
    let bytes = pdfops::to_render_bytes(&mut doc, &mut Default::default()).unwrap();
    assert!(bytes.len() < 64 * 1024, "de bom gaat niet mee naar de renderer");
    render::render_png_with_timeout(bytes, 0, 1.0, Duration::from_secs(20)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(30));

    // Het document zelf blijft onveranderd: opslaan bewaart de oorspronkelijke stream.
    let Ok(Object::Stream(s)) = doc.get_object(id) else { panic!() };
    assert!(s.content.len() > 100_000);
}

#[test]
fn normal_streams_are_left_alone() {
    let text = b"BT /F1 12 Tf 72 720 Td (Gewoon een factuur) Tj ET".repeat(1000);
    let pixels = vec![0x80u8; 500 * 400];
    let mut doc = build(flate(dictionary! {}, zlib(&text)), Some(image((500, 400), "FlateDecode", zlib(&pixels))));
    for obj in doc.objects.values() {
        if let Object::Stream(s) = obj {
            assert!(!pdfops::stream_is_unsafe(&doc, s));
        }
    }
    let plain = pdfops::to_bytes(&mut doc).unwrap();
    let guarded = pdfops::to_render_bytes(&mut doc, &mut Default::default()).unwrap();
    assert_eq!(plain, guarded);
    assert!(pdfops::page_text(&doc, 0).contains("Gewoon een factuur"));
}

#[test]
fn giant_images_are_replaced_by_one_pixel_for_drawing() {
    let content = Stream::new(dictionary! {}, b"q 595 0 0 842 0 0 cm /Im1 Do Q".to_vec());
    // Het woordenboek belooft 100.000 x 100.000 pixels.
    let mut doc = build(content.clone(), Some(image((100_000, 100_000), "FlateDecode", zlib(&[0; 100]))));
    let bytes = pdfops::to_render_bytes(&mut doc, &mut Default::default()).unwrap();
    let rendered = lopdf::Document::load_mem(&bytes).unwrap();
    let widths: Vec<i64> = rendered
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter_map(|s| s.dict.get(b"Width").and_then(Object::as_i64).ok())
        .collect();
    assert_eq!(widths, vec![1]);
    render::render_png_with_timeout(bytes, 0, 1.0, Duration::from_secs(20)).unwrap();

    // Een JPEG die in het woordenboek klein is, maar in de JPEG zelf 65000 x 65000.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8];
    jpeg.extend_from_slice(&65000u16.to_be_bytes());
    jpeg.extend_from_slice(&65000u16.to_be_bytes());
    jpeg.extend_from_slice(&[1, 1, 0x11, 0, 0xFF, 0xD9]);
    let doc = build(content, Some(image((10, 10), "DCTDecode", jpeg)));
    let unsafe_images =
        doc.objects.values().filter_map(|o| o.as_stream().ok()).filter(|s| pdfops::stream_is_unsafe(&doc, s)).count();
    assert_eq!(unsafe_images, 1);
}

#[test]
fn deep_save_nesting_stays_cheap() {
    let mut content = b"q ".repeat(300_000);
    content.extend_from_slice(b"BT /F1 12 Tf 72 720 Td (diep) Tj ET ");
    content.extend_from_slice(&b"Q ".repeat(300_000));
    let doc = build(flate(dictionary! {}, zlib(&content)), None);
    assert!(matches!(textedit::complexity(&doc, 0), textedit::Complexity::DeepNesting));
    assert!(pdfops::page_text(&doc, 0).contains("diep"));
    let lines = textedit::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].text, "diep");
}

#[test]
fn pages_with_absurdly_many_operators_are_skipped() {
    let content = b"0 0 m ".repeat(400_000);
    let doc = build(flate(dictionary! {}, zlib(&content)), None);
    assert!(matches!(textedit::complexity(&doc, 0), textedit::Complexity::TooLarge));
    assert!(textedit::text_lines(&doc, 0).is_err());
    assert_eq!(pdfops::page_text(&doc, 0), "");
}

#[test]
fn replacing_a_line_of_many_fragments_is_linear() {
    // 200.000 stukjes op één regel: met `splice` per stukje duurde dit minuten.
    let mut content = b"BT /F1 4 Tf 10 400 Td ".to_vec();
    content.extend_from_slice(&b"(a)Tj ".repeat(200_000));
    content.extend_from_slice(b"ET");
    let mut doc = build(flate(dictionary! {}, zlib(&content)), None);
    let lines = textedit::text_lines(&doc, 0).unwrap();
    let start = Instant::now();
    textedit::replace_line(&mut doc, 0, &lines[0], "kort").unwrap();
    assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
    assert_eq!(textedit::text_lines(&doc, 0).unwrap()[0].text, "kort");
}

#[test]
fn stray_delimiters_do_not_stall_the_scan() {
    for junk in [&b") ) )"[..], b"(((", b"(\\", b"%", b"<<>>]]}}", b")\\)", b""] {
        let doc = build(Stream::new(dictionary! {}, junk.to_vec()), None);
        let _ = textedit::complexity(&doc, 0);
        let _ = textedit::text_lines(&doc, 0);
        let _ = pdfops::page_text(&doc, 0);
    }
}
