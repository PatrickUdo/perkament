//! Voert alle bewerkingen van de app uit op één bestand, voor tests met kwaadaardige PDF's.
use perkament::{excel, pdfops, render, textedit};
use std::time::{Duration, Instant};

fn step<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let rss = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().unwrap_or(0) / 1024).unwrap_or(0);
    eprintln!("  {name}: {:?} rss={rss}MB", start.elapsed());
    out
}

fn main() {
    // 1 MB stack, zoals de hoofdthread op Windows.
    std::thread::Builder::new().stack_size(1 << 20).spawn(run).unwrap().join().unwrap();
}

fn run() {
    let path = std::env::args().nth(1).expect("bestand");
    let bytes = std::fs::read(&path).unwrap();
    let mut doc = match step("load", || pdfops::load(&bytes)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  load error (ok): {e:#}");
            Default::default()
        }
    };
    step("repair", || textedit::repair_overlay_colors(&mut doc));
    let n = step("page_count", || pdfops::page_count(&doc));
    step("annots", || pdfops::list_annotations(&doc).len());
    step("meta", || pdfops::get_meta(&doc).title.len());
    let pages = n.min(5);
    step("page_text", || (0..pages).map(|i| pdfops::page_text(&doc, i).len()).sum::<usize>());
    for i in 0..pages {
        let lines = step("text_lines", || textedit::text_lines(&doc, i).unwrap_or_default());
        if let Some(line) = lines.first() {
            let _ = step("replace_line", || textedit::replace_line(&mut doc, i, line, "Test $ 12,00"));
        }
        step("excel", || excel::page_table(&doc, i).len());
    }
    let _ = step("save", || pdfops::to_bytes_for_saving(&doc));
    let saved = step("render_bytes", || pdfops::to_render_bytes(&mut doc, &mut Default::default()));
    if let Ok(saved) = saved {
        for i in 0..pages {
            let r = step("render", || render::render_png_with_timeout(saved.clone(), i, 1.0, Duration::from_secs(20)));
            if let Err(e) = r {
                eprintln!("  render error (ok): {e}");
            }
        }
    }
    eprintln!("  DONE");
}
