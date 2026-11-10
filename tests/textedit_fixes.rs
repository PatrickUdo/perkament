//! Regressietests voor de bevindingen uit het review van `textedit`.
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use perkament::pdfops;
use perkament::textedit::{self, Replaced};

/// Eén of meer pagina's met dezelfde lettertypen:
/// F1 Helvetica, F2 Helvetica-Bold, FS een "subset" (alleen bestaande tekens bruikbaar),
/// FT een Type0-lettertype zonder ToUnicode en met een vijandige /W.
fn build(contents: &[&[u8]]) -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let simple = |doc: &mut Document, base: &str| {
        doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base, "Encoding" => "WinAnsiEncoding" })
    };
    let f1 = simple(&mut doc, "Helvetica");
    let f2 = simple(&mut doc, "Helvetica-Bold");
    let fs = simple(&mut doc, "ABCDEF+Helvetica");
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "ABCDEF+Arial",
        "W" => vec![Object::Integer(4294967295), Object::Array(vec![500.into(), 600.into()]),
                    Object::Integer(4294967000), Object::Integer(4294967295), 500.into()],
    });
    let ft = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "ABCDEF+Arial", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid)],
    });
    let cs = Object::Array(vec!["Separation".into(), "Spot".into(), "DeviceRGB".into(),
        Object::Dictionary(dictionary! { "FunctionType" => 2, "Domain" => vec![0.into(), 1.into()],
            "C0" => vec![1.into(), 1.into(), 1.into()], "C1" => vec![0.into(), 0.into(), 1.into()], "N" => 1 })]);
    let res = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => f1, "F2" => f2, "FS" => fs, "FT" => ft },
        "ColorSpace" => dictionary! { "CS0" => cs },
    });
    let mut kids = vec![];
    for content in contents {
        let c = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let p = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c, "Resources" => res,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        kids.push(Object::Reference(p));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    pdfops::load(&out).unwrap()
}

fn page(doc: &Document, index: usize) -> ObjectId {
    pdfops::page_ids(doc)[index]
}

/// De volledige, uitgepakte inhoud van een pagina.
fn content(doc: &Document, index: usize) -> Vec<u8> {
    doc.get_page_content(page(doc, index))
}

fn has(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn texts(doc: &Document, index: usize) -> Vec<String> {
    textedit::text_lines(doc, index).unwrap().into_iter().map(|l| l.text).collect()
}

fn replace(doc: &mut Document, index: usize, old: &str, new: &str) -> anyhow::Result<Replaced> {
    let lines = textedit::text_lines(doc, index)?;
    let line = lines.iter().find(|l| l.text == old).unwrap_or_else(|| panic!("regel {old:?} niet gevonden in {lines:?}"));
    textedit::replace_line(doc, index, line, new)
}

#[test]
fn inline_image_survives_an_edit() {
    let src = b"q 20 0 0 20 100 100 cm BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID 00ff00ff> EI Q BT /F1 12 Tf 72 700 Td (Hello) Tj ET 0 0 1 rg 300 300 100 100 re f";
    let mut doc = build(&[src]);
    assert_eq!(texts(&doc, 0), vec!["Hello"]);
    assert_eq!(replace(&mut doc, 0, "Hello", "Hallo").unwrap(), Replaced::OriginalFont);
    let after = content(&doc, 0);
    assert!(has(&after, b"BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID 00ff00ff> EI Q"), "inline-afbeelding is beschadigd");
    assert!(has(&after, b"0 0 1 rg 300 300 100 100 re f"));
    assert_eq!(texts(&doc, 0), vec!["Hallo"]);
}

#[test]
fn content_after_an_odd_or_unreadable_token_is_kept() {
    // "--5" is een vorm waar andere parsers op vastlopen; hier is het gewoon een getal.
    let src = b"BT /F1 12 Tf 72 700 Td (Hello) Tj ET\n0 0 m --5 0 l S\n1 0 0 rg 100 100 200 200 re f\nBT /F1 12 Tf 72 600 Td (Second line) Tj ET";
    let mut doc = build(&[src]);
    assert_eq!(texts(&doc, 0), vec!["Hello", "Second line"]);
    replace(&mut doc, 0, "Hello", "Hallo").unwrap();
    assert!(has(&content(&doc, 0), b"\n0 0 m --5 0 l S\n1 0 0 rg 100 100 200 200 re f\nBT /F1 12 Tf 72 600 Td (Second line) Tj ET"));

    // Echt onleesbaar: regels ervoor blijven bewerkbaar, alles erna blijft byte-voor-byte staan.
    let tail: &[u8] = b"\n} ) kapot { 1 0 0 rg 5 5 5 5 re f BT /F1 12 Tf 72 500 Td (Na de fout) Tj ET";
    let src = [b"BT /F1 12 Tf 72 700 Td (Hello) Tj ET".as_slice(), tail].concat();
    let mut doc = build(&[&src]);
    assert_eq!(texts(&doc, 0), vec!["Hello"]);
    replace(&mut doc, 0, "Hello", "Hallo").unwrap();
    let after = content(&doc, 0);
    assert!(has(&after, tail), "inhoud na het onleesbare token is aangetast: {}", String::from_utf8_lossy(&after));
}

#[test]
fn q_and_big_q_restore_the_text_state() {
    let src = b"BT /F1 12 Tf ET q BT /F2 30 Tf 72 700 Td (Big) Tj ET Q BT 72 600 Td (Small) Tj ET";
    let doc = build(&[src]);
    let lines = textedit::text_lines(&doc, 0).unwrap();
    let small = lines.iter().find(|l| l.text == "Small").unwrap();
    let height = small.quad[3].1 - small.quad[0].1;
    assert!((height - 12.0 * 1.06).abs() < 0.5, "hoogte {height}: lettergrootte is niet hersteld na Q");
}

#[test]
fn invisible_and_unreadable_text_is_not_offered() {
    let src = b"BT 3 Tr /F1 12 Tf 72 700 Td (Hidden ocr) Tj ET BT 0 Tr /F1 12 Tf 72 650 Td (Zichtbaar) Tj ET BT /FT 12 Tf 72 600 Td <00240025> Tj ET";
    let doc = build(&[src]);
    assert_eq!(texts(&doc, 0), vec!["Zichtbaar"]);
}

#[test]
fn trailing_space_and_following_run_are_preserved() {
    let src = b"BT /F1 12 Tf 72 700 Td (Total ) Tj /F2 12 Tf (bold) Tj ET";
    let mut doc = build(&[src]);
    let before = textedit::text_lines(&doc, 0).unwrap();
    let bold_x = before.iter().find(|l| l.text == "bold").unwrap().quad[0].0;
    assert_eq!(replace(&mut doc, 0, "Total", "Totaal").unwrap(), Replaced::OriginalFont);
    // De spatie aan het eind staat nog in de PDF.
    assert!(has(&content(&doc, 0), b"<546F7461616C20>"), "{}", String::from_utf8_lossy(&content(&doc, 0)));
    let after = textedit::text_lines(&doc, 0).unwrap();
    let new_x = after.iter().find(|l| l.text == "bold").unwrap().quad[0].0;
    assert!((bold_x - new_x).abs() < 0.01, "vervolgtekst is verschoven van {bold_x} naar {new_x}");
}

#[test]
fn substitute_font_keeps_following_runs_in_place() {
    let src = b"BT /FS 12 Tf 72 700 Td (Hello ) Tj /F2 12 Tf (bold) Tj /FS 12 Tf ( rest) Tj ET";
    let mut doc = build(&[src]);
    let x = |doc: &Document, text: &str| textedit::text_lines(doc, 0).unwrap().iter().find(|l| l.text == text).unwrap().quad[0].0;
    let (bold, rest) = (x(&doc, "bold"), x(&doc, "rest"));
    assert_eq!(replace(&mut doc, 0, "Hello", "Hallo!").unwrap(), Replaced::SubstituteFont);
    assert!((x(&doc, "bold") - bold).abs() < 0.01 && (x(&doc, "rest") - rest).abs() < 0.01, "vervolgtekst is verschoven");
    assert!(texts(&doc, 0).contains(&"Hallo!".to_string()));
    assert!((x(&doc, "Hallo!") - 72.0).abs() < 0.01);
}

#[test]
fn separately_positioned_columns_are_not_merged() {
    let src = b"BT /F1 12 Tf 72 700 Td (Item) Tj 45 0 Td (12) Tj 30 0 Td (EUR) Tj ET";
    let mut doc = build(&[src]);
    assert_eq!(texts(&doc, 0), vec!["Item", "12", "EUR"]);
    replace(&mut doc, 0, "12", "13").unwrap();
    assert_eq!(texts(&doc, 0), vec!["Item", "13", "EUR"]);
    assert!(has(&content(&doc, 0), b"45 0 Td") && has(&content(&doc, 0), b"30 0 Td (EUR) Tj"));
}

#[test]
fn fonts_without_a_space_glyph_get_a_word_gap() {
    // Zoals TeX: woordafstand als kerning, geen spatieteken in het lettertype.
    let src = b"BT /FS 10 Tf 72 700 Td [(Hello)-333(World)]TJ ET";
    let mut doc = build(&[src]);
    assert_eq!(texts(&doc, 0), vec!["Hello World"]);
    assert_eq!(replace(&mut doc, 0, "Hello World", "World Hello").unwrap(), Replaced::OriginalFont);
    let after = content(&doc, 0);
    assert!(has(&after, b"<576F726C64>-333.0<48656C6C6F>"), "{}", String::from_utf8_lossy(&after));
    assert!(!has(&after, b"20>") && !has(&after, b"<20"), "er is een spatiecode geschreven die het lettertype niet heeft");
    assert_eq!(texts(&doc, 0), vec!["World Hello"]);
}

#[test]
fn hostile_cid_widths_do_not_panic() {
    let doc = build(&[b"BT /FT 12 Tf 72 700 Td <00240025002600480049> Tj ET"]);
    assert!(textedit::text_lines(&doc, 0).unwrap().is_empty());
}

#[test]
fn substitute_text_keeps_the_original_colour_space() {
    let src = b"/CS0 cs 0.5 scn BT /FS 12 Tf 72 700 Td (Hello) Tj ET";
    let mut doc = build(&[src]);
    assert_eq!(replace(&mut doc, 0, "Hello", "Hallo!").unwrap(), Replaced::SubstituteFont);
    let after = content(&doc, 0);
    let overlay = &after[after.len() - 160.min(after.len())..];
    assert!(has(overlay, b"/CS0 cs 0.5 scn BT"), "{}", String::from_utf8_lossy(overlay));
}

#[test]
fn failed_substitution_leaves_the_page_untouched() {
    let mut doc = build(&[b"BT /FS 12 Tf 72 700 Td (Hello) Tj ET"]);
    let before = content(&doc, 0);
    assert!(replace(&mut doc, 0, "Hello", "Hello \u{0416}").is_err());
    assert_eq!(content(&doc, 0), before);
    assert_eq!(texts(&doc, 0), vec!["Hello"]);
}

#[test]
fn shared_content_stream_is_copied_before_editing() {
    let mut doc = build(&[b"BT /F1 12 Tf 72 700 Td (Gedeeld) Tj ET", b"0 g"]);
    let shared = doc.get_page_contents(page(&doc, 0))[0];
    let second = page(&doc, 1);
    doc.get_dictionary_mut(second).unwrap().set("Contents", shared);
    assert_eq!(texts(&doc, 1), vec!["Gedeeld"]);
    replace(&mut doc, 0, "Gedeeld", "Gewijzigd").unwrap();
    assert_eq!(texts(&doc, 0), vec!["Gewijzigd"]);
    assert_eq!(texts(&doc, 1), vec!["Gedeeld"], "de andere pagina is meeveranderd");
}

#[test]
fn quote_operators_keep_their_line_moves() {
    let src = b"BT /F1 12 Tf 14 TL 72 700 Td (Een) Tj (Twee) ' 2 1 (Drie) \" ET";
    let mut doc = build(&[src]);
    let y = |doc: &Document, text: &str| textedit::text_lines(doc, 0).unwrap().iter().find(|l| l.text == text).unwrap().quad[0].1;
    assert_eq!(texts(&doc, 0), vec!["Een", "Twee", "Drie"]);
    let drie = y(&doc, "Drie");
    replace(&mut doc, 0, "Twee", "Zwei").unwrap();
    assert_eq!(texts(&doc, 0), vec!["Een", "Zwei", "Drie"]);
    assert!((y(&doc, "Drie") - drie).abs() < 0.01);
    replace(&mut doc, 0, "Drie", "Drei").unwrap();
    assert!(has(&content(&doc, 0), b"2 Tw 1 Tc T* ["), "{}", String::from_utf8_lossy(&content(&doc, 0)));
    assert!((y(&doc, "Drei") - drie).abs() < 0.01);
}

#[test]
fn stale_line_is_refused() {
    let mut doc = build(&[b"BT /F1 12 Tf 72 700 Td (Een) Tj ET"]);
    let stale = textedit::text_lines(&doc, 0).unwrap()[0].clone();
    replace(&mut doc, 0, "Een", "Twee").unwrap();
    assert!(textedit::replace_line(&mut doc, 0, &stale, "x").is_err());
}
