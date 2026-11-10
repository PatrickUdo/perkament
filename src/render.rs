//! Rasteren van pagina's met `hayro`, in een achtergrondthread.
//!
//! Gebruik vanuit de interface:
//! - [`spawn`] geeft een [`Renderer`]; [`Renderer::load`] stuurt een document,
//!   [`Renderer::request`] vraagt een pagina aan, [`Renderer::try_recv`] haalt
//!   antwoorden op.
//! - [`Renderer::keep_main`] laat aangevraagde pagina's vallen die niet meer in
//!   beeld zijn.
//! - Roep [`Renderer::check_stuck`] regelmatig aan. Sommige (kapotte) inhoud
//!   laat de renderbibliotheek eindeloos doorlopen; een thread is niet af te
//!   breken, dus dan wordt de vastgelopen thread losgelaten, een nieuwe gestart
//!   en de pagina als mislukt gemeld. De losgelaten thread blijft tot het
//!   afsluiten rekenen, maar houdt de rest niet meer op.

use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Grootste zijde (in pixels) die we laten rasteren; past ruim in een GPU-textuur.
const MAX_SIDE: f32 = 6000.0;
const MIN_SCALE: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Main,
    Thumb,
}

/// Afmetingen van een pagina (in punten, zoals getoond) en de matrix van
/// PDF-gebruikersruimte naar weergaveruimte (oorsprong linksboven) op schaal 1.
#[derive(Clone, Copy, Debug)]
pub struct PageGeom {
    pub w: f32,
    pub h: f32,
    pub to_view: [f64; 6],
}

impl PageGeom {
    pub fn pdf_to_view(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.to_view;
        let (x, y) = (x as f64, y as f64);
        ((a * x + c * y + e) as f32, (b * x + d * y + f) as f32)
    }

    pub fn view_to_pdf(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.to_view;
        let det = a * d - b * c;
        if !det.is_finite() || det.abs() < 1e-12 {
            return (x, y);
        }
        let (x, y) = (x as f64 - e, y as f64 - f);
        let (px, py) = (((d * x - c * y) / det) as f32, ((a * y - b * x) / det) as f32);
        if px.is_finite() && py.is_finite() { (px, py) } else { (0.0, 0.0) }
    }
}

enum Request {
    Load { generation: u64, bytes: Arc<Vec<u8>> },
    Render { generation: u64, page: usize, scale: f32, kind: Kind },
    /// Alleen deze pagina's van de hoofdweergave zijn nog gewenst.
    KeepMain(Vec<usize>),
}

pub enum Response {
    Loaded { generation: u64, pages: Result<Vec<PageGeom>, String> },
    Image { generation: u64, page: usize, kind: Kind, scale: f32, size: [usize; 2], rgba: Vec<u8> },
    /// De pagina kon niet worden weergegeven (vastgelopen, gecrasht of zonder zinnige afmetingen).
    Failed { generation: u64, page: usize, kind: Kind },
}

fn geometry(page: &Page<'_>) -> PageGeom {
    let (w, h) = page.render_dimensions();
    let sane = |v: f32| if v.is_finite() && v >= 1.0 { v } else { 1.0 };
    PageGeom { w: sane(w), h: sane(h), to_view: page.initial_transform(true).to_kurbo().as_coeffs() }
}

/// De schaal waarmee de pagina binnen de grenzen blijft, of `None` als dat niet kan.
fn safe_scale(w: f32, h: f32, wanted: f32) -> Option<f32> {
    if !(w.is_finite() && h.is_finite() && wanted.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    // Eerst de ondergrens, dan de bovengrens: de bovengrens wint altijd.
    let mut scale = wanted.max(MIN_SCALE).min(MAX_SIDE / w.max(h));
    // Geen zijde van nul pixels (de renderer kapt af naar hele pixels).
    scale = scale.max(1.001 / w.min(h));
    (w.max(h) * scale <= MAX_SIDE + 1.0).then_some(scale)
}

fn rasterize<'a>(page: &'a Page<'a>, cache: &RenderCache<'a>, scale: f32) -> Option<hayro::vello_cpu::Pixmap> {
    let (w, h) = page.render_dimensions();
    let scale = safe_scale(w, h, scale)?;
    let pixmap = hayro::render(
        page,
        cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE },
    );
    (pixmap.width() > 0 && pixmap.height() > 0).then_some(pixmap)
}

#[derive(Clone, Copy)]
struct Job {
    generation: u64,
    page: usize,
    kind: Kind,
    started: Instant,
}

#[derive(Default)]
struct WorkerState {
    current: Mutex<Option<Job>>,
    /// Gezet door de bewaker: deze thread mag niets meer versturen.
    abandoned: AtomicBool,
}

struct Worker {
    tx: Sender<Request>,
    state: Arc<WorkerState>,
}

type Notify = Arc<dyn Fn() + Send + Sync>;

fn start_worker(responses: Sender<Response>, notify: Notify) -> Worker {
    let (tx, rx) = channel::<Request>();
    let state = Arc::new(WorkerState::default());
    let shared = state.clone();
    std::thread::Builder::new()
        .name("render".into())
        .spawn(move || worker_loop(rx, responses, shared, notify))
        .expect("cannot start the render thread");
    Worker { tx, state }
}

fn worker_loop(rx: Receiver<Request>, responses: Sender<Response>, state: Arc<WorkerState>, notify: Notify) {
    let mut current: Option<(u64, Pdf)> = None;
    let mut queue: Vec<Request> = Vec::new();
    let send = |response: Response| {
        if state.abandoned.load(Ordering::SeqCst) {
            return false;
        }
        let ok = responses.send(response).is_ok();
        notify();
        ok
    };
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(r) => queue.push(r),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());

        // Het nieuwste document wint; oudere renderverzoeken vervallen.
        if let Some(pos) = queue.iter().rposition(|r| matches!(r, Request::Load { .. })) {
            let Request::Load { generation, bytes } = queue.remove(pos) else { unreachable!() };
            queue.retain(|r| matches!(r, Request::Render { generation: g, .. } if *g == generation));
            let pages = match catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes))) {
                Ok(Ok(pdf)) => {
                    let geoms = pdf.pages().iter().map(geometry).collect();
                    current = Some((generation, pdf));
                    Ok(geoms)
                }
                Ok(Err(e)) => {
                    current = None;
                    Err(format!("{e:?}"))
                }
                Err(_) => {
                    current = None;
                    Err("renderer panicked".into())
                }
            };
            if !send(Response::Loaded { generation, pages }) {
                return;
            }
            continue;
        }
        // Pagina's die uit beeld zijn geraakt hoeven niet meer.
        if let Some(pos) = queue.iter().rposition(|r| matches!(r, Request::KeepMain(_))) {
            let Request::KeepMain(keep) = queue.remove(pos) else { unreachable!() };
            queue.retain(|r| match r {
                Request::Render { kind: Kind::Main, page, .. } => keep.contains(page),
                Request::KeepMain(_) => false,
                _ => true,
            });
            continue;
        }

        let Some((doc_gen, pdf)) = &current else {
            queue.clear();
            continue;
        };
        // Het laatst gevraagde van de hoofdweergave eerst (daar kijkt de gebruiker naar), dan miniaturen.
        let pos = queue.iter().rposition(|r| matches!(r, Request::Render { kind: Kind::Main, .. })).unwrap_or(0);
        let Request::Render { generation, page, scale, kind } = queue.remove(pos) else { continue };
        let superseded = queue
            .iter()
            .any(|r| matches!(r, Request::Render { page: p, kind: k, .. } if *p == page && *k == kind));
        if generation != *doc_gen || superseded {
            continue;
        }
        let Some(p) = pdf.pages().get(page) else {
            if !send(Response::Failed { generation, page, kind }) {
                return;
            }
            continue;
        };
        *state.current.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Job { generation, page, kind, started: Instant::now() });
        // Een crash in de renderbibliotheek mag alleen deze pagina kosten.
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            let cache = RenderCache::new();
            rasterize(p, &cache, scale).map(|pixmap| {
                ([pixmap.width() as usize, pixmap.height() as usize], pixmap.data_as_u8_slice().to_vec())
            })
        }));
        *state.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let response = match rendered {
            Ok(Some((size, rgba))) => Response::Image { generation, page, kind, scale, size, rgba },
            _ => Response::Failed { generation, page, kind },
        };
        if !send(response) {
            return;
        }
    }
}

/// Handvat op de renderthread; zie de uitleg bovenaan dit bestand.
pub struct Renderer {
    worker: Worker,
    responses: Sender<Response>,
    incoming: Receiver<Response>,
    notify: Notify,
    last_load: Option<(u64, Arc<Vec<u8>>)>,
    /// Pagina's (per documentversie) die de renderer lieten vastlopen.
    poisoned: HashSet<(u64, usize)>,
}

/// Start de renderthread. `notify` wordt aangeroepen na elk antwoord.
pub fn spawn(notify: impl Fn() + Send + Sync + 'static) -> Renderer {
    let (responses, incoming) = channel::<Response>();
    let notify: Notify = Arc::new(notify);
    let worker = start_worker(responses.clone(), notify.clone());
    Renderer { worker, responses, incoming, notify, last_load: None, poisoned: HashSet::new() }
}

impl Renderer {
    pub fn load(&mut self, generation: u64, bytes: Vec<u8>) {
        let bytes = Arc::new(bytes);
        self.poisoned.clear();
        self.last_load = Some((generation, bytes.clone()));
        let _ = self.worker.tx.send(Request::Load { generation, bytes });
    }

    pub fn request(&self, generation: u64, page: usize, scale: f32, kind: Kind) {
        if self.poisoned.contains(&(generation, page)) {
            let _ = self.responses.send(Response::Failed { generation, page, kind });
            (self.notify)();
            return;
        }
        let _ = self.worker.tx.send(Request::Render { generation, page, scale, kind });
    }

    /// Laat wachtende verzoeken voor de hoofdweergave vallen, behalve voor deze pagina's.
    pub fn keep_main(&self, pages: &[usize]) {
        let _ = self.worker.tx.send(Request::KeepMain(pages.to_vec()));
    }

    pub fn try_recv(&self) -> Option<Response> {
        self.incoming.try_recv().ok()
    }

    /// Controleert of de renderthread langer dan `timeout` op één pagina zit.
    /// Zo ja: thread loslaten, nieuwe starten, document opnieuw laden en de
    /// pagina teruggeven; die wordt voor deze documentversie niet meer geprobeerd.
    pub fn check_stuck(&mut self, timeout: Duration) -> Option<(u64, usize, Kind)> {
        let job = (*self.worker.state.current.lock().unwrap_or_else(|e| e.into_inner()))?;
        if job.started.elapsed() < timeout {
            return None;
        }
        self.worker.state.abandoned.store(true, Ordering::SeqCst);
        self.worker = start_worker(self.responses.clone(), self.notify.clone());
        self.poisoned.insert((job.generation, job.page));
        if let Some((generation, bytes)) = &self.last_load {
            let _ = self.worker.tx.send(Request::Load { generation: *generation, bytes: bytes.clone() });
        }
        Some((job.generation, job.page, job.kind))
    }
}

/// Rendert één pagina synchroon naar PNG (voor de opdrachtregel en tests).
pub fn render_png(bytes: Vec<u8>, page: usize, scale: f32) -> Result<Vec<u8>, String> {
    let pdf = Pdf::new(bytes).map_err(|e| format!("{e:?}"))?;
    let p = pdf.pages().get(page).ok_or(crate::i18n::t("err.page_missing"))?;
    let cache = RenderCache::new();
    let pixmap = rasterize(p, &cache, scale).ok_or(crate::i18n::t("view.failed"))?;
    pixmap.into_png().map_err(|e| e.to_string())
}

/// Als [`render_png`], maar geeft op als het langer duurt dan `timeout`, zodat
/// de interface niet kan bevriezen op een pagina die de renderer laat vastlopen.
pub fn render_png_with_timeout(bytes: Vec<u8>, page: usize, scale: f32, timeout: Duration) -> Result<Vec<u8>, String> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| render_png(bytes, page, scale)));
            let _ = tx.send(result.unwrap_or_else(|_| Err(crate::i18n::t("view.failed").to_string())));
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(timeout).unwrap_or_else(|_| Err(crate::i18n::t("view.failed").to_string()))
}

pub fn page_geometries(bytes: Vec<u8>) -> Result<Vec<PageGeom>, String> {
    let pdf = Pdf::new(bytes).map_err(|e| format!("{e:?}"))?;
    Ok(pdf.pages().iter().map(geometry).collect())
}

#[cfg(test)]
mod tests {
    use super::safe_scale;

    #[test]
    fn scale_respects_both_limits() {
        // Een enorme pagina: de bovengrens wint van de minimale schaal.
        let s = safe_scale(1_000_000.0, 500_000.0, 1.0).unwrap();
        assert!(1_000_000.0 * s <= 6001.0, "{s}");
        // Een heel smalle pagina: geen zijde van nul pixels.
        let s = safe_scale(0.5, 800.0, 1.0).unwrap();
        assert!(0.5 * s >= 1.0 && 800.0 * s <= 6001.0, "{s}");
        // Te extreme verhouding of onzinnige maten: niet renderen.
        assert!(safe_scale(0.001, 100_000.0, 1.0).is_none());
        assert!(safe_scale(f32::NAN, 100.0, 1.0).is_none());
        assert!(safe_scale(100.0, 0.0, 1.0).is_none());
        assert!((safe_scale(595.0, 842.0, 2.0).unwrap() - 2.0).abs() < 1e-6);
    }
}
