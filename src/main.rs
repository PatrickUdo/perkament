// In een release-build op Windows geen consolevenster naast de app openen.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod settings;
mod updater;

use eframe::egui;
use perkament::i18n::{self, t};
use perkament::{pdfops, render};
use std::path::PathBuf;

fn main() -> eframe::Result {
    // Volgorde: PERKAMENT_LANG (handig bij testen), dan de bewaarde keuze, dan de systeemtaal.
    let forced = std::env::var("PERKAMENT_LANG").ok().and_then(|code| i18n::Lang::from_code(&code));
    i18n::set(forced.or_else(settings::load_language).unwrap_or_else(i18n::system_language));
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Headless: `perkament --render in.pdf uitvoermap [schaal]`
    if args.first().map(String::as_str) == Some("--render") {
        if let Err(e) = render_cli(&args[1..]) {
            eprintln!("{}: {e}", t("err.prefix"));
            std::process::exit(1);
        }
        return Ok(());
    }

    // Headless: `perkament --check-update` meldt of er een nieuwere release is.
    if args.first().map(String::as_str) == Some("--check-update") {
        match updater::check() {
            Ok(Some(update)) => println!("build {}: {} (signed: {})", update.build, update.name, update.signed()),
            Ok(None) => println!("{}", t("upd.up_to_date")),
            Err(e) => {
                eprintln!("{}: {e}", t("err.prefix"));
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    let initial = args.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(pdfops::APP_NAME)
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        // Het standaardmenu van macOS bevat een "Stop" die het proces direct beëindigt,
        // zonder dat de app naar niet-opgeslagen wijzigingen kan vragen. Wij handelen
        // Cmd+Q daarom zelf af.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
        ..Default::default()
    };
    eframe::run_native(
        pdfops::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::PerkamentApp::new(cc, initial)))),
    )
}

fn render_cli(args: &[String]) -> Result<(), String> {
    let [input, out_dir, rest @ ..] = args else {
        return Err(t("err.usage").into());
    };
    let scale: f32 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let mut doc = pdfops::load(&bytes).map_err(|e| e.to_string())?;
    let bytes = pdfops::to_bytes(&mut doc).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    for page in 0..pdfops::page_count(&doc) {
        let png = render::render_png(bytes.clone(), page, scale)?;
        let path = PathBuf::from(out_dir).join(format!("pagina-{:03}.png", page + 1));
        std::fs::write(&path, png).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    Ok(())
}
