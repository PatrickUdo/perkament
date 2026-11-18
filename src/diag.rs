//! Logboek en opstartbewaking.
//!
//! Een harde crash in een grafisch stuurprogramma laat geen spoor na en is
//! vanuit de app niet op te vangen. Daarom zetten we bij het opstarten een
//! markering neer die pas weggaat als de eerste beelden getekend zijn. Vindt
//! een volgende start die markering nog, dan is de vorige start mislukt en
//! schakelen we over op de softwarematige weergave, die geen stuurprogramma
//! van de grafische kaart nodig heeft.
//!
//! Het logboek staat in de instellingenmap als `perkament.log`.

use crate::settings::{config_dir, read, write};
use std::io::Write;

const LOG: &str = "perkament.log";
const MARKER: &str = "starting";
const LEVEL: &str = "graphics";
const MAX_LOG: u64 = 256 * 1024;
/// Een markering jonger dan dit (seconden) is van een start die nog bezig is,
/// bijvoorbeeld als iemand twee keer snel achter elkaar op het pictogram klikt.
const STILL_STARTING: u64 = 20;

/// Niveau 0 gebruikt de grafische kaart; dit niveau de softwarematige weergave.
pub const SOFTWARE: u8 = 1;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Datum en tijd in UTC (burgerlijke datum volgens het algoritme van Howard Hinnant).
fn stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let (era, doe) = (z.div_euclid(146097), z.rem_euclid(146097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Voegt een regel toe aan het logboek. Mislukt dat, dan gaat de app gewoon door.
pub fn log(message: impl AsRef<str>) {
    let Some(dir) = config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(LOG);
    // Niet eindeloos laten groeien: bij de grens opnieuw beginnen.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} [{}] {}", stamp(now()), std::process::id(), message.as_ref());
    }
}

/// Zorgt dat een paniek in het logboek komt in plaats van spoorloos te verdwijnen
/// (een Windows-release heeft geen console).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log(format!("PANIC in thread '{}': {info}", thread.name().unwrap_or("?")));
        previous(info);
    }));
}

fn build() -> u64 {
    crate::updater::own_build().unwrap_or(0)
}

/// Het bewaarde niveau geldt alleen voor de build waarin het is vastgelegd:
/// een nieuwe versie probeert de grafische kaart opnieuw.
fn stored_level() -> u8 {
    let Some(text) = read(LEVEL) else { return 0 };
    let mut parts = text.split_whitespace();
    let level = parts.next().and_then(|p| p.parse::<u8>().ok()).unwrap_or(0);
    let same_build = parts.next().and_then(|p| p.parse::<u64>().ok()) == Some(build());
    if same_build { level.min(SOFTWARE) } else { 0 }
}

fn store_level(level: u8) {
    write(LEVEL, &format!("{level} {}", build()));
}

/// Wat te doen bij een start: het niveau om te gebruiken, en of de markering
/// (opnieuw) gezet moet worden. `marker_age` is de leeftijd van een gevonden markering.
fn decide(level: u8, marker_age: Option<u64>) -> (u8, bool) {
    match marker_age {
        // Geen markering: de vorige start is gelukt.
        None => (level, true),
        // Een andere start is nog bezig: niets concluderen en zijn markering laten staan.
        Some(age) if age <= STILL_STARTING => (level, false),
        // De vorige start heeft het eerste beeld niet gehaald.
        Some(_) => ((level + 1).min(SOFTWARE), true),
    }
}

/// Aan het begin van een start met venster. Geeft het grafische niveau terug.
pub fn begin_start() -> u8 {
    let stored = stored_level();
    let age = read(MARKER).map(|text| now().saturating_sub(text.parse::<u64>().unwrap_or(0)));
    let (level, set_marker) = decide(stored, age);
    if level != stored {
        store_level(level);
        log("the previous start did not reach the first frame; switching to software rendering");
    } else if age.is_some() && set_marker {
        log("the previous start did not reach the first frame (already on software rendering)");
    }
    if set_marker {
        write(MARKER, &now().to_string());
    }
    level
}

/// De eerste beelden zijn getekend: de start is gelukt.
pub fn start_completed() {
    if let Some(dir) = config_dir() {
        let _ = std::fs::remove_file(dir.join(MARKER));
    }
    log("start completed");
}

/// Het venster kon niet worden opgezet. Geeft `true` als een volgende poging
/// met de softwarematige weergave zin heeft.
pub fn start_failed(error: &str) -> bool {
    log(format!("could not start the window: {error}"));
    if let Some(dir) = config_dir() {
        let _ = std::fs::remove_file(dir.join(MARKER));
    }
    if stored_level() < SOFTWARE {
        store_level(SOFTWARE);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_decisions() {
        // Normale start.
        assert_eq!(decide(0, None), (0, true));
        // Vorige start is gecrasht voor het eerste beeld: overschakelen.
        assert_eq!(decide(0, Some(3600)), (SOFTWARE, true));
        // Dubbelklik: de eerste start is nog bezig, dus niets concluderen.
        assert_eq!(decide(0, Some(2)), (0, false));
        // Al op software en toch mislukt: niet verder dan het hoogste niveau.
        assert_eq!(decide(SOFTWARE, Some(3600)), (SOFTWARE, true));
        assert_eq!(decide(SOFTWARE, None), (SOFTWARE, true));
    }

    #[test]
    fn timestamps() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp(1_794_306_153), "2026-11-10 10:22:33Z");
        // Schrikkeldag.
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00:00Z");
    }
}
