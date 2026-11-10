//! Kleine, blijvende instellingen van de app (nu alleen de taal).

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
