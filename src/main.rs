// In een release-build op Windows geen consolevenster naast de app openen.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod diag;
mod imagedrag;
mod settings;
mod updater;

use eframe::egui;
use perkament::i18n::{self, t};
use perkament::{pdfops, render};
use std::path::PathBuf;
use std::sync::Arc;

/// Stelt de grafische aansturing in.
///
/// Op Windows alleen DirectX 12: het opstarten van Vulkan laadt alle stuurprogramma's en
/// overlay-lagen van de machine, en dat crasht op sommige kantoorlaptops af en toe nog
/// voor er een venster is. De zuinige grafische kaart is voor een PDF-programma ruim
/// genoeg en voorkomt het omschakelen naar een tweede kaart.
fn configure_graphics(options: &mut eframe::NativeOptions, level: u8) {
    use eframe::egui_wgpu::WgpuSetup;
    use eframe::wgpu;
    let WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup else { return };
    // De omgevingsvariabelen van wgpu blijven voorgaan, voor wie iets anders wil proberen.
    if cfg!(windows) && std::env::var_os("WGPU_BACKEND").is_none() {
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
    }
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        setup.power_preference = wgpu::PowerPreference::LowPower;
    }
    if level >= diag::SOFTWARE {
        // Na een mislukte start: de softwarematige weergave (op Windows altijd aanwezig),
        // die geen stuurprogramma van de grafische kaart gebruikt.
        setup.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
            adapters
                .iter()
                .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
                .or_else(|| adapters.first())
                .cloned()
                .ok_or_else(|| "no graphics adapter found".to_string())
        }));
    }
}

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

    // Headless: `perkament --repair in.pdf uit.pdf` herstelt bestanden met de kleurfout van eerdere versies.
    if args.first().map(String::as_str) == Some("--repair") {
        let result = (|| -> anyhow::Result<usize> {
            let [_, input, output] = &args[..] else { anyhow::bail!("{}", t("err.usage")) };
            let mut doc = pdfops::load(&std::fs::read(input)?)?;
            let fixed = perkament::textedit::repair_overlay_colors(&mut doc);
            std::fs::write(output, pdfops::to_bytes_for_saving(&doc)?)?;
            Ok(fixed)
        })();
        match result {
            Ok(fixed) => println!("{fixed}"),
            Err(e) => {
                eprintln!("{}: {e:#}", t("err.prefix"));
                std::process::exit(1);
            }
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

    // Headless: `perkament --install-update` installeert de nieuwste release, als die nieuwer is.
    if args.first().map(String::as_str) == Some("--install-update") {
        match updater::install_latest() {
            Ok(Some(name)) => println!("{name}: {}", t("upd.ready")),
            Ok(None) => println!("{}", t("upd.up_to_date")),
            Err(e) => {
                eprintln!("{}: {e}", t("err.prefix"));
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    diag::install_panic_hook();
    diag::log(format!(
        "starting Perkament {} (build {}) on {} {}",
        env!("CARGO_PKG_VERSION"),
        updater::own_build().map_or("dev".to_string(), |b| b.to_string()),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let level = diag::begin_start();
    let initial = args.first().map(PathBuf::from);
    let mut options = eframe::NativeOptions {
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
    configure_graphics(&mut options, level);
    let result = eframe::run_native(
        pdfops::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::PerkamentApp::new(cc, initial)))),
    );
    if let Err(error) = &result {
        // Het venster kwam er niet. Eén keer opnieuw proberen met de softwarematige weergave.
        let retry = diag::start_failed(&error.to_string());
        if retry && std::env::var_os("PERKAMENT_RELAUNCHED").is_none() {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).args(&args).env("PERKAMENT_RELAUNCHED", "1").spawn();
            }
        }
    }
    result
}

fn render_cli(args: &[String]) -> Result<(), String> {
    let [input, out_dir, rest @ ..] = args else {
        return Err(t("err.usage").into());
    };
    let scale: f32 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let mut doc = pdfops::load(&bytes).map_err(|e| e.to_string())?;
    let bytes = pdfops::to_render_bytes(&mut doc, &mut Default::default()).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    for page in 0..pdfops::page_count(&doc) {
        let png = render::render_png(bytes.clone(), page, scale)?;
        let path = PathBuf::from(out_dir).join(format!("pagina-{:03}.png", page + 1));
        std::fs::write(&path, png).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    Ok(())
}
