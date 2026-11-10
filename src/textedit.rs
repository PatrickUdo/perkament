//! Bestaande tekst op een pagina opsporen en vervangen.
//!
//! De content streams van de pagina worden hier zelf ontleed, met van elk
//! token de bytepositie. Vervangen gebeurt door alleen de bytes van de
//! betrokken tekstoperatoren te vervangen: de rest van de pagina blijft
//! byte-voor-byte gelijk. Er wordt dus nooit een hele pagina opnieuw
//! weggeschreven via een parser die inhoud kan kwijtraken.
//!
//! Fragmenten die op dezelfde basislijn op elkaar aansluiten vormen één regel.
//! Bij vervangen komt de nieuwe tekst in het eerste fragment; de overige
//! fragmenten worden leeg maar houden hun breedte, zodat wat erna komt op
//! zijn plaats blijft.

use crate::i18n::t;
use crate::pdfops::{content_string, overlay, page_id, page_ids, text_width};
use anyhow::{anyhow, bail, Result};
use lopdf::{dictionary, Dictionary, Document, Encoding, Object, ObjectId, Stream};
use std::collections::HashMap;

type Matrix = [f32; 6];
type Range = (usize, usize);
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
/// Fragmenten verder uit elkaar dan dit (in em) horen niet bij dezelfde regel.
const MAX_GAP_EM: f32 = 0.8;
const DEFAULT_SPACE: f32 = 280.0;

/// `a × b` in PDF-notatie: eerst `a` toepassen, dan `b`.
fn mul(a: Matrix, b: Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

// ---------------------------------------------------------------------------
// Ontleden van de content stream
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Operand {
    Num(f32),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Operand>),
    Other,
}

#[derive(Debug)]
struct Operation {
    op: Vec<u8>,
    operands: Vec<Operand>,
    /// Van het begin van de eerste operand tot en met de operator.
    range: Range,
    /// Begin van elke operand op het hoogste niveau (voor het knippen van delen).
    starts: Vec<usize>,
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    b"()<>[]{}/%".contains(&b)
}

/// Een operand, of het woord van een operator.
enum Token {
    Operand(Operand),
    Operator(Vec<u8>),
}

struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn skip_blank(&mut self) {
        while let Some(&b) = self.data.get(self.pos) {
            if is_space(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.data.get(self.pos).is_some_and(|&c| c != b'\n' && c != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn regular(&mut self) -> &[u8] {
        let start = self.pos;
        while self.data.get(self.pos).is_some_and(|&b| !is_space(b) && !is_delimiter(b)) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn literal_string(&mut self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut depth = 1;
        self.pos += 1;
        loop {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let e = *self.data.get(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut value = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        value = value * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn hex_string(&mut self) -> Option<Vec<u8>> {
        self.pos += 1;
        let mut digits = Vec::new();
        loop {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            match b {
                b'>' => break,
                _ if is_space(b) => {}
                _ => digits.push((b as char).to_digit(16)? as u8),
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        Some(digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect())
    }

    /// Slaat een inline-afbeelding over; `self.pos` staat direct na `BI`.
    fn inline_image(&mut self) -> Option<()> {
        let mut length = None;
        let mut key: Vec<u8> = Vec::new();
        loop {
            self.skip_blank();
            let start = self.pos;
            match *self.data.get(self.pos)? {
                b'/' => {
                    self.pos += 1;
                    key = self.regular().to_vec();
                }
                b'(' => {
                    self.literal_string()?;
                }
                b'<' if self.data.get(self.pos + 1) == Some(&b'<') => self.pos += 2,
                b'<' => {
                    self.hex_string()?;
                }
                b'>' if self.data.get(self.pos + 1) == Some(&b'>') => self.pos += 2,
                b'[' | b']' => self.pos += 1,
                _ => {
                    let word = self.regular().to_vec();
                    if word == b"ID" {
                        break;
                    }
                    if key == b"L" || key == b"Length" {
                        length = std::str::from_utf8(&word).ok().and_then(|w| w.parse::<usize>().ok());
                    }
                }
            }
            if self.pos == start {
                return None;
            }
        }
        // Na ID volgt precies één witruimteteken, daarna de beelddata.
        if self.data.get(self.pos).is_some_and(|&b| is_space(b)) {
            self.pos += 1;
        }
        let data = self.data;
        let ends_here = |at: usize| {
            data.get(at..at + 2) == Some(b"EI".as_slice())
                && data.get(at + 2).is_none_or(|&b| is_space(b) || is_delimiter(b))
        };
        if let Some(len) = length {
            let mut at = self.pos.checked_add(len)?;
            while data.get(at).is_some_and(|&b| is_space(b)) {
                at += 1;
            }
            if ends_here(at) {
                self.pos = at + 2;
                return Some(());
            }
        }
        let mut at = self.pos;
        while at + 2 <= data.len() {
            if (at == self.pos || is_space(data[at - 1])) && ends_here(at) {
                self.pos = at + 2;
                return Some(());
            }
            at += 1;
        }
        None
    }

    fn number(word: &[u8]) -> Option<f32> {
        let text = std::str::from_utf8(word).ok()?;
        if let Ok(value) = text.parse::<f32>() {
            return value.is_finite().then_some(value);
        }
        // Soepel met vormen als "--5" of "5-", die in het wild voorkomen.
        let negative = text.bytes().filter(|&b| b == b'-').count() % 2 == 1;
        let digits: String = text.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
        let value = digits.parse::<f32>().ok().filter(|v| v.is_finite())?;
        Some(if negative { -value } else { value })
    }

    /// Leest één operand of operator. `Ok(None)` aan het einde, `Err` bij iets onleesbaars.
    fn token(&mut self, depth: usize) -> Result<Option<Token>, ()> {
        self.skip_blank();
        let Some(&b) = self.data.get(self.pos) else { return Ok(None) };
        let operand = match b {
            b'(' => Operand::Str(self.literal_string().ok_or(())?),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                // Woordenboek (bij BDC/DP): inhoud overslaan tot de bijpassende >>.
                loop {
                    self.skip_blank();
                    if self.data.get(self.pos..self.pos + 2) == Some(b">>".as_slice()) {
                        self.pos += 2;
                        break;
                    }
                    if depth > 16 || self.token(depth + 1)?.is_none() {
                        return Err(());
                    }
                }
                Operand::Other
            }
            b'<' => Operand::Str(self.hex_string().ok_or(())?),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_blank();
                    if self.data.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        break;
                    }
                    if depth > 16 {
                        return Err(());
                    }
                    match self.token(depth + 1)? {
                        Some(Token::Operand(item)) => items.push(item),
                        _ => return Err(()),
                    }
                }
                Operand::Array(items)
            }
            b'/' => {
                self.pos += 1;
                Operand::Name(self.regular().to_vec())
            }
            b')' | b'>' | b']' | b'{' | b'}' => return Err(()),
            _ => {
                let word = self.regular().to_vec();
                if word.is_empty() {
                    return Err(());
                }
                if matches!(word[0], b'0'..=b'9' | b'+' | b'-' | b'.') {
                    Operand::Num(Self::number(&word).ok_or(())?)
                } else if word == b"true" || word == b"false" || word == b"null" {
                    Operand::Other
                } else {
                    return Ok(Some(Token::Operator(word)));
                }
            }
        };
        Ok(Some(Token::Operand(operand)))
    }
}

/// Ontleedt zoveel van de inhoud als veilig kan. Bij iets onleesbaars stopt
/// het; wat ervoor staat blijft bruikbaar, want vervangen raakt alleen de
/// bytes van bekende operatoren.
fn parse(data: &[u8]) -> Vec<Operation> {
    let mut lexer = Lexer { data, pos: 0 };
    let mut out = Vec::new();
    let mut operands = Vec::new();
    let mut starts = Vec::new();
    loop {
        lexer.skip_blank();
        let start = lexer.pos;
        match lexer.token(0) {
            Ok(Some(Token::Operand(operand))) => {
                operands.push(operand);
                starts.push(start);
            }
            Ok(Some(Token::Operator(op))) => {
                if op == b"BI" && lexer.inline_image().is_none() {
                    break;
                }
                let begin = starts.first().copied().unwrap_or(start);
                out.push(Operation {
                    op,
                    operands: std::mem::take(&mut operands),
                    range: (begin, lexer.pos),
                    starts: std::mem::take(&mut starts),
                });
            }
            Ok(None) | Err(()) => break,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Lettertypen
// ---------------------------------------------------------------------------

struct Font<'a> {
    name: String,
    encoding: Option<Encoding<'a>>,
    two_byte: bool,
    first_char: i64,
    widths: Vec<f32>,
    cid_widths: HashMap<u32, f32>,
    default_width: Option<f32>,
    bold: bool,
    /// Kunnen de codes betrouwbaar naar tekst worden vertaald?
    reliable: bool,
    /// Alleen tekens gebruiken die al in het document voorkomen (ingesloten of Type 3).
    restricted: bool,
}

fn num(doc: &Document, obj: &Object) -> Option<f32> {
    doc.dereference(obj).ok()?.1.as_float().ok().filter(|v| v.is_finite())
}

fn deref_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(obj).ok()?.1.as_dict().ok()
}

fn name_of<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    doc.dereference(dict.get(key).ok()?).ok()?.1.as_name().ok()
}

fn is_subset(name: &str) -> bool {
    name.len() > 7 && name.as_bytes()[6] == b'+' && name[..6].bytes().all(|b| b.is_ascii_uppercase())
}

fn load_font<'a>(doc: &'a Document, dict: &'a Dictionary) -> Font<'a> {
    let name = name_of(doc, dict, b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
    let subtype = name_of(doc, dict, b"Subtype").unwrap_or_default();
    let has_to_unicode = dict.has(b"ToUnicode");
    let descriptor = dict.get(b"FontDescriptor").ok().and_then(|o| deref_dict(doc, o));
    let embedded =
        descriptor.is_some_and(|fd| [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"].iter().any(|k| fd.has(k)));
    let mut font = Font {
        bold: name.contains("Bold"),
        name,
        encoding: dict.get_font_encoding(doc).ok(),
        two_byte: subtype == b"Type0",
        first_char: dict.get(b"FirstChar").ok().and_then(|o| num(doc, o)).unwrap_or(0.0) as i64,
        widths: Vec::new(),
        cid_widths: HashMap::new(),
        default_width: None,
        reliable: false,
        restricted: false,
    };

    if font.two_byte {
        let identity = matches!(name_of(doc, dict, b"Encoding"), Some(b"Identity-H") | Some(b"Identity-V"));
        // Zonder ToUnicode is de tekst niet te lezen; bij een andere CMap is de codebreedte onbekend.
        font.reliable = identity && has_to_unicode && font.encoding.is_some();
        font.restricted = true;
        let descendant = dict
            .get(b"DescendantFonts")
            .ok()
            .and_then(|o| doc.dereference(o).ok())
            .and_then(|(_, o)| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| deref_dict(doc, o));
        if let Some(desc) = descendant {
            font.default_width = Some(desc.get(b"DW").ok().and_then(|o| num(doc, o)).unwrap_or(1000.0));
            let w = desc.get(b"W").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok());
            let w = w.map(|v| v.as_slice()).unwrap_or_default();
            // Vormen: `c [w1 w2 …]` en `c_eerste c_laatste w`. Begrensd tegen vijandige invoer.
            let in_range = |v: &f32| (0.0..=65535.0).contains(v);
            let mut i = 0;
            while i < w.len() && font.cid_widths.len() < 70_000 {
                let Some(first) = num(doc, &w[i]).filter(in_range) else { break };
                let first = first as u32;
                match w.get(i + 1).and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o) {
                    Some(Object::Array(list)) => {
                        for (k, item) in list.iter().take(65536).enumerate() {
                            if let Some(width) = num(doc, item) {
                                font.cid_widths.insert(first.saturating_add(k as u32), width);
                            }
                        }
                        i += 2;
                    }
                    Some(last) => {
                        let last = last.as_float().ok().filter(in_range);
                        let (Some(last), Some(width)) = (last, w.get(i + 2).and_then(|o| num(doc, o))) else { break };
                        for cid in first..=(last as u32) {
                            font.cid_widths.insert(cid, width);
                        }
                        i += 3;
                    }
                    None => break,
                }
            }
        }
        return font;
    }

    if let Some(widths) =
        dict.get(b"Widths").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok())
    {
        font.widths = widths.iter().map(|o| num(doc, o).unwrap_or(0.0)).collect();
        font.default_width = descriptor.and_then(|d| d.get(b"MissingWidth").ok()).and_then(|o| num(doc, o));
    }
    let type3 = subtype == b"Type3";
    font.restricted = type3 || embedded || is_subset(&font.name);
    let symbolic = font.name.contains("Symbol") || font.name.contains("Dingbats");
    font.reliable = font.encoding.is_some()
        && (has_to_unicode
            || match dict.get(b"Encoding").ok().and_then(|o| doc.dereference(o).ok()).map(|(_, o)| o) {
                Some(Object::Name(_)) => true,
                // lopdf valt bij onleesbare /Differences stilletjes terug op StandardEncoding.
                Some(Object::Dictionary(_)) => matches!(font.encoding, Some(Encoding::Differences(_))),
                // Geen /Encoding: alleen bij niet-ingesloten standaardletters is de codering bekend.
                _ => !type3 && !embedded && !symbolic,
            });
    font
}

impl Font<'_> {
    fn decode(&self, bytes: &[u8]) -> String {
        self.encoding
            .as_ref()
            .and_then(|e| Document::decode_text(e, bytes).ok())
            .unwrap_or_else(|| bytes.iter().map(|&b| b as char).collect())
    }

    fn step(&self) -> usize {
        if self.two_byte { 2 } else { 1 }
    }

    /// Breedte van één code in duizendsten van de lettergrootte.
    fn width(&self, code: &[u8]) -> f32 {
        let value = code.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32);
        if self.two_byte {
            return self.cid_widths.get(&value).copied().or(self.default_width).unwrap_or(1000.0);
        }
        let index = value as i64 - self.first_char;
        if index >= 0 {
            if let Some(&w) = self.widths.get(index as usize) {
                return w;
            }
        }
        if let Some(w) = self.default_width.filter(|_| !self.widths.is_empty()) {
            return w;
        }
        // Standaardlettertype zonder breedtetabel: schatten.
        text_width(&self.decode(code), 1000.0, self.bold)
    }

    /// Verplaatsing in tekstruimte voor een reeks codes, zonder horizontale schaal.
    fn advance(&self, codes: &[u8], size: f32, char_space: f32, word_space: f32) -> f32 {
        codes
            .chunks(self.step())
            .map(|code| {
                let space = if !self.two_byte && code == [32] { word_space } else { 0.0 };
                self.width(code) / 1000.0 * size + char_space + space
            })
            .sum()
    }
}

// ---------------------------------------------------------------------------
// Fragmenten en regels
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Fragment {
    /// Bytes van de hele operatie in de samengevoegde inhoud.
    range: Range,
    /// Wat vóór de nieuwe `TJ` moet staan om de bijwerkingen van `'` en `"` te behouden.
    prefix: Vec<u8>,
    text: String,
    font: Vec<u8>,
    size: f32,
    char_space: f32,
    word_space: f32,
    h_scale: f32,
    /// Volledige tekstmatrix aan het begin (lettergrootte × Tm × CTM).
    matrix: Matrix,
    start: (f32, f32),
    end: (f32, f32),
    /// Verplaatsing in tekstruimte, zonder horizontale schaal.
    advance: f32,
    /// Operatoren die de vulkleur instellen, letterlijk overgenomen.
    color: Vec<u8>,
    /// Woordafstanden die als kerning waren opgegeven (duizendsten).
    gaps: Vec<f32>,
}

/// De grafische toestand voor zover die voor tekst telt; `q`/`Q` bewaren hem volledig.
#[derive(Clone)]
struct State {
    ctm: Matrix,
    color_space: Option<Range>,
    color: Option<Range>,
    font: Vec<u8>,
    size: f32,
    char_space: f32,
    word_space: f32,
    h_scale: f32,
    leading: f32,
    rise: f32,
    render_mode: i32,
}

struct Analysis {
    lines: Vec<Line>,
    /// Per lettertype: welk teken hoort bij welke code, afgeleid uit bestaande tekst.
    seen: HashMap<Vec<u8>, HashMap<char, Vec<u8>>>,
}

#[derive(Clone, Debug)]
struct Line {
    fragments: Vec<Fragment>,
    /// Tekst inclusief witruimte aan begin en einde.
    raw: String,
}

fn nums(operands: &[Operand]) -> Vec<f32> {
    operands.iter().filter_map(|o| if let Operand::Num(v) = o { Some(*v) } else { None }).collect()
}

fn analyse(doc: &Document, pid: ObjectId, data: &[u8]) -> Analysis {
    let font_dicts = doc.get_page_fonts(pid).unwrap_or_default();
    let fonts: HashMap<&Vec<u8>, Font<'_>> = font_dicts.iter().map(|(k, d)| (k, load_font(doc, d))).collect();
    let mut seen: HashMap<Vec<u8>, HashMap<char, Vec<u8>>> = HashMap::new();

    let mut fragments: Vec<Fragment> = Vec::new();
    let mut stack: Vec<State> = Vec::new();
    let mut gs = State {
        ctm: IDENTITY,
        color_space: None,
        color: None,
        font: Vec::new(),
        size: 0.0,
        char_space: 0.0,
        word_space: 0.0,
        h_scale: 1.0,
        leading: 0.0,
        rise: 0.0,
        render_mode: 0,
    };
    let (mut tm, mut tlm) = (IDENTITY, IDENTITY);

    for op in parse(data) {
        let n = nums(&op.operands);
        let mut show: Option<&Operand> = None;
        let mut prefix: Vec<u8> = Vec::new();
        match op.op.as_slice() {
            b"q" => stack.push(gs.clone()),
            b"Q" => {
                if let Some(saved) = stack.pop() {
                    gs = saved;
                }
            }
            b"cm" if n.len() == 6 => gs.ctm = mul([n[0], n[1], n[2], n[3], n[4], n[5]], gs.ctm),
            b"g" | b"rg" | b"k" => {
                gs.color_space = None;
                gs.color = Some(op.range);
            }
            b"cs" => {
                gs.color_space = Some(op.range);
                gs.color = None;
            }
            b"sc" | b"scn" => gs.color = Some(op.range),
            b"BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            b"Tf" => {
                if let Some(Operand::Name(name)) = op.operands.first() {
                    gs.font = name.clone();
                }
                gs.size = n.first().copied().unwrap_or(gs.size);
            }
            b"Tc" if n.len() == 1 => gs.char_space = n[0],
            b"Tw" if n.len() == 1 => gs.word_space = n[0],
            b"Tz" if n.len() == 1 => gs.h_scale = n[0] / 100.0,
            b"TL" if n.len() == 1 => gs.leading = n[0],
            b"Ts" if n.len() == 1 => gs.rise = n[0],
            b"Tr" if n.len() == 1 => gs.render_mode = n[0] as i32,
            b"Td" | b"TD" if n.len() == 2 => {
                if op.op == b"TD" {
                    gs.leading = -n[1];
                }
                tlm = mul([1.0, 0.0, 0.0, 1.0, n[0], n[1]], tlm);
                tm = tlm;
            }
            b"Tm" if n.len() == 6 => {
                tlm = [n[0], n[1], n[2], n[3], n[4], n[5]];
                tm = tlm;
            }
            b"T*" => {
                tlm = mul([1.0, 0.0, 0.0, 1.0, 0.0, -gs.leading], tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" => show = op.operands.last(),
            b"'" | b"\"" => {
                if op.op == b"\"" && n.len() >= 2 && op.starts.len() == 3 {
                    gs.word_space = n[0];
                    gs.char_space = n[1];
                    // `aw ac (tekst) "` is hetzelfde als `aw Tw ac Tc T* (tekst) Tj`.
                    prefix.extend_from_slice(data[op.starts[0]..op.starts[1]].trim_ascii());
                    prefix.extend_from_slice(b" Tw ");
                    prefix.extend_from_slice(data[op.starts[1]..op.starts[2]].trim_ascii());
                    prefix.extend_from_slice(b" Tc ");
                }
                prefix.extend_from_slice(b"T* ");
                tlm = mul([1.0, 0.0, 0.0, 1.0, 0.0, -gs.leading], tlm);
                tm = tlm;
                show = op.operands.last();
            }
            _ => {}
        }
        let Some(operand) = show else { continue };
        let Some(font) = fonts.get(&gs.font) else { continue };
        let (size, h_scale) = (gs.size, gs.h_scale);
        let start_matrix = mul([size * h_scale, 0.0, 0.0, size, 0.0, gs.rise], mul(tm, gs.ctm));

        let mut text = String::new();
        let mut advance = 0.0f32;
        let mut gaps = Vec::new();
        let pieces: Vec<&Operand> = match operand {
            Operand::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for piece in pieces {
            match piece {
                Operand::Str(bytes) => {
                    let known = seen.entry(gs.font.clone()).or_default();
                    for code in bytes.chunks(font.step()) {
                        let decoded = font.decode(code);
                        let mut chars = decoded.chars();
                        if let (Some(c), None) = (chars.next(), chars.next()) {
                            if font.reliable && code.len() == font.step() {
                                known.entry(c).or_insert_with(|| code.to_vec());
                            }
                        }
                        text.push_str(&decoded);
                    }
                    advance += font.advance(bytes, size, gs.char_space, gs.word_space);
                }
                Operand::Num(adjust) => {
                    // Grote negatieve correcties in TJ zijn in de praktijk woordspaties.
                    if *adjust < -200.0 && !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                        gaps.push(-adjust);
                    }
                    advance -= adjust / 1000.0 * size;
                }
                _ => {}
            }
        }
        tm = mul([1.0, 0.0, 0.0, 1.0, advance * h_scale, 0.0], tm);
        let end_matrix = mul([1.0, 0.0, 0.0, 1.0, 0.0, gs.rise], mul(tm, gs.ctm));

        // Onzichtbare tekst (OCR-laag) en tekst in een niet-leesbaar lettertype niet aanbieden.
        let visible = !matches!(gs.render_mode, 3 | 7);
        let finite = start_matrix.iter().chain(&end_matrix).all(|v| v.is_finite()) && advance.is_finite();
        if !visible || !font.reliable || !finite || size.abs() < 0.01 || h_scale.abs() < 0.001 {
            continue;
        }
        let mut color = Vec::new();
        for range in [gs.color_space, gs.color].into_iter().flatten() {
            color.extend_from_slice(&data[range.0..range.1]);
            color.push(b' ');
        }
        fragments.push(Fragment {
            range: op.range,
            prefix,
            text,
            font: gs.font.clone(),
            size,
            char_space: gs.char_space,
            word_space: gs.word_space,
            h_scale,
            matrix: start_matrix,
            start: (start_matrix[4], start_matrix[5]),
            end: (end_matrix[4], end_matrix[5]),
            advance,
            color,
            gaps,
        });
    }
    Analysis { lines: group(fragments), seen }
}

fn quad(matrix: Matrix, start: (f32, f32), end: (f32, f32)) -> [(f32, f32); 4] {
    // `up` is de richting van één lettergrootte omhoog.
    let up = (matrix[2], matrix[3]);
    let (below, above) = (-0.24, 0.82);
    [
        (start.0 + up.0 * below, start.1 + up.1 * below),
        (end.0 + up.0 * below, end.1 + up.1 * below),
        (end.0 + up.0 * above, end.1 + up.1 * above),
        (start.0 + up.0 * above, start.1 + up.1 * above),
    ]
}

fn group(fragments: Vec<Fragment>) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for mut f in fragments {
        let m = f.matrix;
        let height = (m[2] * m[2] + m[3] * m[3]).sqrt();
        let width = (m[0] * m[0] + m[1] * m[1]).sqrt();
        if height < 0.01 || width < 0.01 {
            continue;
        }
        let dir = (m[0] / width, m[1] / width);
        if let Some(line) = lines.last_mut() {
            let last = &line.fragments[line.fragments.len() - 1];
            let first = &line.fragments[0];
            let lm = first.matrix;
            let close = |a: f32, b: f32| (a - b).abs() < 0.01 * height;
            let same_style = first.font == f.font
                && (0..4).all(|i| close(lm[i], m[i]))
                && first.char_space == f.char_space
                && first.word_space == f.word_space
                && first.h_scale == f.h_scale;
            let delta = (f.start.0 - last.end.0, f.start.1 - last.end.1);
            let along = delta.0 * dir.0 + delta.1 * dir.1;
            let across = (delta.0 * -dir.1 + delta.1 * dir.0).abs();
            if same_style && across < 0.15 * height && along > -0.5 * width && along < MAX_GAP_EM * width {
                if along > 0.2 * width && !line.raw.ends_with(' ') && !f.text.starts_with(' ') {
                    line.raw.push(' ');
                    f.gaps.push(along / width * 1000.0);
                }
                line.raw.push_str(&f.text);
                line.fragments.push(f);
                continue;
            }
        }
        lines.push(Line { raw: f.text.clone(), fragments: vec![f] });
    }
    lines.retain(|l| !l.raw.trim().is_empty());
    lines
}

// ---------------------------------------------------------------------------
// Publieke interface
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TextLine {
    pub text: String,
    /// Hoekpunten in PDF-gebruikersruimte: linksonder, rechtsonder, rechtsboven, linksboven.
    pub quad: [(f32, f32); 4],
    /// Bytepositie van de eerste operator; samen met de tekst herkent dit de regel.
    key: usize,
}

struct Segment {
    id: ObjectId,
    /// Bereik van deze stream in de samengevoegde inhoud.
    range: Range,
}

/// De content streams van de pagina, uitgepakt en achter elkaar gezet.
fn page_content(doc: &Document, pid: ObjectId) -> Result<(Vec<u8>, Vec<Segment>)> {
    let mut data = Vec::new();
    let mut segments = Vec::new();
    for id in doc.get_page_contents(pid) {
        let Ok(Object::Stream(stream)) = doc.get_object(id) else { continue };
        let bytes = if stream.dict.has(b"Filter") {
            stream.decompressed_content().map_err(|_| anyhow!(t("err.content_read")))?
        } else {
            stream.content.clone()
        };
        let start = data.len();
        data.extend_from_slice(&bytes);
        segments.push(Segment { id, range: (start, data.len()) });
        // Streams worden als losse tokens aaneengeregen: een scheiding ertussen.
        data.push(b'\n');
    }
    Ok((data, segments))
}

fn public_line(line: &Line) -> TextLine {
    let first = &line.fragments[0];
    let last = &line.fragments[line.fragments.len() - 1];
    TextLine { text: line.raw.trim().to_string(), quad: quad(first.matrix, first.start, last.end), key: first.range.0 }
}

/// Alle bewerkbare tekstregels in de content stream van de pagina.
///
/// Tekst in ingesloten formulieren (Form XObjects), onzichtbare tekst en tekst
/// in lettertypen waarvan de codering niet betrouwbaar te lezen is vallen hier buiten.
pub fn text_lines(doc: &Document, page_index: usize) -> Result<Vec<TextLine>> {
    let pid = page_id(doc, page_index)?;
    let (data, _) = page_content(doc, pid)?;
    Ok(analyse(doc, pid, &data).lines.iter().map(public_line).collect())
}

fn standard_font(original: &str) -> Dictionary {
    let lower = original.to_lowercase();
    let bold = lower.contains("bold") || lower.contains("black") || lower.contains("heavy");
    let italic = lower.contains("italic") || lower.contains("oblique");
    let mono = ["courier", "mono", "menlo", "consolas"].iter().any(|n| lower.contains(n));
    let serif =
        lower.contains("times") || lower.contains("georgia") || (lower.contains("serif") && !lower.contains("sans"));
    let name = match (mono, serif, bold, italic) {
        (true, _, true, true) => "Courier-BoldOblique",
        (true, _, true, false) => "Courier-Bold",
        (true, _, false, true) => "Courier-Oblique",
        (true, _, false, false) => "Courier",
        (_, true, true, true) => "Times-BoldItalic",
        (_, true, true, false) => "Times-Bold",
        (_, true, false, true) => "Times-Italic",
        (_, true, false, false) => "Times-Roman",
        (_, _, true, true) => "Helvetica-BoldOblique",
        (_, _, true, false) => "Helvetica-Bold",
        (_, _, false, true) => "Helvetica-Oblique",
        _ => "Helvetica",
    };
    dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => name, "Encoding" => "WinAnsiEncoding" }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replaced {
    /// De tekst staat in het oorspronkelijke lettertype.
    OriginalFont,
    /// Het lettertype miste tekens; er is een standaardlettertype gebruikt.
    SubstituteFont,
}

enum Piece {
    Codes(Vec<u8>),
    /// Woordafstand als kerning, in duizendsten van de lettergrootte.
    Gap(f32),
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 2);
    out.push('<');
    for b in bytes {
        out.push_str(&format!("{b:02X}"));
    }
    out.push('>');
    out
}

/// `[...] TJ` die niets tekent maar de pen evenveel laat opschuiven als het origineel.
fn spacer(f: &Fragment) -> Vec<u8> {
    let mut out = f.prefix.clone();
    out.extend_from_slice(format!("[{:.3}] TJ", -f.advance / f.size * 1000.0).as_bytes());
    out
}

/// Codeert `text` in het lettertype van de regel, of `None` als dat niet kan.
fn encode_in_font(
    font: &Font<'_>,
    seen: Option<&HashMap<char, Vec<u8>>>,
    text: &str,
    gap: f32,
) -> Option<Vec<Piece>> {
    let mut pieces = Vec::new();
    let mut run: Vec<u8> = Vec::new();
    if font.restricted {
        // Alleen tekens waarvan de code uit bestaande tekst bekend is: dan bestaat de glyph zeker.
        let seen = seen?;
        for c in text.chars() {
            match seen.get(&c) {
                Some(code) => run.extend_from_slice(code),
                // Veel lettertypen (alle TeX-uitvoer) hebben geen spatie: gebruik een woordafstand.
                None if c == ' ' => {
                    if !run.is_empty() {
                        pieces.push(Piece::Codes(std::mem::take(&mut run)));
                    }
                    pieces.push(Piece::Gap(gap));
                }
                None => return None,
            }
        }
    } else {
        let encoding = font.encoding.as_ref()?;
        let bytes = Document::encode_text(encoding, text);
        let round_trip = Document::decode_text(encoding, &bytes).ok()?;
        if round_trip != text || bytes.len() != text.chars().count() {
            return None;
        }
        run = bytes;
    }
    if !run.is_empty() {
        pieces.push(Piece::Codes(run));
    }
    Some(pieces)
}

/// Tekening van de vervangende tekst in een standaardlettertype.
struct Substitute {
    color: Vec<u8>,
    matrix: Matrix,
    text: Vec<u8>,
    original_font: String,
}

/// Vervangt de tekst van `line` (verkregen via [`text_lines`] op dezelfde documenttoestand).
pub fn replace_line(doc: &mut Document, page_index: usize, line: &TextLine, new_text: &str) -> Result<Replaced> {
    let pid = page_id(doc, page_index)?;
    let (data, segments) = page_content(doc, pid)?;

    // Fase 1: alles uitrekenen zonder het document aan te raken.
    let (edits, substitute) = {
        let analysis = analyse(doc, pid, &data);
        let current = analysis
            .lines
            .iter()
            .find(|l| l.fragments[0].range.0 == line.key && l.raw.trim() == line.text)
            .ok_or_else(|| anyhow!(t("err.text_stale")))?;
        let first = &current.fragments[0];
        let fonts = doc.get_page_fonts(pid).unwrap_or_default();
        let font = load_font(doc, fonts.get(&first.font).ok_or_else(|| anyhow!(t("err.font_missing")))?);

        // Witruimte aan begin en einde hoort bij de PDF, niet bij wat de gebruiker ziet: behouden.
        let lead = &current.raw[..current.raw.len() - current.raw.trim_start().len()];
        let trail = &current.raw[current.raw.trim_end().len()..];
        let full = format!("{lead}{new_text}{trail}");
        let gap = current
            .fragments
            .iter()
            .flat_map(|f| f.gaps.iter().copied())
            .next()
            .unwrap_or(DEFAULT_SPACE)
            .clamp(100.0, 1500.0);

        let mut edits: Vec<(Range, Vec<u8>)> = Vec::new();
        let substitute = match encode_in_font(&font, analysis.seen.get(&first.font), &full, gap) {
            Some(pieces) => {
                let mut body = String::from("[");
                let mut advance = 0.0f32;
                for piece in &pieces {
                    match piece {
                        Piece::Codes(codes) => {
                            body.push_str(&hex(codes));
                            advance += font.advance(codes, first.size, first.char_space, first.word_space);
                        }
                        Piece::Gap(g) => {
                            body.push_str(&format!("{:.1}", -g));
                            advance += g / 1000.0 * first.size;
                        }
                    }
                }
                // Terug naar de oorspronkelijke eindpositie, zodat wat volgt niet verschuift.
                body.push_str(&format!("{:.3}] TJ", (advance - first.advance) / first.size * 1000.0));
                let mut bytes = first.prefix.clone();
                bytes.extend_from_slice(body.as_bytes());
                edits.push((first.range, bytes));
                edits.extend(current.fragments[1..].iter().map(|f| (f.range, spacer(f))));
                None
            }
            None => {
                // Eerst controleren of het standaardlettertype de tekst aankan; pas dan iets wijzigen.
                let text = content_string(new_text)?;
                edits.extend(current.fragments.iter().map(|f| (f.range, spacer(f))));
                Some(Substitute {
                    color: first.color.clone(),
                    matrix: first.matrix,
                    text,
                    original_font: font.name.clone(),
                })
            }
        };
        (edits, substitute)
    };

    // Fase 2: de bytes per stream vervangen.
    let mut per_stream: HashMap<usize, Vec<(Range, Vec<u8>)>> = HashMap::new();
    for (range, bytes) in edits {
        // Een operator die over twee streams verdeeld is laten we met rust.
        let index = segments
            .iter()
            .position(|s| range.0 >= s.range.0 && range.1 <= s.range.1)
            .ok_or_else(|| anyhow!(t("err.text_pos")))?;
        let base = segments[index].range.0;
        per_stream.entry(index).or_default().push(((range.0 - base, range.1 - base), bytes));
    }
    // Dezelfde stream kan door meerdere pagina's (of twee keer door deze) worden gebruikt.
    let mut uses: HashMap<ObjectId, usize> = HashMap::new();
    for other in page_ids(doc) {
        for id in doc.get_page_contents(other) {
            *uses.entry(id).or_default() += 1;
        }
    }
    let mut ids: Vec<ObjectId> = segments.iter().map(|s| s.id).collect();
    for (index, mut list) in per_stream {
        let segment = &segments[index];
        let mut content = data[segment.range.0..segment.range.1].to_vec();
        list.sort_by_key(|(range, _)| std::cmp::Reverse(range.0));
        for ((start, end), bytes) in list {
            content.splice(start..end, bytes);
        }
        let Ok(Object::Stream(old)) = doc.get_object(segment.id) else { bail!(t("err.text_pos")) };
        let mut dict = old.dict.clone();
        for key in [b"Filter".as_slice(), b"DecodeParms", b"Length"] {
            dict.remove(key);
        }
        let mut stream = Stream::new(dict, content);
        let _ = stream.compress();
        if uses.get(&segment.id).copied().unwrap_or(0) > 1 {
            ids[index] = doc.add_object(stream);
        } else {
            doc.objects.insert(segment.id, Object::Stream(stream));
        }
    }
    let contents: Vec<Object> = ids.into_iter().map(Object::Reference).collect();
    doc.get_dictionary_mut(pid)?.set("Contents", contents);

    let Some(sub) = substitute else { return Ok(Replaced::OriginalFont) };
    let font = doc.add_object(standard_font(&sub.original_font));
    let [a, b, c, d, e, f] = sub.matrix;
    // Dezelfde kleuroperatoren als het origineel; de bronnen van de pagina gelden ook hier.
    let mut content = if sub.color.is_empty() { b"0 g ".to_vec() } else { sub.color };
    content.extend_from_slice(
        format!("BT {a:.4} {b:.4} {c:.4} {d:.4} {e:.3} {f:.3} Tm /PkF{} 1 Tf ", font.0).as_bytes(),
    );
    content.extend(sub.text);
    content.extend_from_slice(b" Tj ET");
    overlay(doc, pid, content, font, None, false)?;
    Ok(Replaced::SubstituteFont)
}
