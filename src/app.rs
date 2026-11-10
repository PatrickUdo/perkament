//! De desktop-interface (egui).

use eframe::egui::{
    self, Align2, Color32, FontId, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use perkament::i18n::{self, t, tf, Lang};
use perkament::pdfops::{self, AnnotInfo, AnnotKind, Meta, NewAnnot, SearchHit, Watermark};
use perkament::render::{self, Kind, PageGeom, Renderer, Response};
use perkament::textedit::{self, Replaced, TextLine};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PAGE_GAP: f32 = 14.0;
const THUMB_W: f32 = 116.0;
/// Hoe lang één pagina mag renderen voordat de bewaker ingrijpt.
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// Geheugen dat de geschiedenis voor ongedaan maken ongeveer mag gebruiken.
const UNDO_BUDGET: usize = 400 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Hand,
    EditText,
    Highlight,
    Rectangle,
    Note,
    FreeText,
    Ink,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    None,
    Search,
    Comments,
}

struct Tex {
    scale: f32,
    handle: egui::TextureHandle,
}

struct Doc {
    pdf: lopdf::Document,
    path: Option<PathBuf>,
    /// Lengte van `undo` op het moment van laden of opslaan; `None` als die toestand onbereikbaar is.
    saved_at: Option<usize>,
    /// Aantal stappen dat bewaard blijft: elke stap is een volledige kopie van het document.
    undo_limit: usize,
    undo: Vec<lopdf::Document>,
    redo: Vec<lopdf::Document>,
    generation: u64,
    geoms: Vec<PageGeom>,
    geoms_ready: bool,
    /// De documentversie waar `geoms` bij hoort; loopt even achter na een wijziging.
    geoms_generation: u64,
    /// Pagina's die de renderer niet kon weergeven.
    failed: HashSet<usize>,
    textures: HashMap<(usize, Kind), Tex>,
    pending: HashMap<(usize, Kind), f32>,
    current: usize,
    annots: Vec<AnnotInfo>,
    texts: Option<Vec<String>>,
    /// Bewerkbare tekstregels per pagina (lui berekend).
    lines: HashMap<usize, Vec<TextLine>>,
}

impl Doc {
    fn dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }

    fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| t("untitled").into())
    }
}

#[derive(Clone)]
enum After {
    Quit,
    Open(Option<PathBuf>),
    CloseDoc,
}

enum Dialog {
    Properties(Meta),
    Watermark(Watermark),
    PageNumbers(String),
    Extract { from: usize, to: usize },
    Text { page: usize, kind: AnnotKind, rect: [f32; 4], text: String },
    EditText { page: usize, line: TextLine, text: String },
    Unsaved(After),
    Error(String),
    About,
}

enum Drag {
    None,
    /// Startpunt in paginaruimte (punten, oorsprong linksboven, zonder zoom).
    Rect { page: usize, start: Pos2, now: Pos2 },
    Ink { page: usize, points: Vec<Pos2> },
}

enum Action {
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    CloseDoc,
    Quit,
    Undo,
    Redo,
    Rotate(usize, i64),
    Delete(usize),
    Move(usize, usize),
    InsertBlank(usize),
    InsertPdf(usize),
    ExtractDialog,
    Extract(usize, usize),
    ExportPng,
    PropertiesDialog,
    SetMeta(Meta),
    WatermarkDialog,
    AddWatermark(Watermark),
    PageNumbersDialog,
    AddPageNumbers(String),
    AddAnnot(usize, NewAnnot),
    DeleteAnnot(usize, lopdf::ObjectId),
    ReplaceText(usize, TextLine, String),
    Search,
    GoTo(usize),
    Zoom(f32),
    FitWidth,
    Proceed(After),
    SaveThen(After),
}

/// Menutekst met de sneltoets erachter, in de notatie van het platform.
fn with_shortcut(label: &str, shift: bool, key: &str) -> String {
    let keys = if cfg!(target_os = "macos") {
        format!("{}\u{2318}{key}", if shift { "\u{21E7}" } else { "" })
    } else {
        format!("Ctrl+{}{key}", if shift { "Shift+" } else { "" })
    };
    format!("{label}   {keys}")
}

pub struct PerkamentApp {
    doc: Option<Doc>,
    renderer: Renderer,
    next_generation: u64,
    zoom: f32,
    fit_width: bool,
    zoom_changed: Instant,
    tool: Tool,
    color: [f32; 3],
    show_thumbs: bool,
    side: Side,
    query: String,
    hits: Vec<SearchHit>,
    searched: bool,
    scroll_to: Option<usize>,
    /// Expliciet gekozen pagina: (pagina, frames sinds de sprong, scrollpositie). Geldt tot de gebruiker scrolt.
    nav: Option<(usize, u8, f32)>,
    /// Pagina bovenaan in beeld en de positie daarbinnen, om bij zoomen op dezelfde plek te blijven.
    anchor: Option<(usize, f32)>,
    layout_zoom: f32,
    /// Pagina's waarvoor de renderer nog verzoeken mag afhandelen.
    kept: Vec<usize>,
    thumb_follow: Option<usize>,
    drag: Drag,
    dialog: Option<Dialog>,
    status: String,
    title: String,
    force_quit: bool,
    initial: Option<PathBuf>,
}

impl PerkamentApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let renderer = render::spawn(move || ctx.request_repaint());
        // De ingebouwde zoom van egui zou anders de hele interface schalen bij Cmd/Ctrl met + en -.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        Self {
            doc: None,
            renderer,
            next_generation: 1,
            zoom: 1.0,
            fit_width: true,
            zoom_changed: Instant::now(),
            tool: Tool::Hand,
            color: [1.0, 0.85, 0.1],
            show_thumbs: true,
            side: Side::None,
            query: String::new(),
            hits: Vec::new(),
            searched: false,
            scroll_to: None,
            nav: None,
            anchor: None,
            layout_zoom: 1.0,
            kept: Vec::new(),
            thumb_follow: None,
            drag: Drag::None,
            dialog: None,
            status: t("st.start").into(),
            title: String::new(),
            force_quit: false,
            initial,
        }
    }

    // -- documentbeheer -----------------------------------------------------

    fn error(&mut self, message: impl Into<String>) {
        self.dialog = Some(Dialog::Error(message.into()));
    }

    fn open_path(&mut self, path: &Path) {
        let size = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0).max(1);
        let loaded = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("{}", tf("err.read_file", &[("e", &e.to_string())])))
            .and_then(|bytes| pdfops::load(&bytes));
        match loaded {
            Ok(pdf) => {
                self.doc = Some(Doc {
                    pdf,
                    path: Some(path.to_path_buf()),
                    saved_at: Some(0),
                    undo_limit: (UNDO_BUDGET / size).clamp(3, 30),
                    undo: Vec::new(),
                    redo: Vec::new(),
                    generation: 0,
                    geoms: Vec::new(),
                    geoms_ready: false,
                    geoms_generation: 0,
                    failed: HashSet::new(),
                    textures: HashMap::new(),
                    pending: HashMap::new(),
                    current: 0,
                    annots: Vec::new(),
                    texts: None,
                    lines: HashMap::new(),
                });
                self.hits.clear();
                self.searched = false;
                self.fit_width = true;
                self.scroll_to = Some(0);
                self.reload();
                self.status = tf("st.opened", &[("path", &path.display().to_string())]);
            }
            Err(e) => self.error(format!("{}: {e:#}", path.display())),
        }
    }

    /// Stuurt de huidige toestand naar de renderer en wist afgeleide caches.
    fn reload(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        doc.generation = self.next_generation;
        self.next_generation += 1;
        doc.textures.clear();
        doc.pending.clear();
        doc.failed.clear();
        // Een lopende sleepactie verwijst naar de oude indeling.
        self.drag = Drag::None;
        self.kept.clear();
        doc.texts = None;
        doc.lines.clear();
        doc.annots = pdfops::list_annotations(&doc.pdf);
        self.hits.clear();
        self.searched = false;
        match pdfops::to_bytes(&mut doc.pdf) {
            Ok(bytes) => {
                self.renderer.load(doc.generation, bytes);
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn mutate(&mut self, label: &str, f: impl FnOnce(&mut lopdf::Document) -> anyhow::Result<()>) {
        let Some(doc) = &mut self.doc else { return };
        let backup = doc.pdf.clone();
        match f(&mut doc.pdf) {
            Ok(()) => {
                // Lag de opgeslagen toestand in de tak die nu vervalt, dan is die niet meer te bereiken.
                if doc.saved_at.is_some_and(|at| at > doc.undo.len()) {
                    doc.saved_at = None;
                }
                doc.undo.push(backup);
                if doc.undo.len() > doc.undo_limit {
                    doc.undo.remove(0);
                    doc.saved_at = doc.saved_at.and_then(|at| at.checked_sub(1));
                }
                doc.redo.clear();
                self.status = label.to_string();
                self.reload();
            }
            Err(e) => {
                doc.pdf = backup;
                self.error(format!("{e:#}"));
            }
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        let Some(doc) = &mut self.doc else { return false };
        // Door een symlink heen schrijven in plaats van hem te vervangen.
        let target = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = target.with_file_name(format!(".{name}.perkament-tmp"));
        let result = pdfops::to_bytes_for_saving(&doc.pdf).and_then(|bytes| {
            std::fs::write(&tmp, bytes)?;
            if let Ok(existing) = std::fs::metadata(&target) {
                let _ = std::fs::set_permissions(&tmp, existing.permissions());
            }
            std::fs::rename(&tmp, &target)?;
            Ok(())
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        match result {
            Ok(()) => {
                doc.saved_at = Some(doc.undo.len());
                self.status = tf("st.saved", &[("path", &path.display().to_string())]);
                doc.path = Some(path);
                true
            }
            Err(e) => {
                self.error(tf("err.save", &[("e", &format!("{e:#}"))]));
                false
            }
        }
    }

    fn save(&mut self, force_dialog: bool) -> bool {
        let Some(doc) = &self.doc else { return false };
        let path = match (&doc.path, force_dialog) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .set_file_name(doc.name())
                .save_file(),
        };
        path.is_some_and(|p| self.save_to(p))
    }

    fn page_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |d| pdfops::page_count(&d.pdf))
    }

    // -- acties ---------------------------------------------------------------

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        let dirty = self.doc.as_ref().is_some_and(|d| d.dirty());
        match action {
            Action::Open => {
                if dirty {
                    self.dialog = Some(Dialog::Unsaved(After::Open(None)));
                } else {
                    self.apply(ctx, Action::Proceed(After::Open(None)));
                }
            }
            Action::OpenPath(path) => {
                if dirty {
                    self.dialog = Some(Dialog::Unsaved(After::Open(Some(path))));
                } else {
                    self.open_path(&path);
                }
            }
            Action::CloseDoc => {
                if dirty {
                    self.dialog = Some(Dialog::Unsaved(After::CloseDoc));
                } else {
                    self.apply(ctx, Action::Proceed(After::CloseDoc));
                }
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::SaveThen(after) => {
                if self.save(false) {
                    self.apply(ctx, Action::Proceed(after));
                }
            }
            Action::Proceed(after) => match after {
                After::Quit => {
                    self.force_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                After::Open(Some(path)) => self.open_path(&path),
                After::Open(None) => {
                    if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                        self.open_path(&path);
                    }
                }
                After::CloseDoc => {
                    self.doc = None;
                    self.status = t("st.closed").into();
                }
            },
            Action::Save => {
                self.save(false);
            }
            Action::SaveAs => {
                self.save(true);
            }
            Action::Undo => {
                if let Some(doc) = &mut self.doc {
                    if let Some(prev) = doc.undo.pop() {
                        doc.redo.push(std::mem::replace(&mut doc.pdf, prev));
                        self.status = t("st.undone").into();
                        self.reload();
                    }
                }
            }
            Action::Redo => {
                if let Some(doc) = &mut self.doc {
                    if let Some(next) = doc.redo.pop() {
                        doc.undo.push(std::mem::replace(&mut doc.pdf, next));
                        self.status = t("st.redone").into();
                        self.reload();
                    }
                }
            }
            Action::Rotate(page, delta) => {
                self.mutate(t("st.rotated"), |d| pdfops::rotate_page(d, page, delta));
            }
            Action::Delete(page) => {
                self.mutate(t("st.page_deleted"), |d| pdfops::delete_pages(d, &[page]));
            }
            Action::Move(from, to) => {
                if to < self.page_count() {
                    self.mutate(t("st.page_moved"), |d| pdfops::move_page(d, from, to));
                    self.scroll_to = Some(to);
                }
            }
            Action::InsertBlank(at) => {
                self.mutate(t("st.blank_inserted"), |d| pdfops::insert_blank_page(d, at));
                self.scroll_to = Some(at);
            }
            Action::InsertPdf(at) => {
                if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            self.mutate(t("st.pdf_inserted"), |d| pdfops::insert_pdf(d, &bytes, at).map(|_| ()));
                            self.scroll_to = Some(at);
                        }
                        Err(e) => self.error(tf("err.cannot_read", &[("path", &path.display().to_string()), ("e", &e.to_string())])),
                    }
                }
            }
            Action::ExtractDialog => {
                let n = self.page_count();
                let cur = self.doc.as_ref().map_or(0, |d| d.current) + 1;
                self.dialog = Some(Dialog::Extract { from: cur, to: cur.min(n) });
            }
            Action::Extract(from, to) => {
                let Some(doc) = &self.doc else { return };
                if from == 0 || to < from || to > pdfops::page_count(&doc.pdf) {
                    return;
                }
                let indices: Vec<usize> = (from - 1..to).collect();
                let stem = doc.name().trim_end_matches(".pdf").to_string();
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(format!("{stem}-p{from}-{to}.pdf"))
                    .save_file()
                else {
                    return;
                };
                match pdfops::extract_pages(&doc.pdf, &indices)
                    .and_then(|bytes| Ok(std::fs::write(&path, bytes)?))
                {
                    Ok(()) => {
                        self.status = tf(
                            "st.extracted",
                            &[("from", &from.to_string()), ("to", &to.to_string()), ("path", &path.display().to_string())],
                        )
                    }
                    Err(e) => self.error(tf("err.extract", &[("e", &format!("{e:#}"))])),
                }
            }
            Action::ExportPng => {
                let Some(doc) = &mut self.doc else { return };
                let page = doc.current;
                let stem = doc.name().trim_end_matches(".pdf").to_string();
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG", &["png"])
                    .set_file_name(format!("{stem}-p{}.png", page + 1))
                    .save_file()
                else {
                    return;
                };
                let result = pdfops::to_bytes(&mut doc.pdf)
                    .and_then(|bytes| {
                        render::render_png_with_timeout(bytes, page, 2.0, RENDER_TIMEOUT).map_err(anyhow::Error::msg)
                    })
                    .and_then(|png| Ok(std::fs::write(&path, png)?));
                match result {
                    Ok(()) => self.status = tf("st.exported", &[("path", &path.display().to_string())]),
                    Err(e) => self.error(tf("err.export", &[("e", &format!("{e:#}"))])),
                }
            }
            Action::PropertiesDialog => {
                if let Some(doc) = &self.doc {
                    self.dialog = Some(Dialog::Properties(pdfops::get_meta(&doc.pdf)));
                }
            }
            Action::SetMeta(meta) => {
                self.mutate(t("st.meta_updated"), |d| pdfops::set_meta(d, &meta));
            }
            Action::WatermarkDialog => self.dialog = Some(Dialog::Watermark(Watermark::default())),
            Action::AddWatermark(wm) => {
                self.mutate(t("st.watermark_added"), |d| pdfops::add_watermark(d, &wm));
            }
            Action::PageNumbersDialog => self.dialog = Some(Dialog::PageNumbers(t("pn.default").into())),
            Action::AddPageNumbers(format) => {
                self.mutate(t("st.numbers_added"), |d| pdfops::add_page_numbers(d, &format));
            }
            Action::AddAnnot(page, annot) => {
                self.mutate(t("st.annot_added"), |d| pdfops::add_annotation(d, page, &annot).map(|_| ()));
            }
            Action::DeleteAnnot(page, id) => {
                self.mutate(t("st.annot_deleted"), |d| pdfops::delete_annotation(d, page, id));
            }
            Action::ReplaceText(page, line, text) => {
                let mut outcome = Replaced::OriginalFont;
                self.mutate(t("st.text_changed"), |d| {
                    outcome = textedit::replace_line(d, page, &line, &text)?;
                    Ok(())
                });
                if outcome == Replaced::SubstituteFont && self.dialog.is_none() {
                    self.status = t("st.text_substitute").into();
                }
            }
            Action::Search => {
                let Some(doc) = &mut self.doc else { return };
                let texts = doc.texts.get_or_insert_with(|| {
                    (0..pdfops::page_count(&doc.pdf)).map(|i| pdfops::page_text(&doc.pdf, i)).collect()
                });
                self.hits = pdfops::search(texts, &self.query);
                self.searched = true;
                self.side = Side::Search;
                self.status = tf("st.results", &[("n", &self.hits.len().to_string()), ("q", self.query.trim())]);
                if let Some(first) = self.hits.first() {
                    self.scroll_to = Some(first.page);
                }
            }
            Action::GoTo(page) => {
                if page < self.page_count() {
                    self.scroll_to = Some(page);
                    if let Some(doc) = &mut self.doc {
                        doc.current = page;
                    }
                }
            }
            Action::Zoom(factor) => {
                self.zoom = (self.zoom * factor).clamp(0.1, 8.0);
                self.fit_width = false;
                self.zoom_changed = Instant::now();
            }
            Action::FitWidth => self.fit_width = true,
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let cmd = Modifiers::COMMAND;
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        let hit = |mods, key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key)));
        // In een tekstveld horen Cmd/Ctrl+Z bij dat veld, niet bij het document.
        let typing = ctx.text_edit_focused();
        if !typing && hit(shift_cmd, Key::Z) {
            actions.push(Action::Redo);
        }
        if hit(cmd, Key::Q) {
            actions.push(Action::Quit);
        }
        if hit(shift_cmd, Key::S) {
            actions.push(Action::SaveAs);
        }
        if !typing && hit(cmd, Key::Z) {
            actions.push(Action::Undo);
        }
        if hit(cmd, Key::O) {
            actions.push(Action::Open);
        }
        if hit(cmd, Key::S) {
            actions.push(Action::Save);
        }
        if hit(cmd, Key::W) {
            actions.push(Action::CloseDoc);
        }
        if hit(cmd, Key::F) {
            self.side = Side::Search;
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("search_box")));
        }
        if hit(cmd, Key::Plus) || hit(cmd, Key::Equals) {
            actions.push(Action::Zoom(1.2));
        }
        if hit(cmd, Key::Minus) {
            actions.push(Action::Zoom(1.0 / 1.2));
        }
        if hit(cmd, Key::Num0) {
            actions.push(Action::FitWidth);
        }
    }

    // -- interface ------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let has_doc = self.doc.is_some();
        let (cur, n, can_undo, can_redo) = self.doc.as_ref().map_or((0, 0, false, false), |d| {
            (d.current, pdfops::page_count(&d.pdf), !d.undo.is_empty(), !d.redo.is_empty())
        });
        let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, action: Action| {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                actions.push(action);
                ui.close();
            }
        };
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(t("menu.file"), |ui| {
                item(ui, true, &with_shortcut(t("menu.open"), false, "O"), Action::Open);
                item(ui, has_doc, &with_shortcut(t("menu.save"), false, "S"), Action::Save);
                item(ui, has_doc, &with_shortcut(t("menu.save_as"), true, "S"), Action::SaveAs);
                ui.separator();
                item(ui, has_doc, t("menu.export_png"), Action::ExportPng);
                item(ui, has_doc, t("menu.extract"), Action::ExtractDialog);
                ui.separator();
                item(ui, has_doc, &with_shortcut(t("menu.close"), false, "W"), Action::CloseDoc);
                item(ui, true, t("menu.quit"), Action::Quit);
            });
            ui.menu_button(t("menu.edit"), |ui| {
                item(ui, can_undo, &with_shortcut(t("menu.undo"), false, "Z"), Action::Undo);
                item(ui, can_redo, &with_shortcut(t("menu.redo"), true, "Z"), Action::Redo);
            });
            ui.menu_button(t("menu.pages"), |ui| {
                item(ui, has_doc, t("menu.rotate_left"), Action::Rotate(cur, -90));
                item(ui, has_doc, t("menu.rotate_right"), Action::Rotate(cur, 90));
                ui.separator();
                item(ui, has_doc && cur > 0, t("menu.move_forward"), Action::Move(cur, cur.saturating_sub(1)));
                item(ui, has_doc && cur + 1 < n, t("menu.move_back"), Action::Move(cur, cur + 1));
                ui.separator();
                item(ui, has_doc, t("menu.insert_blank"), Action::InsertBlank(cur + 1));
                item(ui, has_doc, t("menu.insert_pdf"), Action::InsertPdf(cur + 1));
                item(ui, has_doc, t("menu.append_pdf"), Action::InsertPdf(n));
                ui.separator();
                item(ui, has_doc && n > 1, t("menu.delete_page"), Action::Delete(cur));
            });
            ui.menu_button(t("menu.document"), |ui| {
                item(ui, has_doc, t("menu.properties"), Action::PropertiesDialog);
                item(ui, has_doc, t("menu.watermark"), Action::WatermarkDialog);
                item(ui, has_doc, t("menu.page_numbers"), Action::PageNumbersDialog);
            });
            ui.menu_button(t("menu.view"), |ui| {
                item(ui, has_doc, &with_shortcut(t("menu.zoom_in"), false, "+"), Action::Zoom(1.2));
                item(ui, has_doc, &with_shortcut(t("menu.zoom_out"), false, "-"), Action::Zoom(1.0 / 1.2));
                item(ui, has_doc, &with_shortcut(t("menu.fit_width"), false, "0"), Action::FitWidth);
                ui.separator();
                if ui.checkbox(&mut self.show_thumbs, t("menu.thumbnails")).clicked() {
                    ui.close();
                }
                let mut search = self.side == Side::Search;
                if ui.checkbox(&mut search, t("menu.search_panel")).clicked() {
                    self.side = if search { Side::Search } else { Side::None };
                    ui.close();
                }
                let mut comments = self.side == Side::Comments;
                if ui.checkbox(&mut comments, t("menu.comments")).clicked() {
                    self.side = if comments { Side::Comments } else { Side::None };
                    ui.close();
                }
                ui.separator();
                ui.menu_button(t("menu.language"), |ui| {
                    for lang in Lang::ALL {
                        if ui.radio(i18n::current() == lang, lang.name()).clicked() {
                            i18n::set(lang);
                            crate::settings::save_language(lang);
                            if self.doc.is_none() {
                                self.status = t("st.start").into();
                            }
                            ui.close();
                        }
                    }
                });
            });
            ui.menu_button(t("menu.help"), |ui| {
                if ui.button(t("menu.about")).clicked() {
                    self.dialog = Some(Dialog::About);
                    ui.close();
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let n = self.page_count();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui.button(t("tb.open")).clicked() {
                actions.push(Action::Open);
            }
            let dirty = self.doc.as_ref().is_some_and(|d| d.dirty());
            if ui.add_enabled(dirty, egui::Button::new(t("tb.save"))).clicked() {
                actions.push(Action::Save);
            }
            ui.separator();
            ui.add_enabled_ui(n > 0, |ui| {
                for (tool, label, tip) in [
                    (Tool::Hand, t("tool.hand"), t("tool.hand.tip")),
                    (Tool::EditText, t("tool.edit"), t("tool.edit.tip")),
                    (Tool::Highlight, t("tool.highlight"), t("tool.highlight.tip")),
                    (Tool::Rectangle, t("tool.rect"), t("tool.rect.tip")),
                    (Tool::Note, t("tool.note"), t("tool.note.tip")),
                    (Tool::FreeText, t("tool.text"), t("tool.text.tip")),
                    (Tool::Ink, t("tool.pen"), t("tool.pen.tip")),
                ] {
                    ui.selectable_value(&mut self.tool, tool, label).on_hover_text(tip);
                }
                ui.color_edit_button_rgb(&mut self.color).on_hover_text(t("tb.color.tip"));
                ui.separator();
                if ui.button("−").on_hover_text(t("menu.zoom_out")).clicked() {
                    actions.push(Action::Zoom(1.0 / 1.2));
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").on_hover_text(t("menu.zoom_in")).clicked() {
                    actions.push(Action::Zoom(1.2));
                }
                if ui.selectable_label(self.fit_width, t("tb.width")).on_hover_text(t("tb.width.tip")).clicked() {
                    self.fit_width = !self.fit_width;
                }
                ui.separator();
                let cur = self.doc.as_ref().map_or(0, |d| d.current);
                if ui.add_enabled(cur > 0, egui::Button::new("◀")).clicked() {
                    actions.push(Action::GoTo(cur.saturating_sub(1)));
                }
                let mut page = cur + 1;
                if ui.add(egui::DragValue::new(&mut page).range(1..=n.max(1)).speed(0.1)).changed() {
                    actions.push(Action::GoTo(page - 1));
                }
                ui.label(format!("/ {n}"));
                if ui.add_enabled(cur + 1 < n, egui::Button::new("▶")).clicked() {
                    actions.push(Action::GoTo(cur + 1));
                }
                ui.separator();
                if ui.button("⟲").on_hover_text(t("tb.rotate_left.tip")).clicked() {
                    actions.push(Action::Rotate(cur, -90));
                }
                if ui.button("⟳").on_hover_text(t("tb.rotate_right.tip")).clicked() {
                    actions.push(Action::Rotate(cur, 90));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(self.side == Side::Comments, t("menu.comments")).clicked() {
                        self.side = if self.side == Side::Comments { Side::None } else { Side::Comments };
                    }
                    if ui.button(t("tb.search")).clicked() {
                        actions.push(Action::Search);
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .id(egui::Id::unique("search_box"))
                            .hint_text(t("tb.search.hint"))
                            .desired_width(180.0),
                    );
                    if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        actions.push(Action::Search);
                    }
                });
            });
        });
    }

    fn request_texture(renderer: &Renderer, doc: &mut Doc, page: usize, kind: Kind, want: f32, settled: bool) {
        if doc.failed.contains(&page) {
            return;
        }
        let key = (page, kind);
        let close = |a: f32, b: f32| (a / b - 1.0).abs() < 0.04;
        let have = doc.textures.get(&key).map(|t| t.scale);
        if have.is_some_and(|s| close(s, want)) || doc.pending.get(&key).is_some_and(|&s| close(s, want)) {
            return;
        }
        // Tijdens het zoomen de bestaande textuur opgerekt tonen; pas renderen als het stilstaat.
        if have.is_some() && !settled {
            return;
        }
        doc.pending.insert(key, want);
        renderer.request(doc.generation, page, want, kind);
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(doc) = &mut self.doc else { return };
        let ppp = ui.ctx().pixels_per_point();
        let n = doc.geoms.len();
        let follow = self.thumb_follow.take();
        let mut shown: Option<(usize, usize)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            for i in 0..n {
                let g = doc.geoms[i];
                let th = THUMB_W * g.h / g.w.max(1.0);
                let (cell, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), th + 26.0), Sense::click());
                if follow == Some(i) {
                    response.scroll_to_me(None);
                }
                if ui.is_rect_visible(cell) {
                    shown = Some((shown.map_or(i, |(lo, _)| lo), i));
                    let rect = Rect::from_min_size(
                        Pos2::new(cell.center().x - THUMB_W / 2.0, cell.min.y + 4.0),
                        Vec2::new(THUMB_W, th),
                    );
                    let selected = doc.current == i;
                    let painter = ui.painter();
                    if selected {
                        painter.rect_filled(cell.shrink(2.0), 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.35));
                    }
                    painter.rect_filled(rect, 0.0, Color32::WHITE);
                    Self::request_texture(&self.renderer, doc, i, Kind::Thumb, THUMB_W / g.w.max(1.0) * ppp, true);
                    if let Some(tex) = doc.textures.get(&(i, Kind::Thumb)) {
                        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                        painter.image(tex.handle.id(), rect, uv, Color32::WHITE);
                    }
                    let stroke = if selected {
                        Stroke::new(2.0, ui.visuals().selection.bg_fill)
                    } else {
                        Stroke::new(1.0, Color32::from_gray(120))
                    };
                    painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Outside);
                    painter.text(
                        Pos2::new(cell.center().x, rect.max.y + 3.0),
                        Align2::CENTER_TOP,
                        (i + 1).to_string(),
                        FontId::proportional(12.0),
                        ui.visuals().text_color(),
                    );
                }
                if response.clicked() {
                    actions.push(Action::GoTo(i));
                }
                response.context_menu(|ui| {
                    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, action: Action| {
                        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                            actions.push(action);
                            ui.close();
                        }
                    };
                    item(ui, true, t("menu.rotate_left"), Action::Rotate(i, -90));
                    item(ui, true, t("menu.rotate_right"), Action::Rotate(i, 90));
                    ui.separator();
                    item(ui, i > 0, t("ctx.forward"), Action::Move(i, i.saturating_sub(1)));
                    item(ui, i + 1 < n, t("ctx.back"), Action::Move(i, i + 1));
                    ui.separator();
                    item(ui, true, t("ctx.blank_after"), Action::InsertBlank(i + 1));
                    item(ui, true, t("ctx.pdf_after"), Action::InsertPdf(i + 1));
                    ui.separator();
                    item(ui, n > 1, t("ctx.delete"), Action::Delete(i));
                });
            }
        });
        if let Some((lo, hi)) = shown {
            doc.textures.retain(|(page, kind), _| *kind == Kind::Main || (page + 40 >= lo && *page <= hi + 40));
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        match self.side {
            Side::None => {}
            Side::Search => {
                ui.heading(t("side.search"));
                if !self.searched {
                    ui.label(t("side.search.prompt"));
                } else if self.hits.is_empty() {
                    ui.label(t("side.no_results"));
                } else {
                    ui.label(tf("side.results", &[("n", &self.hits.len().to_string())]));
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for hit in &self.hits {
                        let text = format!("{}: {}", tf("page.abbr", &[("n", &(hit.page + 1).to_string())]), hit.snippet);
                        if ui.add(egui::Button::new(text).wrap().frame(false)).clicked() {
                            actions.push(Action::GoTo(hit.page));
                        }
                        ui.separator();
                    }
                });
            }
            Side::Comments => {
                ui.heading(t("menu.comments"));
                let Some(doc) = &self.doc else { return };
                if doc.annots.is_empty() {
                    ui.label(t("side.no_comments"));
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for annot in &doc.annots {
                        ui.horizontal(|ui| {
                            if ui.link(format!("{} · {}", tf("page.abbr", &[("n", &(annot.page + 1).to_string())]), annot.kind)).clicked() {
                                actions.push(Action::GoTo(annot.page));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(t("side.delete")).clicked() {
                                    actions.push(Action::DeleteAnnot(annot.page, annot.id));
                                }
                            });
                        });
                        if !annot.contents.is_empty() {
                            ui.label(&annot.contents);
                        }
                        ui.separator();
                    }
                });
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading(egui::RichText::new("Perkament").size(40.0));
            ui.add_space(4.0);
            ui.label(t("welcome.tagline"));
            ui.add_space(18.0);
            if ui.add(egui::Button::new(egui::RichText::new(t("welcome.open")).size(16.0))).clicked() {
                actions.push(Action::Open);
            }
            ui.add_space(8.0);
            ui.weak(t("welcome.drop"));
        });
    }

    fn pages(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(doc) = &mut self.doc else { return };
        if !doc.geoms_ready || doc.geoms.is_empty() {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
        let avail = ui.available_size();
        let max_w = doc.geoms.iter().map(|g| g.w).fold(1.0, f32::max);
        if self.fit_width {
            let fitted = ((avail.x - 2.0 * PAGE_GAP - 16.0) / max_w).clamp(0.1, 8.0);
            if (fitted - self.zoom).abs() > 0.001 {
                self.zoom = fitted;
                self.zoom_changed = Instant::now();
            }
        }
        let pinch = ui.input(|i| i.zoom_delta());
        if pinch != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.zoom = (self.zoom * pinch).clamp(0.1, 8.0);
            self.fit_width = false;
            self.zoom_changed = Instant::now();
        }
        let zoom = self.zoom;
        let settled = self.zoom_changed.elapsed().as_secs_f32() > 0.2;
        if !settled {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(220));
        }

        // Indeling: pagina's onder elkaar, horizontaal gecentreerd.
        let content_w = (max_w * zoom + 2.0 * PAGE_GAP).max(avail.x - 16.0);
        let mut y = PAGE_GAP;
        let rects: Vec<Rect> = doc
            .geoms
            .iter()
            .map(|g| {
                let size = Vec2::new(g.w, g.h) * zoom;
                let r = Rect::from_min_size(Pos2::new((content_w - size.x) / 2.0, y), size);
                y += size.y + PAGE_GAP;
                r
            })
            .collect();
        let content = Vec2::new(content_w, y);

        let mut area = egui::ScrollArea::both().id_salt("pages").auto_shrink([false, false]);
        // Na een wijziging loopt de indeling één antwoord van de renderer achter;
        // tot die binnen is niet springen en geen gereedschap gebruiken.
        let fresh = doc.geoms_generation == doc.generation;
        let mut jumped = false;
        if fresh {
            if let Some(page) = self.scroll_to.take() {
                if let Some(r) = rects.get(page) {
                    area = area.vertical_scroll_offset((r.min.y - PAGE_GAP / 2.0).max(0.0));
                    doc.current = page;
                    self.thumb_follow = Some(page);
                    self.nav = Some((page, 0, 0.0));
                    jumped = true;
                }
            }
        }
        if !jumped && (zoom - self.layout_zoom).abs() > 1e-4 {
            // De zoom is veranderd: zet dezelfde plek van dezelfde pagina weer bovenaan.
            if let Some(r) = self.anchor.and_then(|(page, _)| rects.get(page)) {
                let frac = self.anchor.map_or(0.0, |(_, frac)| frac);
                area = area.vertical_scroll_offset((r.min.y + frac * r.height()).max(0.0));
            }
        }
        self.layout_zoom = zoom;
        let tool = if fresh { self.tool } else { Tool::Hand };
        let color = self.color;
        let ppp = ui.ctx().pixels_per_point();
        let sense = if tool == Tool::Hand { Sense::hover() } else { Sense::click_and_drag() };

        area.show_viewport(ui, |ui, viewport| {
            let (outer, response) = ui.allocate_exact_size(content, sense);
            let origin = outer.min.to_vec2();
            let painter = ui.painter_at(outer);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

            let mut visible: Vec<usize> = Vec::new();
            for (i, local) in rects.iter().enumerate() {
                if !viewport.expand(300.0).intersects(*local) {
                    continue;
                }
                visible.push(i);
                let r = local.translate(origin);
                painter.rect_filled(r.translate(Vec2::new(2.0, 3.0)), 0.0, Color32::from_black_alpha(60));
                painter.rect_filled(r, 0.0, Color32::WHITE);
                Self::request_texture(&self.renderer, doc, i, Kind::Main, zoom * ppp, settled);
                match doc.textures.get(&(i, Kind::Main)).or_else(|| doc.textures.get(&(i, Kind::Thumb))) {
                    Some(tex) => {
                        painter.image(tex.handle.id(), r, uv, Color32::WHITE);
                    }
                    None => {
                        let message = if doc.failed.contains(&i) { t("view.failed") } else { t("view.rendering") };
                        painter.text(r.center(), Align2::CENTER_CENTER, message, FontId::proportional(14.0), Color32::GRAY);
                    }
                }
            }

            // Texturen van pagina's ver buiten beeld vrijgeven, en de renderer
            // niet laten doorwerken aan pagina's waar al voorbij is gescrold.
            if let (Some(&lo), Some(&hi)) = (visible.first(), visible.last()) {
                doc.textures.retain(|(page, kind), _| *kind == Kind::Thumb || (page + 2 >= lo && *page <= hi + 2));
            }
            if visible != self.kept {
                self.renderer.keep_main(&visible);
                doc.pending.retain(|(page, kind), _| *kind == Kind::Thumb || visible.contains(page));
                self.kept = visible;
            }

            // Onthoud wat bovenaan in beeld staat, voor als de zoom verandert.
            if let Some(top) = rects.iter().position(|r| r.max.y > viewport.min.y) {
                let r = rects[top];
                self.anchor = Some((top, (viewport.min.y - r.min.y) / r.height().max(1.0)));
            }

            // Huidige pagina: de expliciet gekozen pagina zolang er niet is gescrold,
            // anders de pagina die het grootste deel van het venster vult.
            let chosen = match &mut self.nav {
                Some((page, frames, y)) if *frames < 2 => {
                    *frames += 1;
                    *y = viewport.min.y;
                    Some(*page)
                }
                Some((page, _, y)) if (viewport.min.y - *y).abs() <= 2.0 => Some(*page),
                _ => None,
            };
            if chosen.is_none() {
                self.nav = None;
            }
            let overlap = |r: &Rect| (r.max.y.min(viewport.max.y) - r.min.y.max(viewport.min.y)).max(0.0);
            let mut most_visible = 0;
            for (i, r) in rects.iter().enumerate() {
                if overlap(r) > overlap(&rects[most_visible]) + 0.5 {
                    most_visible = i;
                }
            }
            let current = chosen.filter(|page| *page < rects.len()).unwrap_or(most_visible);
            if current != doc.current {
                doc.current = current;
                self.thumb_follow = Some(current);
            }

            if tool == Tool::Hand {
                self.drag = Drag::None;
                return;
            }
            if tool == Tool::EditText {
                self.drag = Drag::None;
                let accent = ui.visuals().selection.bg_fill;
                let hover = response.hover_pos();
                let mut clicked = None;
                for (i, local) in rects.iter().enumerate() {
                    if !viewport.intersects(*local) {
                        continue;
                    }
                    let g = doc.geoms[i];
                    let page_origin = local.translate(origin).min;
                    let lines = doc
                        .lines
                        .entry(i)
                        .or_insert_with(|| textedit::text_lines(&doc.pdf, i).unwrap_or_default());
                    for line in lines.iter() {
                        let points: Vec<Pos2> = line
                            .quad
                            .iter()
                            .map(|&(x, y)| {
                                let (vx, vy) = g.pdf_to_view(x, y);
                                page_origin + Vec2::new(vx, vy) * zoom
                            })
                            .collect();
                        let hovered = hover.is_some_and(|p| Rect::from_points(&points).expand(1.0).contains(p));
                        let stroke = Stroke::new(if hovered { 2.0 } else { 1.0 }, accent.gamma_multiply(if hovered { 1.0 } else { 0.45 }));
                        if hovered {
                            painter.add(egui::Shape::convex_polygon(points.clone(), accent.gamma_multiply(0.15), Stroke::NONE));
                            if response.clicked() {
                                clicked = Some((i, line.clone()));
                            }
                        }
                        painter.add(egui::Shape::closed_line(points, stroke));
                    }
                }
                if hover.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }
                if let Some((page, line)) = clicked {
                    let text = line.text.clone();
                    self.dialog = Some(Dialog::EditText { page, line, text });
                }
                return;
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let page_at = |pos: Pos2| rects.iter().position(|r| r.translate(origin).contains(pos));
            // Schermpositie -> paginaruimte (zonder zoom), begrensd tot de pagina.
            let to_page = |page: usize, pos: Pos2| -> Pos2 {
                let r = rects[page].translate(origin);
                let p = pos.clamp(r.min, r.max);
                ((p - r.min) / zoom).to_pos2()
            };
            let to_screen = |page: usize, p: Pos2| rects[page].translate(origin).min + p.to_vec2() * zoom;
            let pointer = response.interact_pointer_pos();

            if let Drag::Rect { page, .. } | Drag::Ink { page, .. } = &self.drag {
                if *page >= rects.len() || *page >= doc.geoms.len() {
                    self.drag = Drag::None;
                }
            }
            if response.drag_started() {
                // egui meldt het slepen pas na een paar pixels; begin waar de knop is ingedrukt.
                let pressed = ui.input(|i| i.pointer.press_origin()).or(pointer);
                if let Some((page, pos)) = pressed.and_then(|p| page_at(p).map(|page| (page, p))) {
                    let start = to_page(page, pos);
                    self.drag = match tool {
                        Tool::Ink => Drag::Ink { page, points: vec![start] },
                        Tool::Note => Drag::None,
                        _ => Drag::Rect { page, start, now: start },
                    };
                }
            }
            if response.dragged() {
                if let Some(pos) = pointer {
                    match &mut self.drag {
                        Drag::Rect { page, now, .. } => *now = to_page(*page, pos),
                        Drag::Ink { page, points } => {
                            let p = to_page(*page, pos);
                            if points.last().is_none_or(|l| l.distance(p) > 1.0 / zoom) {
                                points.push(p);
                            }
                        }
                        Drag::None => {}
                    }
                }
            }
            let stroke_color = Color32::from_rgb(
                (color[0] * 255.0) as u8,
                (color[1] * 255.0) as u8,
                (color[2] * 255.0) as u8,
            );
            match &self.drag {
                Drag::Rect { page, start, now } => {
                    let r = Rect::from_two_pos(to_screen(*page, *start), to_screen(*page, *now));
                    if tool == Tool::Highlight {
                        painter.rect_filled(r, 0.0, stroke_color.gamma_multiply(0.4));
                    } else {
                        painter.rect_stroke(r, 0.0, Stroke::new(2.0, stroke_color), StrokeKind::Inside);
                    }
                }
                Drag::Ink { page, points } => {
                    let line: Vec<Pos2> = points.iter().map(|p| to_screen(*page, *p)).collect();
                    painter.line(line, Stroke::new(2.0 * zoom, stroke_color));
                }
                Drag::None => {}
            }
            if response.drag_stopped() {
                match std::mem::replace(&mut self.drag, Drag::None) {
                    Drag::Rect { page, start, now } => {
                        let g = doc.geoms[page];
                        let a = g.view_to_pdf(start.x, start.y);
                        let b = g.view_to_pdf(now.x, now.y);
                        let rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
                        let kind = match tool {
                            Tool::Highlight => AnnotKind::Highlight,
                            Tool::Rectangle => AnnotKind::Rectangle,
                            _ => AnnotKind::FreeText,
                        };
                        if kind == AnnotKind::FreeText {
                            self.dialog = Some(Dialog::Text { page, kind, rect, text: String::new() });
                        } else if rect[2] - rect[0] > 2.0 && rect[3] - rect[1] > 2.0 {
                            let annot = NewAnnot { kind, rect, color, text: String::new(), points: vec![] };
                            actions.push(Action::AddAnnot(page, annot));
                        }
                    }
                    Drag::Ink { page, points } if points.len() > 1 => {
                        let g = doc.geoms[page];
                        let points = points.iter().map(|p| g.view_to_pdf(p.x, p.y)).collect();
                        let annot =
                            NewAnnot { kind: AnnotKind::Ink, rect: [0.0; 4], color, text: String::new(), points };
                        actions.push(Action::AddAnnot(page, annot));
                    }
                    _ => {}
                }
            }
            if response.clicked() && matches!(tool, Tool::Note | Tool::FreeText) {
                if let Some((page, pos)) = pointer.and_then(|p| page_at(p).map(|page| (page, p))) {
                    let p = to_page(page, pos);
                    let (x, y) = doc.geoms[page].view_to_pdf(p.x, p.y);
                    let kind = if tool == Tool::Note { AnnotKind::Note } else { AnnotKind::FreeText };
                    self.dialog = Some(Dialog::Text { page, kind, rect: [x, y, x, y], text: String::new() });
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let n = self.page_count();
        let color = self.color;
        let keep;
        let window = |title: &str, add: &mut dyn FnMut(&mut egui::Ui) -> bool| {
            let mut close = false;
            // Modaal: wat eronder ligt reageert niet, dus een klik ernaast gooit geen
            // invoer weg en het document kan onder de dialoog niet veranderen.
            egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
                ui.heading(title);
                ui.add_space(8.0);
                close = add(ui);
            });
            !close && !ctx.input(|i| i.key_pressed(Key::Escape))
        };
        // Elke dialoog geeft `true` terug als hij moet sluiten.
        match &mut dialog {
            Dialog::Properties(meta) => {
                keep = window(t("dlg.properties"), &mut |ui| {
                    egui::Grid::new("meta").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        for (label, field) in [
                            (t("meta.title"), &mut meta.title),
                            (t("meta.author"), &mut meta.author),
                            (t("meta.subject"), &mut meta.subject),
                            (t("meta.keywords"), &mut meta.keywords),
                            (t("meta.creator"), &mut meta.creator),
                        ] {
                            ui.label(label);
                            ui.add(egui::TextEdit::singleline(field).desired_width(320.0));
                            ui.end_row();
                        }
                        ui.label(t("meta.pages"));
                        ui.label(n.to_string());
                        ui.end_row();
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.apply")).clicked() {
                            actions.push(Action::SetMeta(meta.clone()));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::Watermark(wm) => {
                keep = window(t("dlg.watermark"), &mut |ui| {
                    egui::Grid::new("wm").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label(t("wm.text"));
                        ui.add(egui::TextEdit::singleline(&mut wm.text).desired_width(260.0));
                        ui.end_row();
                        ui.label(t("wm.size"));
                        ui.add(egui::Slider::new(&mut wm.size, 12.0..=160.0).suffix(" pt"));
                        ui.end_row();
                        ui.label(t("wm.opacity"));
                        ui.add(egui::Slider::new(&mut wm.opacity, 0.05..=1.0));
                        ui.end_row();
                        ui.label(t("wm.color"));
                        ui.color_edit_button_rgb(&mut wm.color);
                        ui.end_row();
                        ui.label(t("wm.direction"));
                        ui.checkbox(&mut wm.diagonal, t("wm.diagonal"));
                        ui.end_row();
                    });
                    ui.weak(t("wm.all_pages"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.add")).clicked() {
                            actions.push(Action::AddWatermark(wm.clone()));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::PageNumbers(format) => {
                keep = window(t("dlg.page_numbers"), &mut |ui| {
                    ui.label(t("pn.format"));
                    ui.add(egui::TextEdit::singleline(format).desired_width(280.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.add")).clicked() {
                            actions.push(Action::AddPageNumbers(format.clone()));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::Extract { from, to } => {
                keep = window(t("dlg.extract"), &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.label(t("ex.from"));
                        ui.add(egui::DragValue::new(from).range(1..=n));
                        ui.label(t("ex.to"));
                        ui.add(egui::DragValue::new(to).range(1..=n));
                    });
                    *to = (*to).max(*from);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.save_as")).clicked() {
                            actions.push(Action::Extract(*from, *to));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::Text { page, kind, rect, text } => {
                let title = if *kind == AnnotKind::Note { t("dlg.note") } else { t("dlg.textbox") };
                keep = window(title, &mut |ui| {
                    let edit = ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_width(340.0)
                            .desired_rows(4)
                            .hint_text(t("dlg.text.hint")),
                    );
                    if !edit.has_focus() && text.is_empty() {
                        edit.request_focus();
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!text.trim().is_empty(), egui::Button::new(t("btn.place"))).clicked() {
                            let annot = NewAnnot {
                                kind: *kind,
                                rect: *rect,
                                color,
                                text: text.trim().to_string(),
                                points: vec![],
                            };
                            actions.push(Action::AddAnnot(*page, annot));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::EditText { page, line, text } => {
                keep = window(t("dlg.edit_text"), &mut |ui| {
                    let edit = ui.add(egui::TextEdit::singleline(text).desired_width(460.0));
                    if !edit.has_focus() && *text == line.text {
                        edit.request_focus();
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.weak(t("edit.hint"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.apply")).clicked() || enter {
                            if *text != line.text {
                                actions.push(Action::ReplaceText(*page, line.clone(), text.clone()));
                            }
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::Unsaved(after) => {
                keep = window(t("dlg.unsaved"), &mut |ui| {
                    ui.label(t("unsaved.body"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("btn.save")).clicked() {
                            actions.push(Action::SaveThen(after.clone()));
                            return true;
                        }
                        if ui.button(t("btn.dont_save")).clicked() {
                            actions.push(Action::Proceed(after.clone()));
                            return true;
                        }
                        ui.button(t("btn.cancel")).clicked()
                    })
                    .inner
                });
            }
            Dialog::Error(message) => {
                keep = window(t("dlg.error"), &mut |ui| {
                    ui.set_max_width(420.0);
                    ui.label(message.as_str());
                    ui.add_space(8.0);
                    ui.button(t("btn.ok")).clicked()
                });
            }
            Dialog::About => {
                keep = window(t("menu.about"), &mut |ui| {
                    ui.heading("Perkament");
                    ui.label(tf("about.version", &[("v", env!("CARGO_PKG_VERSION"))]));
                    ui.label(t("about.body"));
                    ui.add_space(8.0);
                    ui.button(t("btn.close")).clicked()
                });
            }
        }
        // Een actie kan zelf een nieuwe dialoog openen; die niet overschrijven.
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        // Een pagina waar de renderer niet uitkomt mag de rest niet ophouden.
        if let Some((generation, page, kind)) = self.renderer.check_stuck(RENDER_TIMEOUT) {
            if let Some(doc) = self.doc.as_mut().filter(|d| d.generation == generation) {
                doc.failed.insert(page);
                doc.pending.remove(&(page, kind));
            }
        }
        while let Some(response) = self.renderer.try_recv() {
            let Some(doc) = &mut self.doc else { continue };
            match response {
                Response::Loaded { generation, pages } if generation == doc.generation => match pages {
                    Ok(geoms) => {
                        doc.current = doc.current.min(geoms.len().saturating_sub(1));
                        doc.geoms = geoms;
                        doc.geoms_ready = true;
                        doc.geoms_generation = generation;
                    }
                    Err(e) => {
                        if self.dialog.is_none() {
                            self.dialog = Some(Dialog::Error(tf("err.display", &[("e", &e)])));
                        }
                    }
                },
                Response::Image { generation, page, kind, scale, size, rgba } if generation == doc.generation => {
                    let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
                    let handle = ctx.load_texture(format!("p{page}-{kind:?}"), image, egui::TextureOptions::LINEAR);
                    doc.textures.insert((page, kind), Tex { scale, handle });
                    if doc.pending.get(&(page, kind)) == Some(&scale) {
                        doc.pending.remove(&(page, kind));
                    }
                }
                Response::Failed { generation, page, kind } if generation == doc.generation => {
                    doc.failed.insert(page);
                    doc.pending.remove(&(page, kind));
                }
                _ => {}
            }
        }
        // Zolang er iets wordt gerenderd regelmatig terugkomen, zodat de bewaker kan ingrijpen.
        if self.doc.as_ref().is_some_and(|d| !d.pending.is_empty()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

impl eframe::App for PerkamentApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();

        if let Some(path) = self.initial.take() {
            self.open_path(&path);
        }
        self.receive(&ctx);

        // Venster sluiten met niet-opgeslagen wijzigingen: eerst vragen.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.force_quit
            && self.doc.as_ref().is_some_and(|d| d.dirty())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.dialog.is_none() {
                self.dialog = Some(Dialog::Unsaved(After::Quit));
            }
        }
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| Some(f.path().to_path_buf()))) {
            actions.push(Action::OpenPath(path));
        }
        if self.dialog.is_none() {
            self.shortcuts(&ctx, &mut actions);
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui, &mut actions);
            ui.add_space(3.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(doc) = &self.doc {
                        let page = tf(
                            "st.page_of",
                            &[("n", &(doc.current + 1).to_string()), ("N", &pdfops::page_count(&doc.pdf).to_string())],
                        );
                        ui.label(format!("{page}  ·  {:.0}%", self.zoom * 100.0));
                    }
                });
            });
        });
        if self.doc.is_some() && self.show_thumbs {
            egui::Panel::left("thumbs")
                .resizable(false)
                .exact_size(THUMB_W + 40.0)
                .show(ui, |ui| self.thumbnails(ui, &mut actions));
        }
        if self.doc.is_some() && self.side != Side::None {
            egui::Panel::right("side")
                .default_size(280.0)
                .size_range(220.0..=480.0)
                .show(ui, |ui| self.side_panel(ui, &mut actions));
        }
        let background = if ui.visuals().dark_mode { Color32::from_gray(38) } else { Color32::from_gray(205) };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(background))
            .show(ui, |ui| {
                if self.doc.is_some() {
                    self.pages(ui, &mut actions);
                } else {
                    self.welcome(ui, &mut actions);
                }
            });

        self.dialogs(&ctx, &mut actions);
        for action in actions {
            self.apply(&ctx, action);
        }

        let title = match &self.doc {
            Some(doc) => format!("{}{} - {}", if doc.dirty() { "• " } else { "" }, doc.name(), pdfops::APP_NAME),
            None => pdfops::APP_NAME.to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}
