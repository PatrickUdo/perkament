//! Bewerkingen op een PDF-document (bovenop `lopdf`).
//!
//! Alle functies werken op een document waarvan de paginaboom is afgevlakt
//! (zie [`load`]): elke pagina hangt direct onder de root en draagt zijn eigen
//! MediaBox/Resources/Rotate. Dat maakt herschikken, verwijderen en invoegen
//! een kwestie van de `Kids`-lijst herschrijven.

use crate::i18n::{t, tf};
use anyhow::{anyhow, bail, Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use std::collections::HashSet;

pub const APP_NAME: &str = "Perkament";

// ---------------------------------------------------------------------------
// Laden / opslaan
// ---------------------------------------------------------------------------

pub fn load(bytes: &[u8]) -> Result<Document> {
    let mut doc = Document::load_mem(bytes).context(t("err.pdf_read"))?;
    if doc.is_encrypted() {
        doc.decrypt("")
            .map_err(|_| anyhow!(t("err.password")))?;
    }
    flatten_page_tree(&mut doc)?;
    Ok(doc)
}

/// Serialiseert het document zoals het nu is (voor de renderer).
pub fn to_bytes(doc: &mut Document) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out).context(t("err.serialize"))?;
    Ok(out)
}

/// Serialiseert een opgeschoonde, gecomprimeerde kopie (voor opslaan op schijf).
pub fn to_bytes_for_saving(doc: &Document) -> Result<Vec<u8>> {
    let mut copy = doc.clone();
    purge_removed_pages(&mut copy);
    // Alleen Producer en ModDate bijwerken; de overige velden blijven byte-voor-byte gelijk.
    let mut info = info_dict(&copy).cloned().unwrap_or_default();
    info.set("Producer", Object::string_literal(APP_NAME));
    info.set("ModDate", now_pdf_date());
    store_info(&mut copy, info);
    copy.prune_objects();
    copy.compress();
    to_bytes(&mut copy)
}

/// Verwijst `obj` naar een pagina die niet meer in het document zit, of is het
/// een bestemming (`[pagina /Fit …]`) naar zo'n pagina?
fn points_at_removed(obj: &Object, removed: &HashSet<ObjectId>) -> bool {
    match obj {
        Object::Reference(id) => removed.contains(id),
        Object::Array(items) => matches!(items.first(), Some(Object::Reference(id)) if removed.contains(id)),
        _ => false,
    }
}

fn scrub(obj: &mut Object, removed: &HashSet<ObjectId>) {
    match obj {
        Object::Array(items) => {
            items.retain(|item| !matches!(item, Object::Reference(id) if removed.contains(id)));
            for item in items.iter_mut() {
                if points_at_removed(item, removed) {
                    *item = Object::Null;
                } else {
                    scrub(item, removed);
                }
            }
        }
        Object::Dictionary(dict) => scrub_dict(dict, removed),
        Object::Stream(stream) => scrub_dict(&mut stream.dict, removed),
        _ => {}
    }
}

fn scrub_dict(dict: &mut Dictionary, removed: &HashSet<ObjectId>) {
    let dead: Vec<Vec<u8>> =
        dict.iter().filter(|(_, v)| points_at_removed(v, removed)).map(|(k, _)| k.clone()).collect();
    for key in dead {
        dict.remove(&key);
    }
    for (_, value) in dict.iter_mut() {
        scrub(value, removed);
    }
}

/// Haalt pagina's die niet meer in de paginaboom staan echt uit het document.
///
/// Zonder dit blijven ze bij het opslaan bestaan zodra iets er nog naar wijst
/// (bladwijzer, link, formulierveld), en staat de inhoud van een "verwijderde"
/// pagina dus nog in het bestand.
fn purge_removed_pages(doc: &mut Document) {
    let keep: HashSet<ObjectId> = page_ids(doc).into_iter().collect();
    let removed: HashSet<ObjectId> = doc
        .objects
        .iter()
        .filter(|(id, obj)| {
            !keep.contains(id)
                && obj.as_dict().is_ok_and(|d| d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Page".as_slice()))
        })
        .map(|(id, _)| *id)
        .collect();
    if removed.is_empty() {
        return;
    }
    for id in &removed {
        doc.objects.remove(id);
    }
    for obj in doc.objects.values_mut() {
        scrub(obj, &removed);
    }
    scrub_dict(&mut doc.trailer, &removed);
}

// ---------------------------------------------------------------------------
// Paginaboom
// ---------------------------------------------------------------------------

pub fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

pub fn page_count(doc: &Document) -> usize {
    doc.get_pages().len()
}

pub(crate) fn page_id(doc: &Document, index: usize) -> Result<ObjectId> {
    page_ids(doc)
        .get(index)
        .copied()
        .ok_or_else(|| anyhow!("{}", tf("err.no_page", &[("n", &(index + 1).to_string())])))
}

fn pages_root(doc: &Document) -> Result<ObjectId> {
    Ok(doc.catalog()?.get(b"Pages")?.as_reference()?)
}

fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        let node = doc.get_dictionary(current).ok()?;
        if let Ok(value) = node.get(key) {
            if !matches!(doc.dereference(value), Ok((_, Object::Null)) | Err(_)) {
                return Some(value.clone());
            }
        }
        current = node.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn flatten_page_tree(doc: &mut Document) -> Result<()> {
    // Onoplosbare /Kids overslaan en dubbele vermeldingen één keer tellen, zodat
    // onze telling gelijk blijft aan die van de renderer.
    let mut seen = HashSet::new();
    let pages: Vec<ObjectId> =
        page_ids(doc).into_iter().filter(|id| doc.get_dictionary(*id).is_ok() && seen.insert(*id)).collect();
    if pages.is_empty() {
        bail!(t("err.no_pages"));
    }
    for &pid in &pages {
        let media = inherited(doc, pid, b"MediaBox").unwrap_or_else(a4_box);
        let crop = inherited(doc, pid, b"CropBox");
        let resources =
            inherited(doc, pid, b"Resources").unwrap_or_else(|| Object::Dictionary(Dictionary::new()));
        // Rotatie als geheel getal; alles wat geen veelvoud van 90 is telt als 0, net als in de renderer.
        let rotate = inherited(doc, pid, b"Rotate")
            .and_then(|o| number(doc, &o))
            .filter(|r| r.is_finite())
            .map(|r| (r.round() as i64).rem_euclid(360))
            .filter(|r| r % 90 == 0)
            .unwrap_or(0);
        let page = doc.get_dictionary_mut(pid)?;
        page.set("MediaBox", media);
        if let Some(crop) = crop {
            page.set("CropBox", crop);
        }
        page.set("Resources", resources);
        page.set("Rotate", rotate);
    }
    set_page_order(doc, &pages)
}

fn a4_box() -> Object {
    Object::Array(vec![0.into(), 0.into(), 595.into(), 842.into()])
}

/// Zet de pagina's in de gegeven volgorde direct onder de root van de paginaboom.
pub fn set_page_order(doc: &mut Document, order: &[ObjectId]) -> Result<()> {
    if order.is_empty() {
        bail!(t("err.keep_one"));
    }
    let root = pages_root(doc)?;
    for &pid in order {
        doc.get_dictionary_mut(pid)?.set("Parent", root);
    }
    let root_dict = doc.get_dictionary_mut(root)?;
    root_dict.set(
        "Kids",
        order.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>(),
    );
    root_dict.set("Count", order.len() as i64);
    Ok(())
}

fn number(doc: &Document, obj: &Object) -> Option<f32> {
    doc.dereference(obj).ok()?.1.as_float().ok()
}

fn page_box(doc: &Document, pid: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    let obj = doc.get_dictionary(pid).ok()?.get(key).ok()?;
    let Ok((_, Object::Array(arr))) = doc.dereference(obj) else { return None };
    let v: Vec<f32> = arr.iter().filter_map(|o| number(doc, o)).filter(|n| n.is_finite()).collect();
    (arr.len() == 4 && v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// MediaBox als (x0, y0, x1, y1), genormaliseerd.
pub fn media_box(doc: &Document, pid: ObjectId) -> [f32; 4] {
    page_box(doc, pid, b"MediaBox").unwrap_or([0.0, 0.0, 595.0, 842.0])
}

/// Het deel van de pagina dat een viewer toont: de CropBox binnen de MediaBox.
/// Een box zonder oppervlak valt terug op A4, net als in de renderer.
pub fn visible_box(doc: &Document, pid: ObjectId) -> [f32; 4] {
    let media = media_box(doc, pid);
    let b = match page_box(doc, pid, b"CropBox") {
        Some(c) => [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])],
        None => media,
    };
    if b[2] - b[0] < 0.01 || b[3] - b[1] < 0.01 {
        [b[0], b[1], b[0] + 595.0, b[1] + 842.0]
    } else {
        b
    }
}

pub fn rotation(doc: &Document, pid: ObjectId) -> i64 {
    doc.get_dictionary(pid)
        .ok()
        .and_then(|p| p.get(b"Rotate").ok())
        .and_then(|o| number(doc, o))
        .filter(|r| r.is_finite())
        .map(|r| (r.round() as i64).rem_euclid(360))
        .filter(|r| r % 90 == 0)
        .unwrap_or(0)
}

/// Afmetingen zoals getoond (rekening houdend met rotatie) plus de matrix die
/// "weergaveruimte" (oorsprong linksonder) naar PDF-gebruikersruimte afbeeldt.
fn display_space(doc: &Document, pid: ObjectId) -> (f32, f32, [f32; 6]) {
    let [x0, y0, x1, y1] = visible_box(doc, pid);
    let (w, h) = (x1 - x0, y1 - y0);
    match rotation(doc, pid) {
        90 => (h, w, [0.0, 1.0, -1.0, 0.0, x0 + w, y0]),
        180 => (w, h, [-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h]),
        270 => (h, w, [0.0, -1.0, 1.0, 0.0, x0, y0 + h]),
        _ => (w, h, [1.0, 0.0, 0.0, 1.0, x0, y0]),
    }
}

/// PDF-gebruikersruimte -> weergaveruimte (de matrix is een rotatie plus verschuiving).
fn to_display(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    let (ux, uy) = (x - m[4], y - m[5]);
    (m[0] * ux + m[1] * uy, m[2] * ux + m[3] * uy)
}

fn to_user(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Een punt in PDF-gebruikersruimte, omgerekend naar de pagina zoals getoond
/// (oorsprong linksonder, rekening houdend met rotatie en bijsnijden).
pub fn display_point(doc: &Document, pid: ObjectId, x: f32, y: f32) -> (f32, f32) {
    to_display(display_space(doc, pid).2, x, y)
}

fn bbox(a: (f32, f32), b: (f32, f32)) -> [f32; 4] {
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

// ---------------------------------------------------------------------------
// Paginabewerkingen
// ---------------------------------------------------------------------------

pub fn rotate_page(doc: &mut Document, index: usize, delta: i64) -> Result<()> {
    let pid = page_id(doc, index)?;
    let new = (rotation(doc, pid) + delta).rem_euclid(360);
    doc.get_dictionary_mut(pid)?.set("Rotate", new);
    Ok(())
}

pub fn delete_pages(doc: &mut Document, indices: &[usize]) -> Result<()> {
    let order: Vec<ObjectId> = page_ids(doc)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !indices.contains(i))
        .map(|(_, id)| id)
        .collect();
    if order.is_empty() {
        bail!(t("err.delete_all"));
    }
    set_page_order(doc, &order)
}

pub fn move_page(doc: &mut Document, from: usize, to: usize) -> Result<()> {
    let mut order = page_ids(doc);
    if from >= order.len() || to >= order.len() {
        bail!(t("err.out_of_range"));
    }
    let id = order.remove(from);
    order.insert(to, id);
    set_page_order(doc, &order)
}

/// Voegt een lege pagina in op positie `at`, met het formaat van een buurpagina.
pub fn insert_blank_page(doc: &mut Document, at: usize) -> Result<()> {
    let mut order = page_ids(doc);
    let at = at.min(order.len());
    let neighbour = order[at.saturating_sub(1).min(order.len() - 1)];
    let [x0, y0, x1, y1] = visible_box(doc, neighbour);
    let rotate = rotation(doc, neighbour);
    let contents = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let pid = doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![x0.into(), y0.into(), x1.into(), y1.into()],
        "Rotate" => rotate,
        "Resources" => dictionary! {},
        "Contents" => contents,
    });
    order.insert(at, pid);
    set_page_order(doc, &order)
}

fn deref_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(obj).ok()?.1.as_dict().ok()
}

/// Zoekt een benoemde bestemming op in /Dests (namen) of de naamboom /Names /Dests (strings).
fn named_destination(doc: &Document, name: &[u8]) -> Option<Object> {
    let catalog = doc.catalog().ok()?;
    let unwrap = |value: &Object| -> Option<Object> {
        match doc.dereference(value).ok()?.1 {
            Object::Array(a) => Some(Object::Array(a.clone())),
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o.clone()),
            _ => None,
        }
    };
    if let Some(dests) = catalog.get(b"Dests").ok().and_then(|o| deref_dict(doc, o)) {
        if let Ok(value) = dests.get(name) {
            return unwrap(value);
        }
    }
    let root = catalog.get(b"Names").ok().and_then(|o| deref_dict(doc, o))?.get(b"Dests").ok()?;
    let mut stack = vec![(deref_dict(doc, root)?, 0)];
    while let Some((node, depth)) = stack.pop() {
        if let Some(Object::Array(pairs)) = node.get(b"Names").ok().and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o) {
            for pair in pairs.chunks(2) {
                if pair.len() == 2 && pair[0].as_str().ok() == Some(name) {
                    return unwrap(&pair[1]);
                }
            }
        }
        if depth < 32 {
            if let Some(Object::Array(kids)) = node.get(b"Kids").ok().and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o) {
                stack.extend(kids.iter().filter_map(|k| deref_dict(doc, k)).map(|d| (d, depth + 1)));
            }
        }
    }
    None
}

/// Maakt benoemde bestemmingen in de links van `doc` expliciet, zodat ze blijven
/// werken nadat de catalogus van dit document is weggevallen.
fn resolve_named_destinations(doc: &mut Document) {
    let name_of = |o: &Object| match o {
        Object::Name(n) => Some(n.clone()),
        Object::String(s, _) => Some(s.clone()),
        _ => None,
    };
    let mut updates: Vec<(ObjectId, bool, Object)> = Vec::new();
    for pid in page_ids(doc) {
        for id in annot_refs(doc, pid) {
            let Ok(dict) = doc.get_dictionary(id) else { continue };
            if let Some(dest) = dict.get(b"Dest").ok().and_then(name_of).and_then(|n| named_destination(doc, &n)) {
                updates.push((id, false, dest));
            }
            let action = dict.get(b"A").ok().and_then(|o| deref_dict(doc, o));
            if let Some(dest) =
                action.and_then(|a| a.get(b"D").ok()).and_then(name_of).and_then(|n| named_destination(doc, &n))
            {
                updates.push((id, true, dest));
            }
        }
    }
    for (id, in_action, dest) in updates {
        let Ok(dict) = doc.get_dictionary_mut(id) else { continue };
        if in_action {
            dict.remove(b"A");
        }
        dict.set("Dest", dest);
    }
}

/// Voegt de formuliervelden van een ingevoegd document toe aan het formulier van `doc`.
fn merge_acroform(doc: &mut Document, other_form: Dictionary) -> Result<()> {
    let Ok(Object::Array(fields)) = other_form.get(b"Fields").cloned() else { return Ok(()) };
    if fields.is_empty() {
        return Ok(());
    }
    let root = doc.trailer.get(b"Root")?.as_reference()?;
    let form_id = match doc.get_dictionary(root)?.get(b"AcroForm").ok().cloned() {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => id,
        Some(Object::Dictionary(dict)) => doc.add_object(dict),
        _ => doc.add_object(Dictionary::new()),
    };
    doc.get_dictionary_mut(root)?.set("AcroForm", form_id);
    let form = doc.get_dictionary_mut(form_id)?;
    let mut all = match form.get(b"Fields") {
        Ok(Object::Array(existing)) => existing.clone(),
        _ => Vec::new(),
    };
    all.extend(fields);
    form.set("Fields", all);
    for key in ["DA", "DR", "Q"] {
        if !form.has(key.as_bytes()) {
            if let Ok(value) = other_form.get(key.as_bytes()) {
                form.set(key, value.clone());
            }
        }
    }
    Ok(())
}

/// Voegt alle pagina's van een andere PDF in op positie `at`. Geeft het aantal terug.
///
/// Van de catalogus van het ingevoegde document blijven de formuliervelden en
/// de bestemmingen van links behouden; bladwijzers en overige catalogusgegevens niet.
pub fn insert_pdf(doc: &mut Document, other_bytes: &[u8], at: usize) -> Result<usize> {
    let mut other = load(other_bytes)?;
    resolve_named_destinations(&mut other);
    other.renumber_objects_with(doc.max_id + 1);
    let other_pages = page_ids(&other);
    let other_root = pages_root(&other)?;
    let other_catalog = other.trailer.get(b"Root")?.as_reference()?;
    let other_form = other
        .get_dictionary(other_catalog)
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| deref_dict(&other, o))
        .cloned();
    doc.max_id = doc.max_id.max(other.max_id);
    for (id, obj) in std::mem::take(&mut other.objects) {
        if id != other_root && id != other_catalog {
            doc.objects.insert(id, obj);
        }
    }
    let mut order = page_ids(doc);
    let at = at.min(order.len());
    let count = other_pages.len();
    order.splice(at..at, other_pages);
    set_page_order(doc, &order)?;
    if let Some(form) = other_form {
        merge_acroform(doc, form)?;
    }
    Ok(count)
}

/// Maakt een nieuwe PDF met alleen de gegeven pagina's (0-gebaseerd, in die volgorde).
pub fn extract_pages(doc: &Document, indices: &[usize]) -> Result<Vec<u8>> {
    let mut copy = doc.clone();
    let all = page_ids(&copy);
    let order: Vec<ObjectId> = indices.iter().filter_map(|&i| all.get(i).copied()).collect();
    set_page_order(&mut copy, &order)?;
    to_bytes_for_saving(&copy)
}

// ---------------------------------------------------------------------------
// Tekst
// ---------------------------------------------------------------------------

fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in s.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// Unicode-teken voor een byte in PDFDocEncoding (wijkt in 0x18..0x1F en 0x80..0xA0 af van Latin-1).
fn pdf_doc_char(byte: u8) -> char {
    const LOW: [u32; 8] = [0x02D8, 0x02C7, 0x02C6, 0x02D9, 0x02DD, 0x02DB, 0x02DA, 0x02DC];
    const HIGH: [u32; 33] = [
        0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212, 0x2030, 0x201E,
        0x201C, 0x201D, 0x2018, 0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141, 0x0152, 0x0160, 0x0178, 0x017D,
        0x0131, 0x0142, 0x0153, 0x0161, 0x017E, 0xFFFD, 0x20AC,
    ];
    let code = match byte {
        0x18..=0x1F => LOW[(byte - 0x18) as usize],
        0x80..=0xA0 => HIGH[(byte - 0x80) as usize],
        _ => byte as u32,
    };
    char::from_u32(code).unwrap_or('\u{FFFD}')
}

fn decode_text_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        bytes.iter().map(|&b| pdf_doc_char(b)).collect()
    }
}

/// Code in WinAnsiEncoding voor `ch`, als dat teken daarin voorkomt.
fn win_ansi(ch: char) -> Option<u8> {
    let code = ch as u32;
    Some(match code {
        0x20..=0x7E | 0xA0..=0xFF => code as u8,
        0x20AC => 0x80,
        0x201A => 0x82,
        0x0192 => 0x83,
        0x201E => 0x84,
        0x2026 => 0x85,
        0x2020 => 0x86,
        0x2021 => 0x87,
        0x02C6 => 0x88,
        0x2030 => 0x89,
        0x0160 => 0x8A,
        0x2039 => 0x8B,
        0x0152 => 0x8C,
        0x017D => 0x8E,
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201C => 0x93,
        0x201D => 0x94,
        0x2022 => 0x95,
        0x2013 => 0x96,
        0x2014 => 0x97,
        0x02DC => 0x98,
        0x2122 => 0x99,
        0x0161 => 0x9A,
        0x203A => 0x9B,
        0x0153 => 0x9C,
        0x017E => 0x9E,
        0x0178 => 0x9F,
        _ => return None,
    })
}

/// Zet tekst om naar een geëscapete WinAnsi-string voor in een content stream.
///
/// Tekens die niet in WinAnsi bestaan geven een fout: de standaardlettertypen
/// kunnen ze niet weergeven, en stilletjes vervangen zou de tekst verminken.
pub(crate) fn content_string(s: &str) -> Result<Vec<u8>> {
    let mut out = vec![b'('];
    let mut unsupported = String::new();
    for ch in s.chars() {
        let ch = if ch.is_whitespace() { ' ' } else { ch };
        match win_ansi(ch) {
            Some(byte) => {
                if matches!(byte, b'(' | b')' | b'\\') {
                    out.push(b'\\');
                }
                out.push(byte);
            }
            None if ch.is_control() => {}
            None => {
                if !unsupported.contains(ch) {
                    unsupported.push(ch);
                }
            }
        }
    }
    if !unsupported.is_empty() {
        bail!("{}", tf("err.unsupported_chars", &[("chars", &unsupported)]));
    }
    out.push(b')');
    Ok(out)
}

/// Ruwe schatting van de tekstbreedte in Helvetica (eenheden: punten).
pub(crate) fn text_width(s: &str, size: f32, bold: bool) -> f32 {
    let em: f32 = s
        .chars()
        .map(|c| match c {
            'i' | 'j' | 'l' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' | 'I' => 0.26,
            'f' | 't' | 'r' | ' ' | '(' | ')' | '-' | '/' => 0.31,
            'm' | 'w' | 'M' | 'W' | '@' => 0.85,
            c if c.is_uppercase() => 0.69,
            c if c.is_ascii_digit() => 0.556,
            _ => 0.53,
        })
        .sum();
    em * size * if bold { 1.06 } else { 1.0 }
}

fn wrap_text(text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if text_width(&candidate, size, false) > max_width && !line.is_empty() {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn page_text(doc: &Document, index: usize) -> String {
    doc.extract_text(&[index as u32 + 1]).unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub page: usize,
    pub snippet: String,
}

/// Vouwt een teken naar kleine letters; de slot-sigma telt als gewone sigma.
fn fold(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase().map(|l| if l == '\u{03C2}' { '\u{03C3}' } else { l })
}

/// Hoofdletterongevoelig zoeken; `texts` bevat de tekst per pagina.
pub fn search(texts: &[String], query: &str) -> Vec<SearchHit> {
    let needle: Vec<char> = query.trim().chars().flat_map(fold).collect();
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    for (page, text) in texts.iter().enumerate() {
        let chars: Vec<char> = text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
        // Gevouwen tekst met per teken de index van het oorspronkelijke teken:
        // vouwen kan één teken in meerdere veranderen.
        let folded: Vec<(char, usize)> =
            chars.iter().enumerate().flat_map(|(i, &c)| fold(c).map(move |l| (l, i))).collect();
        let mut i = 0;
        while i + needle.len() <= folded.len() {
            if folded[i..i + needle.len()].iter().map(|(c, _)| *c).eq(needle.iter().copied()) {
                let first = folded[i].1;
                let last = folded[i + needle.len() - 1].1;
                let start = first.saturating_sub(30);
                let end = (last + 41).min(chars.len());
                let snippet: String = chars[start..end].iter().collect();
                hits.push(SearchHit { page, snippet: format!("…{}…", snippet.trim()) });
                i += needle.len();
                if hits.len() >= 500 {
                    return hits;
                }
            } else {
                i += 1;
            }
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Annotaties
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotKind {
    Highlight,
    Rectangle,
    Note,
    FreeText,
    Ink,
}

#[derive(Clone, Debug)]
pub struct NewAnnot {
    pub kind: AnnotKind,
    /// (x0, y0, x1, y1) in PDF-gebruikersruimte.
    pub rect: [f32; 4],
    pub color: [f32; 3],
    pub text: String,
    /// Voor `Ink`: punten in PDF-gebruikersruimte.
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug)]
pub struct AnnotInfo {
    pub id: ObjectId,
    pub page: usize,
    pub kind: String,
    pub contents: String,
    /// (x0, y0, x1, y1) in PDF-gebruikersruimte.
    pub rect: [f32; 4],
    /// Een via [`add_image`] geplaatste afbeelding: te verplaatsen en te schalen.
    pub is_image: bool,
}

fn helvetica(bold: bool) -> Dictionary {
    dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => if bold { "Helvetica-Bold" } else { "Helvetica" },
        "Encoding" => "WinAnsiEncoding",
    }
}

fn now_pdf_date() -> Object {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Burgerlijke datum uit Unix-tijd (algoritme van Howard Hinnant).
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    Object::string_literal(format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

pub fn add_annotation(doc: &mut Document, page_index: usize, a: &NewAnnot) -> Result<ObjectId> {
    let pid = page_id(doc, page_index)?;
    let [r, g, b] = a.color;
    let mut rect = [
        a.rect[0].min(a.rect[2]),
        a.rect[1].min(a.rect[3]),
        a.rect[0].max(a.rect[2]),
        a.rect[1].max(a.rect[3]),
    ];
    let mut resources = Dictionary::new();
    let mut extra = Dictionary::new();
    let content: String;
    let subtype;
    // Notities en tekstvakken worden opgemaakt in weergaveruimte, zodat ze ook op
    // een gedraaide pagina rechtop staan en vanaf de klik naar rechtsonder lopen.
    let (dw, dh, m) = display_space(doc, pid);
    let mut upright: Option<(f32, f32)> = None;

    match a.kind {
        AnnotKind::Highlight => {
            subtype = "Highlight";
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
            resources.set(
                "ExtGState",
                dictionary! { "GS0" => dictionary! { "Type" => "ExtGState", "ca" => 0.4, "CA" => 0.4 } },
            );
            content = format!("/GS0 gs {r:.3} {g:.3} {b:.3} rg 0 0 {w:.2} {h:.2} re f");
            extra.set(
                "QuadPoints",
                vec![
                    rect[0].into(), rect[3].into(), rect[2].into(), rect[3].into(),
                    rect[0].into(), rect[1].into(), rect[2].into(), rect[1].into(),
                ],
            );
        }
        AnnotKind::Rectangle => {
            subtype = "Square";
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
            content = format!(
                "{r:.3} {g:.3} {b:.3} RG 2 w 1 1 {:.2} {:.2} re S",
                (w - 2.0).max(0.0),
                (h - 2.0).max(0.0)
            );
            extra.set("BS", dictionary! { "W" => 2 });
        }
        AnnotKind::Note => {
            subtype = "Text";
            // Icoon van 22x22 pt met de klikpositie als linkerbovenhoek, binnen de pagina.
            let size = 22.0;
            let (px, py) = to_display(m, a.rect[0], a.rect[1]);
            let left = px.clamp(0.0, (dw - size).max(0.0));
            let top = py.clamp(size.min(dh), dh);
            rect = bbox(to_user(m, left, top - size), to_user(m, left + size, top));
            upright = Some((size, size));
            content = format!(
                "{r:.3} {g:.3} {b:.3} rg 0.25 0.2 0.05 RG 1 w 0.5 0.5 21 21 re B \
                 4.5 15.5 m 17.5 15.5 l 4.5 11 m 17.5 11 l 4.5 6.5 m 13 6.5 l S"
            );
            extra.set("Name", "Comment");
            extra.set("Open", false);
        }
        AnnotKind::FreeText => {
            subtype = "FreeText";
            let size = 12.0;
            let leading = 15.0;
            let pad = 4.0;
            let d = bbox(to_display(m, rect[0], rect[1]), to_display(m, rect[2], rect[3]));
            let mut w = d[2] - d[0];
            if w < 40.0 {
                w = 200.0;
            }
            w = w.min(dw.max(40.0));
            let lines = wrap_text(&a.text, size, w - 2.0 * pad);
            let h = (d[3] - d[1]).max(lines.len() as f32 * leading + 2.0 * pad);
            let left = d[0].clamp(0.0, (dw - w).max(0.0));
            let top = d[3].clamp(h.min(dh), dh);
            rect = bbox(to_user(m, left, top - h), to_user(m, left + w, top));
            upright = Some((w, h));
            resources.set("Font", dictionary! { "Helv" => helvetica(false) });
            let mut c = format!(
                "1 1 1 rg {r:.3} {g:.3} {b:.3} RG 0.75 w 0.4 0.4 {:.2} {:.2} re B \
                 BT /Helv {size} Tf 0 0 0 rg {leading} TL {pad} {:.2} Td ",
                w - 0.8,
                h - 0.8,
                h - pad - size
            )
            .into_bytes();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    c.extend_from_slice(b"T* ");
                }
                c.extend(content_string(line)?);
                c.extend_from_slice(b" Tj ");
            }
            c.extend_from_slice(b"ET");
            // WinAnsi-bytes zijn niet per se geldige UTF-8; bewaar ze byte-voor-byte.
            content = c.iter().map(|&b| b as char).collect();
            extra.set("DA", Object::string_literal("/Helv 12 Tf 0 g"));
        }
        AnnotKind::Ink => {
            subtype = "Ink";
            if a.points.len() < 2 {
                bail!(t("err.ink_points"));
            }
            let width = 2.0;
            let xs = a.points.iter().map(|p| p.0);
            let ys = a.points.iter().map(|p| p.1);
            rect = [
                xs.clone().fold(f32::MAX, f32::min) - width,
                ys.clone().fold(f32::MAX, f32::min) - width,
                xs.fold(f32::MIN, f32::max) + width,
                ys.fold(f32::MIN, f32::max) + width,
            ];
            let mut c = format!("{r:.3} {g:.3} {b:.3} RG {width} w 1 J 1 j ");
            for (i, (x, y)) in a.points.iter().enumerate() {
                let op = if i == 0 { 'm' } else { 'l' };
                c.push_str(&format!("{:.2} {:.2} {op} ", x - rect[0], y - rect[1]));
            }
            c.push('S');
            content = c;
            let ink: Vec<Object> = a.points.iter().flat_map(|&(x, y)| [x.into(), y.into()]).collect();
            extra.set("InkList", vec![Object::Array(ink)]);
            extra.set("BS", dictionary! { "W" => 2 });
        }
    }

    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    if w < 1.0 || h < 1.0 {
        bail!(t("err.too_small"));
    }
    // `content` kan Latin-1-tekens bevatten die als losse bytes de stream in moeten.
    let content_bytes: Vec<u8> = content.chars().map(|c| c as u32 as u8).collect();
    let (form_w, form_h) = upright.unwrap_or((w, h));
    let mut form = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => vec![0.into(), 0.into(), form_w.into(), form_h.into()],
        "Resources" => resources,
    };
    if upright.is_some() && rotation(doc, pid) != 0 {
        // Alleen de rotatie: de viewer past de gedraaide box daarna in /Rect.
        form.set("Matrix", vec![m[0].into(), m[1].into(), m[2].into(), m[3].into(), 0.into(), 0.into()]);
    }
    let ap = doc.add_object(Stream::new(form, content_bytes));
    let mut annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => subtype,
        "Rect" => vec![rect[0].into(), rect[1].into(), rect[2].into(), rect[3].into()],
        "C" => vec![r.into(), g.into(), b.into()],
        "F" => 4,
        "P" => pid,
        "M" => now_pdf_date(),
        "T" => Object::string_literal(APP_NAME),
        "AP" => dictionary! { "N" => ap },
    };
    if !a.text.is_empty() {
        annot.set("Contents", text_string(&a.text));
    }
    for (k, v) in extra.iter() {
        annot.set(k.clone(), v.clone());
    }
    let annot_id = doc.add_object(annot);

    // Aan /Annots hangen (direct array, verwijzing naar array, of nog niet aanwezig).
    let existing = doc.get_dictionary(pid)?.get(b"Annots").ok().cloned();
    match existing {
        Some(Object::Reference(arr_id)) if doc.get_object(arr_id).map(|o| o.as_array().is_ok()).unwrap_or(false) => {
            doc.get_object_mut(arr_id)?.as_array_mut()?.push(annot_id.into());
        }
        Some(Object::Array(mut arr)) => {
            arr.push(annot_id.into());
            doc.get_dictionary_mut(pid)?.set("Annots", arr);
        }
        _ => doc.get_dictionary_mut(pid)?.set("Annots", vec![Object::Reference(annot_id)]),
    }
    Ok(annot_id)
}

/// Hangt een annotatie aan /Annots van de pagina (directe array, verwijzing, of nog niet aanwezig).
fn attach_annotation(doc: &mut Document, pid: ObjectId, annot_id: ObjectId) -> Result<()> {
    match doc.get_dictionary(pid)?.get(b"Annots").ok().cloned() {
        Some(Object::Reference(arr_id)) if doc.get_object(arr_id).map(|o| o.as_array().is_ok()).unwrap_or(false) => {
            doc.get_object_mut(arr_id)?.as_array_mut()?.push(annot_id.into());
        }
        Some(Object::Array(mut arr)) => {
            arr.push(annot_id.into());
            doc.get_dictionary_mut(pid)?.set("Annots", arr);
        }
        _ => doc.get_dictionary_mut(pid)?.set("Annots", vec![Object::Reference(annot_id)]),
    }
    Ok(())
}

/// Grootste zijde (in pixels) waarmee een afbeelding in de PDF wordt opgenomen.
const MAX_IMAGE_SIDE: u32 = 2400;

/// Plaatst een afbeelding (PNG of JPEG) op de pagina. `rect` is het gesleepte
/// vlak in PDF-gebruikersruimte; de afbeelding past daarbinnen met behoud van
/// verhoudingen, rechtop zoals de pagina wordt getoond. Een klik (vlak zonder
/// afmetingen) geeft een standaardbreedte.
///
/// De afbeelding wordt een stempel-annotatie, zodat hij in de lijst met
/// opmerkingen staat en weer te verwijderen is.
pub fn add_image(doc: &mut Document, page_index: usize, rect: [f32; 4], file: &[u8]) -> Result<ObjectId> {
    let pid = page_id(doc, page_index)?;
    let decoded = image::load_from_memory(file).map_err(|_| anyhow!(t("err.image")))?;
    let decoded = if decoded.width().max(decoded.height()) > MAX_IMAGE_SIDE {
        decoded.resize(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let rgba = decoded.to_rgba8();
    let (pw, ph) = (rgba.width(), rgba.height());
    if pw == 0 || ph == 0 {
        bail!(t("err.image"));
    }
    let mut rgb = Vec::with_capacity((pw * ph * 3) as usize);
    let mut alpha = Vec::with_capacity((pw * ph) as usize);
    for pixel in rgba.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
    }

    // Opmaak in weergaveruimte, net als bij een tekstvak.
    let (dw, dh, m) = display_space(doc, pid);
    let d = bbox(to_display(m, rect[0], rect[1]), to_display(m, rect[2], rect[3]));
    let aspect = pw as f32 / ph as f32;
    let (box_w, box_h) = (d[2] - d[0], d[3] - d[1]);
    let (mut w, mut h) = if box_w < 8.0 || box_h < 8.0 {
        (200.0, 200.0 / aspect)
    } else {
        let scale = (box_w / pw as f32).min(box_h / ph as f32);
        (pw as f32 * scale, ph as f32 * scale)
    };
    // Niet groter dan de pagina.
    let shrink = (dw / w).min(dh / h).min(1.0);
    w *= shrink;
    h *= shrink;
    let left = d[0].clamp(0.0, (dw - w).max(0.0));
    let top = d[3].clamp(h.min(dh), dh);
    let rect = bbox(to_user(m, left, top - h), to_user(m, left + w, top));

    let image_dict = |color_space: &str| {
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => pw as i64, "Height" => ph as i64,
            "ColorSpace" => color_space, "BitsPerComponent" => 8,
        }
    };
    let mut image = Stream::new(image_dict("DeviceRGB"), rgb);
    if alpha.iter().any(|&a| a < 255) {
        let mut mask = Stream::new(image_dict("DeviceGray"), alpha);
        let _ = mask.compress();
        image.dict.set("SMask", doc.add_object(mask));
    }
    let _ = image.compress();
    let image_id = doc.add_object(image);

    let mut form = dictionary! {
        "Type" => "XObject", "Subtype" => "Form",
        "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    };
    if rotation(doc, pid) != 0 {
        form.set("Matrix", vec![m[0].into(), m[1].into(), m[2].into(), m[3].into(), 0.into(), 0.into()]);
    }
    let ap = doc.add_object(Stream::new(form, format!("q {w:.3} 0 0 {h:.3} 0 0 cm /Im0 Do Q").into_bytes()));
    let annot_id = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Stamp", "Name" => "Image",
        "Rect" => vec![rect[0].into(), rect[1].into(), rect[2].into(), rect[3].into()],
        "F" => 4, "P" => pid, "M" => now_pdf_date(), "T" => Object::string_literal(APP_NAME),
        "AP" => dictionary! { "N" => ap },
    });
    attach_annotation(doc, pid, annot_id)?;
    Ok(annot_id)
}

fn is_image_annotation(dict: &Dictionary) -> bool {
    let name = |key: &[u8]| dict.get(key).ok().and_then(|n| n.as_name().ok());
    name(b"Subtype") == Some(b"Stamp".as_slice()) && name(b"Name") == Some(b"Image".as_slice())
}

/// Verplaatst of schaalt een geplaatste afbeelding door zijn kader te wijzigen.
/// Een viewer past de weergave vanzelf in het nieuwe kader.
pub fn set_image_rect(doc: &mut Document, page_index: usize, annot: ObjectId, rect: [f32; 4]) -> Result<()> {
    let pid = page_id(doc, page_index)?;
    let known = annot_refs(doc, pid).contains(&annot) && doc.get_dictionary(annot).is_ok_and(is_image_annotation);
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    if !known || !rect.iter().all(|v| v.is_finite()) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        bail!(t("err.too_small"));
    }
    let dict = doc.get_dictionary_mut(annot)?;
    dict.set("Rect", vec![rect[0].into(), rect[1].into(), rect[2].into(), rect[3].into()]);
    dict.set("M", now_pdf_date());
    Ok(())
}

fn annot_refs(doc: &Document, pid: ObjectId) -> Vec<ObjectId> {
    let Ok(page) = doc.get_dictionary(pid) else { return vec![] };
    let Ok(obj) = page.get(b"Annots") else { return vec![] };
    match doc.dereference(obj) {
        Ok((_, Object::Array(arr))) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => vec![],
    }
}

pub fn list_annotations(doc: &Document) -> Vec<AnnotInfo> {
    let mut out = Vec::new();
    for (page, pid) in page_ids(doc).into_iter().enumerate() {
        for id in annot_refs(doc, pid) {
            let Ok(dict) = doc.get_dictionary(id) else { continue };
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            let kind = match subtype.as_str() {
                "Link" | "Popup" | "Widget" | "" => continue,
                "Highlight" => t("annot.highlight"),
                "Square" => t("annot.square"),
                "Text" => t("annot.note"),
                "FreeText" => t("annot.freetext"),
                "Ink" => t("annot.ink"),
                "Underline" => t("annot.underline"),
                "StrikeOut" => t("annot.strikeout"),
                "Stamp" if dict.get(b"Name").ok().and_then(|n| n.as_name().ok()) == Some(b"Image".as_slice()) => {
                    t("annot.image")
                }
                "Stamp" => t("annot.stamp"),
                other => other,
            }
            .to_string();
            let contents = dict
                .get(b"Contents")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_str().ok())
                .map(decode_text_string)
                .unwrap_or_default();
            let corners: Vec<f32> = match dict.get(b"Rect").ok().and_then(|o| doc.dereference(o).ok()) {
                Some((_, Object::Array(a))) => a.iter().filter_map(|o| number(doc, o)).collect(),
                _ => Vec::new(),
            };
            let rect = match corners[..] {
                [a, b, c, d] => [a.min(c), b.min(d), a.max(c), b.max(d)],
                _ => [0.0; 4],
            };
            let is_image = is_image_annotation(dict);
            out.push(AnnotInfo { id, page, kind, contents, rect, is_image });
        }
    }
    out
}

/// Verwijdert een opmerking, samen met zijn popup en de reacties erop, zodat de
/// tekst niet in het bestand achterblijft. Andere items in /Annots (ook
/// annotaties die direct in de array staan) blijven ongemoeid.
pub fn delete_annotation(doc: &mut Document, page_index: usize, annot: ObjectId) -> Result<()> {
    let pid = page_id(doc, page_index)?;
    let refs = annot_refs(doc, pid);
    let mut doomed: HashSet<ObjectId> = HashSet::from([annot]);
    loop {
        let mut found = Vec::new();
        for &id in &refs {
            let Ok(dict) = doc.get_dictionary(id) else { continue };
            let target = |key: &[u8]| dict.get(key).ok().and_then(|o| o.as_reference().ok());
            if doomed.contains(&id) {
                found.extend(target(b"Popup"));
            } else if [b"Parent".as_slice(), b"IRT"].iter().any(|k| target(k).is_some_and(|t| doomed.contains(&t))) {
                found.push(id);
            }
        }
        let before = doomed.len();
        doomed.extend(found);
        if doomed.len() == before {
            break;
        }
    }
    let keep = |o: &Object| !matches!(o, Object::Reference(id) if doomed.contains(id));
    match doc.get_dictionary(pid)?.get(b"Annots").ok().cloned() {
        Some(Object::Reference(arr_id)) => {
            if let Ok(arr) = doc.get_object_mut(arr_id).and_then(|o| o.as_array_mut()) {
                arr.retain(keep);
            }
        }
        Some(Object::Array(mut arr)) => {
            arr.retain(keep);
            doc.get_dictionary_mut(pid)?.set("Annots", arr);
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Overlays: watermerk en paginanummers
// ---------------------------------------------------------------------------

/// Zorgt dat /Resources van de pagina een eigen object is en geeft het id terug.
fn resources_id(doc: &mut Document, pid: ObjectId) -> Result<ObjectId> {
    match doc.get_dictionary(pid)?.get(b"Resources").ok().cloned() {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => Ok(id),
        Some(Object::Dictionary(dict)) => {
            let id = doc.add_object(dict);
            doc.get_dictionary_mut(pid)?.set("Resources", id);
            Ok(id)
        }
        _ => {
            let id = doc.add_object(Dictionary::new());
            doc.get_dictionary_mut(pid)?.set("Resources", id);
            Ok(id)
        }
    }
}

fn add_resource(doc: &mut Document, res: ObjectId, category: &str, name: &str, value: Object) -> Result<()> {
    let sub = doc.get_dictionary(res)?.get(category.as_bytes()).ok().cloned();
    match sub {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => {
            doc.get_dictionary_mut(id)?.set(name, value);
        }
        Some(Object::Dictionary(mut dict)) => {
            dict.set(name, value);
            doc.get_dictionary_mut(res)?.set(category, dict);
        }
        _ => {
            let mut dict = Dictionary::new();
            dict.set(name, value);
            doc.get_dictionary_mut(res)?.set(category, dict);
        }
    }
    Ok(())
}

/// Tekent `content` (in weergaveruimte, oorsprong linksonder) bovenop de pagina.
pub(crate) fn overlay(
    doc: &mut Document,
    pid: ObjectId,
    content: Vec<u8>,
    font: ObjectId,
    gs: Option<ObjectId>,
    display_space_coords: bool,
) -> Result<()> {
    let res = resources_id(doc, pid)?;
    add_resource(doc, res, "Font", &format!("PkF{}", font.0), font.into())?;
    if let Some(gs) = gs {
        add_resource(doc, res, "ExtGState", &format!("PkGS{}", gs.0), gs.into())?;
    }

    let m = if display_space_coords { display_space(doc, pid).2 } else { [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] };
    let mut stream = format!(
        "Q q {} {} {} {} {:.3} {:.3} cm\n",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
    .into_bytes();
    stream.extend(content);
    stream.extend_from_slice(b"\nQ");

    let old: Vec<Object> = match doc.get_dictionary(pid)?.get(b"Contents").ok().cloned() {
        Some(Object::Array(arr)) => arr,
        Some(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(arr)) => arr.clone(),
            Ok(_) => vec![Object::Reference(id)],
            Err(_) => vec![],
        },
        _ => vec![],
    };
    // De bestaande inhoud tussen q … Q zetten zodat die onze matrix niet beïnvloedt.
    let open = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(dictionary! {}, stream));
    let mut contents = vec![Object::Reference(open)];
    contents.extend(old);
    contents.push(Object::Reference(close));
    doc.get_dictionary_mut(pid)?.set("Contents", contents);
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Watermark {
    pub text: String,
    pub size: f32,
    pub opacity: f32,
    pub color: [f32; 3],
    pub diagonal: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Self { text: t("wm.default").into(), size: 72.0, opacity: 0.25, color: [0.8, 0.1, 0.1], diagonal: true }
    }
}

pub fn add_watermark(doc: &mut Document, wm: &Watermark) -> Result<()> {
    if wm.text.trim().is_empty() {
        bail!(t("err.wm_empty"));
    }
    let text = content_string(&wm.text)?;
    let font = doc.add_object(helvetica(true));
    let gs = doc.add_object(dictionary! {
        "Type" => "ExtGState", "ca" => wm.opacity, "CA" => wm.opacity,
    });
    let tw = text_width(&wm.text, wm.size, true);
    for pid in page_ids(doc) {
        let (w, h, _) = display_space(doc, pid);
        let angle = if wm.diagonal { (h / w).atan() } else { 0.0 };
        let (s, c) = angle.sin_cos();
        let [r, g, b] = wm.color;
        let mut content = format!(
            "/PkGS{} gs {r:.3} {g:.3} {b:.3} rg {c:.5} {s:.5} {:.5} {c:.5} {:.2} {:.2} cm \
             BT /PkF{} {:.1} Tf {:.2} {:.2} Td ",
            gs.0,
            -s,
            w / 2.0,
            h / 2.0,
            font.0,
            wm.size,
            -tw / 2.0,
            -wm.size * 0.35
        )
        .into_bytes();
        content.extend(text.iter());
        content.extend_from_slice(b" Tj ET");
        overlay(doc, pid, content, font, Some(gs), true)?;
    }
    Ok(())
}

/// Voegt onderaan elke pagina een paginanummer toe. In `format` worden `{n}`
/// en `{N}` vervangen door het paginanummer en het totaal.
pub fn add_page_numbers(doc: &mut Document, format: &str) -> Result<()> {
    content_string(format)?;
    let font = doc.add_object(helvetica(false));
    let gs = doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => 1, "CA" => 1 });
    let pages = page_ids(doc);
    let total = pages.len();
    let size = 10.0;
    for (i, pid) in pages.into_iter().enumerate() {
        let label = format
            .replace("{n}", &(i + 1).to_string())
            .replace("{N}", &total.to_string());
        let (w, _, _) = display_space(doc, pid);
        let tw = text_width(&label, size, false);
        let mut content =
            format!("/PkGS{} gs 0.25 0.25 0.25 rg BT /PkF{} {size} Tf {:.2} 24 Td ", gs.0, font.0, (w - tw) / 2.0).into_bytes();
        content.extend(content_string(&label)?);
        content.extend_from_slice(b" Tj ET");
        overlay(doc, pid, content, font, Some(gs), true)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
}

const META_KEYS: [&str; 6] = ["Title", "Author", "Subject", "Keywords", "Creator", "Producer"];

impl Meta {
    fn fields(&mut self) -> [&mut String; 6] {
        [
            &mut self.title, &mut self.author, &mut self.subject,
            &mut self.keywords, &mut self.creator, &mut self.producer,
        ]
    }
}

fn info_dict(doc: &Document) -> Option<&Dictionary> {
    match doc.dereference(doc.trailer.get(b"Info").ok()?).ok()?.1 {
        Object::Dictionary(d) => Some(d),
        _ => None,
    }
}

pub fn get_meta(doc: &Document) -> Meta {
    let mut meta = Meta::default();
    if let Some(info) = info_dict(doc) {
        for (key, field) in META_KEYS.iter().zip(meta.fields()) {
            if let Some(bytes) = info
                .get(key.as_bytes())
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_str().ok())
            {
                *field = decode_text_string(bytes);
            }
        }
    }
    meta
}

fn store_info(doc: &mut Document, info: Dictionary) {
    match doc.trailer.get(b"Info").ok().cloned() {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => {
            doc.objects.insert(id, Object::Dictionary(info));
        }
        _ => {
            let id = doc.add_object(info);
            doc.trailer.set("Info", id);
        }
    }
}

/// Schrijft alleen de velden die echt veranderd zijn; ongewijzigde velden
/// houden hun oorspronkelijke bytes.
pub fn set_meta(doc: &mut Document, meta: &Meta) -> Result<()> {
    let mut current = get_meta(doc);
    let mut wanted = meta.clone();
    let mut info = info_dict(doc).cloned().unwrap_or_default();
    let mut changed = false;
    for ((key, new), old) in META_KEYS.iter().zip(wanted.fields()).zip(current.fields()) {
        if new == old {
            continue;
        }
        changed = true;
        if new.is_empty() {
            info.remove(key.as_bytes());
        } else {
            info.set(*key, text_string(new));
        }
    }
    if !changed {
        return Ok(());
    }
    info.set("ModDate", now_pdf_date());
    store_info(doc, info);
    // De XMP-metadata in de catalogus zou de oude waarden blijven noemen; liever
    // geen XMP dan tegenstrijdige metadata.
    if let Ok(root) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        if let Ok(catalog) = doc.get_dictionary_mut(root) {
            catalog.remove(b"Metadata");
        }
    }
    Ok(())
}
