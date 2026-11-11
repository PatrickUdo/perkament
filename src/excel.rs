//! Exporteren van de tekst van een PDF naar een Excel-werkmap.
//!
//! Een PDF kent geen tabellen, alleen tekst op posities. Hier wordt de tabel
//! teruggevonden uit die posities: tekst op dezelfde basislijn vormt een rij,
//! en de witte stroken die over de hele pagina doorlopen scheiden de kolommen.
//! Elke pagina wordt een eigen werkblad.

use crate::i18n::{t, tf};
use crate::pdfops::page_count;
use crate::textedit::text_lines;
use anyhow::{bail, Context, Result};
use lopdf::Document;
use rust_xlsxwriter::Workbook;
use std::path::Path;

/// Een stuk tekst met zijn plek op de pagina zoals getoond (y loopt omhoog).
#[derive(Clone, Debug)]
pub struct Item {
    pub left: f32,
    pub right: f32,
    pub baseline: f32,
    pub height: f32,
    pub text: String,
}

/// Leesrichting van een regel in kwartslagen (0 = naar rechts, 1 = omhoog, enzovoort).
fn quarter_turns(quad: &[(f32, f32); 4]) -> usize {
    let (dx, dy) = (quad[1].0 - quad[0].0, quad[1].1 - quad[0].1);
    ((dy.atan2(dx) / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4) as usize
}

fn items(doc: &Document, page_index: usize) -> Vec<Item> {
    let lines = text_lines(doc, page_index).unwrap_or_default();
    // De tabel volgt de leesrichting van de meeste tekst, niet de stand van de pagina:
    // zo komt ook een liggend gescande of gedraaide pagina goed uit.
    let mut weight = [0usize; 4];
    for line in &lines {
        weight[quarter_turns(&line.quad)] += line.text.chars().count();
    }
    let turns = (0..4).max_by_key(|&k| weight[k]).unwrap_or(0);
    let upright = |(x, y): (f32, f32)| match turns {
        1 => (y, -x),
        2 => (-x, -y),
        3 => (-y, x),
        _ => (x, y),
    };
    lines
        .into_iter()
        .map(|line| {
            let corners = line.quad.map(upright);
            let xs = corners.map(|c| c.0);
            let ys = corners.map(|c| c.1);
            let (low, high) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
            Item {
                left: xs.iter().copied().fold(f32::MAX, f32::min),
                right: xs.iter().copied().fold(f32::MIN, f32::max),
                baseline: low,
                height: (high - low).max(1.0),
                text: line.text,
            }
        })
        .filter(|item| item.left.is_finite() && item.baseline.is_finite())
        .collect()
}

/// Zet losse tekststukken om in rijen en kolommen.
pub fn build_table(mut items: Vec<Item>) -> Vec<Vec<String>> {
    if items.is_empty() {
        return Vec::new();
    }
    // Rijen: van boven naar beneden, stukken op (bijna) dezelfde basislijn horen bij elkaar.
    items.sort_by(|a, b| b.baseline.total_cmp(&a.baseline));
    let mut rows: Vec<Vec<Item>> = Vec::new();
    for item in items {
        match rows.last_mut() {
            Some(row) if (row[0].baseline - item.baseline).abs() < 0.5 * row[0].height.max(item.height) => row.push(item),
            _ => rows.push(vec![item]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.left.total_cmp(&b.left));
    }

    // Kolommen: de aaneengesloten stroken die door tekst bedekt worden. Rijen met maar
    // één stuk (titels, lopende tekst) tellen niet mee: die zouden alle kolommen overbruggen.
    let mut spans: Vec<(f32, f32)> =
        rows.iter().filter(|row| row.len() > 1).flatten().map(|item| (item.left, item.right)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut columns: Vec<(f32, f32)> = Vec::new();
    for (left, right) in spans {
        match columns.last_mut() {
            // Een smalle kier is nog geen kolomgrens.
            Some(column) if left <= column.1 + 2.0 => column.1 = column.1.max(right),
            _ => columns.push((left, right)),
        }
    }
    let column_of = |item: &Item| {
        let middle = (item.left + item.right) / 2.0;
        columns
            .iter()
            .position(|c| item.left >= c.0 - 1.0 && item.left <= c.1)
            .or_else(|| columns.iter().position(|c| middle >= c.0 && middle <= c.1))
            // Buiten elke kolom: de laatste kolom die links van het stuk begint.
            .unwrap_or_else(|| columns.iter().rposition(|c| c.0 <= item.left).unwrap_or(0))
    };

    let width = columns.len().max(1);
    rows.iter()
        .map(|row| {
            let mut cells = vec![String::new(); width];
            for item in row {
                let cell = &mut cells[column_of(item).min(width - 1)];
                if !cell.is_empty() {
                    cell.push(' ');
                }
                cell.push_str(item.text.trim());
            }
            cells
        })
        .collect()
}

pub fn page_table(doc: &Document, page_index: usize) -> Vec<Vec<String>> {
    build_table(items(doc, page_index))
}

/// Leest een getal als de notatie ondubbelzinnig is; anders blijft de cel tekst.
/// "1.234" kan duizend of een decimaal zijn en wordt daarom niet omgezet.
pub fn parse_number(text: &str) -> Option<f64> {
    let s = text.trim();
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
        return None;
    }
    let digits_only = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let grouped = |part: &str, separator: char| {
        let groups: Vec<&str> = part.split(separator).collect();
        groups.len() > 1
            && digits_only(groups[0])
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3 && digits_only(g))
    };
    let normalised = match (body.matches('.').count(), body.matches(',').count()) {
        (0, 0) => body.to_string(),
        // Komma als decimaalteken: 12,5 of 1.234,56
        (_, 1) => {
            let (whole, fraction) = body.split_once(',')?;
            if !digits_only(fraction) || !(digits_only(whole) || grouped(whole, '.')) {
                return None;
            }
            format!("{}.{fraction}", whole.replace('.', ""))
        }
        // Punt als decimaalteken, behalve precies drie decimalen (dat kan ook duizend zijn).
        (1, 0) => {
            let (whole, fraction) = body.split_once('.')?;
            if !digits_only(whole) || !digits_only(fraction) || fraction.len() == 3 {
                return None;
            }
            body.to_string()
        }
        // Meerdere punten: duizendtallen, 1.234.567
        (_, 0) if grouped(body, '.') => body.replace('.', ""),
        _ => return None,
    };
    // Voorloopnullen horen bij codes (postcodes, artikelnummers), niet bij getallen.
    if normalised.len() > 1 && normalised.starts_with('0') && !normalised.starts_with("0.") {
        return None;
    }
    let value: f64 = normalised.parse().ok()?;
    Some(if s.starts_with('-') { -value } else { value })
}

/// Schrijft alle pagina's naar een werkmap. Geeft het aantal gevulde cellen terug.
pub fn export_xlsx(doc: &Document, path: &Path) -> Result<usize> {
    let mut workbook = Workbook::new();
    let mut filled = 0;
    for page in 0..page_count(doc) {
        let sheet = workbook.add_worksheet();
        sheet.set_name(tf("xlsx.sheet", &[("n", &(page + 1).to_string())])).context(t("err.xlsx"))?;
        for (r, row) in page_table(doc, page).iter().enumerate() {
            // Excel kent ruim een miljoen rijen en ruim zestienduizend kolommen.
            if r >= 1_000_000 {
                break;
            }
            for (c, cell) in row.iter().enumerate().take(16_000) {
                if cell.is_empty() {
                    continue;
                }
                filled += 1;
                match parse_number(cell) {
                    Some(number) => sheet.write_number(r as u32, c as u16, number),
                    None => sheet.write_string(r as u32, c as u16, cell),
                }
                .context(t("err.xlsx"))?;
            }
        }
        sheet.autofit();
    }
    if filled == 0 {
        bail!(t("err.no_text"));
    }
    workbook.save(path).context(t("err.xlsx"))?;
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(left: f32, right: f32, baseline: f32, text: &str) -> Item {
        Item { left, right, baseline, height: 12.0, text: text.into() }
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("12"), Some(12.0));
        assert_eq!(parse_number("12,5"), Some(12.5));
        assert_eq!(parse_number("12.5"), Some(12.5));
        assert_eq!(parse_number("-1.234,56"), Some(-1234.56));
        assert_eq!(parse_number("1.234.567"), Some(1234567.0));
        // Dubbelzinnig of geen getal: tekst laten.
        assert_eq!(parse_number("1.234"), None);
        assert_eq!(parse_number("0123"), None);
        assert_eq!(parse_number("12-05-2026"), None);
        assert_eq!(parse_number("K12345"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("0,5"), Some(0.5));
    }

    #[test]
    fn table_from_positions() {
        // Titel over de volle breedte, daaronder drie kolommen met rechts uitgelijnde bedragen.
        let table = build_table(vec![
            item(50.0, 500.0, 700.0, "Offerte voor de klant met een lange titel"),
            item(50.0, 90.0, 650.0, "Artikel"),
            item(200.0, 240.0, 650.0, "Aantal"),
            item(400.0, 440.0, 650.0, "Prijs"),
            item(50.0, 120.0, 630.0, "Machine A"),
            item(225.0, 240.0, 630.0, "2"),
            item(395.0, 440.0, 630.0, "1.250,00"),
            item(50.0, 100.0, 610.0, "Onderdeel"),
            item(218.0, 240.0, 610.0, "10"),
            item(410.0, 440.0, 610.0, "35,50"),
        ]);
        assert_eq!(table.len(), 4);
        assert_eq!(table[0][0], "Offerte voor de klant met een lange titel");
        assert_eq!(table[1], vec!["Artikel", "Aantal", "Prijs"]);
        assert_eq!(table[2], vec!["Machine A", "2", "1.250,00"]);
        assert_eq!(table[3], vec!["Onderdeel", "10", "35,50"]);
        assert!(build_table(vec![]).is_empty());
    }
}
