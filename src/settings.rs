//! Kleine, blijvende instellingen van de app: de taal en het controleren op updates.

use perkament::i18n::Lang;
use std::path::PathBuf;

/// Map voor instellingen, volgens de gewoonte van het platform.
fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Application Support/Perkament"))
    } else if cfg!(windows) {
        var("APPDATA").map(|dir| dir.join("Perkament"))
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|home| home.join(".config")))
            .map(|dir| dir.join("perkament"))
    }
}

fn language_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("language"))
}

/// De taal die de gebruiker eerder koos, als die er is.
pub fn load_language() -> Option<Lang> {
    let text = std::fs::read_to_string(language_file()?).ok()?;
    Lang::from_code(text.trim())
}

/// Bewaart de taalkeuze. Mislukt dat (bv. alleen-lezen schijf), dan geldt de keuze alleen voor deze sessie.
pub fn save_language(lang: Lang) {
    if let Some(path) = language_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, lang.code());
    }
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(config_dir()?.join(name)).ok().map(|s| s.trim().to_string())
}

fn write(name: &str, value: &str) {
    if let Some(dir) = config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(name), value);
    }
}

/// Mag de app zelf op updates controleren? Standaard aan; de controle legt contact met GitHub.
pub fn auto_update() -> bool {
    read("auto-update").as_deref() != Some("0")
}

pub fn set_auto_update(on: bool) {
    write("auto-update", if on { "1" } else { "0" });
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Is de laatste automatische controle langer dan een dag geleden?
pub fn update_check_due() -> bool {
    let last = read("last-update-check").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    now().saturating_sub(last) > 24 * 3600
}

pub fn mark_update_checked() {
    write("last-update-check", &now().to_string());
}
