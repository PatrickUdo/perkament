//! Ingebouwde updates via de GitHub-releases van het project.
//!
//! De release-workflow bakt het buildnummer in de app (`PERKAMENT_BUILD`) en
//! ondertekent elk releasebestand. De app vergelijkt zijn nummer met dat van
//! de nieuwste release, downloadt het bestand voor dit platform, controleert
//! de handtekening met de ingebouwde publieke sleutel en vervangt zichzelf.
//! Een bestand zonder geldige handtekening wordt nooit geïnstalleerd.

use minisign_verify::{PublicKey, Signature};
use perkament::i18n::{t, tf};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;

const RELEASES: &str = "https://api.github.com/repos/PatrickUdo/perkament/releases/latest";
/// Alleen downloads van deze adressen worden geaccepteerd.
const DOWNLOAD_PREFIX: &str = "https://github.com/PatrickUdo/perkament/releases/download/";
/// Publieke sleutel waarmee releases zijn ondertekend (de geheime sleutel staat alleen in de workflow).
const PUBLIC_KEY: &str = "RWR7s/OSI0i7+yEDK54DPU4NLXF9PkX7AIOvE53+V2UhSKGw6iFBfxr7";
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;

const ASSET: &str = if cfg!(target_os = "macos") {
    "Perkament-macOS.zip"
} else if cfg!(windows) {
    "Perkament-Windows.zip"
} else {
    "Perkament-Linux.tar.gz"
};

#[derive(Clone, Debug)]
pub struct Available {
    pub build: u64,
    pub name: String,
    asset_url: String,
    signature_url: Option<String>,
}

pub enum Event {
    Available(Available),
    UpToDate,
    /// De update staat klaar; na opnieuw starten draait de nieuwe versie.
    Installed,
    Failed(String),
}

/// Het buildnummer van deze app, of `None` voor een lokale build (dan zijn er geen updates).
pub fn own_build() -> Option<u64> {
    option_env!("PERKAMENT_BUILD").and_then(|b| b.parse().ok())
}

/// Buildnummer uit een tag als `v0.1.0-build.42`.
fn parse_build(tag: &str) -> Option<u64> {
    tag.rsplit_once("-build.")?.1.parse().ok()
}

fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", "perkament-updater")
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    response.into_reader().take(limit + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() as u64 > limit {
        return Err(t("upd.too_large").to_string());
    }
    Ok(data)
}

/// Leest uit de beschrijving van een release of die nieuwer is dan `own`.
fn newer_release(json: &str, own: u64) -> Result<Option<Available>, String> {
    let release: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let Some(build) = parse_build(tag).filter(|b| *b > own) else { return Ok(None) };
    let asset = |name: &str| {
        release["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(name)).and_then(|a| {
            a["browser_download_url"].as_str().filter(|u| u.starts_with(DOWNLOAD_PREFIX)).map(str::to_string)
        })
    };
    let Some(asset_url) = asset(ASSET) else { return Ok(None) };
    Ok(Some(Available {
        build,
        name: release["name"].as_str().unwrap_or(tag).to_string(),
        asset_url,
        signature_url: asset(&format!("{ASSET}.minisig")),
    }))
}

impl Available {
    /// Is deze release ondertekend, en dus te installeren?
    pub fn signed(&self) -> bool {
        self.signature_url.is_some()
    }
}

pub fn check() -> Result<Option<Available>, String> {
    let own = own_build().ok_or_else(|| t("upd.dev_build").to_string())?;
    let json = fetch(RELEASES, 4 * 1024 * 1024)?;
    newer_release(&String::from_utf8_lossy(&json), own)
}

fn verify(data: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let key = PublicKey::from_base64(public_key).map_err(|e| e.to_string())?;
    let signature = Signature::decode(signature).map_err(|e| e.to_string())?;
    key.verify(data, &signature, false).map_err(|_| t("upd.bad_signature").to_string())
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Kan deze installatie zichzelf vervangen? Niet bij een lokale build, en niet als
/// de app door een beheerder is geïnstalleerd in een map waar de gebruiker niet mag
/// schrijven: dan lopen updates via de beheerder en vragen we er niet om.
pub fn can_install() -> bool {
    if own_build().is_none() {
        return false;
    }
    let Some(parent) = install_target().ok().and_then(|t| t.parent().map(Path::to_path_buf)) else { return false };
    let probe = parent.join(format!(".perkament-probe-{}", std::process::id()));
    let writable = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    writable
}

/// Wat er op schijf vervangen wordt: de appbundel op macOS, anders het programma zelf.
fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    if cfg!(target_os = "macos") {
        // .../Perkament.app/Contents/MacOS/perkament
        let app = exe.ancestors().nth(3).filter(|p| p.extension().is_some_and(|e| e == "app"));
        app.map(Path::to_path_buf).ok_or_else(|| t("upd.not_bundle").to_string())
    } else {
        Ok(exe)
    }
}

/// Pakt het releasebestand uit en zet de nieuwe versie op de plaats van de oude.
fn replace(archive: &[u8]) -> Result<(), String> {
    let target = install_target()?;
    let parent = target.parent().ok_or("no parent directory")?;
    // Uitpakken naast het doel, zodat verplaatsen binnen hetzelfde volume blijft.
    let work = parent.join(format!(".perkament-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let packed = work.join(ASSET);
        std::fs::write(&packed, archive).map_err(|e| e.to_string())?;
        let out = work.join("out");
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let fresh = if cfg!(target_os = "macos") {
            run(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&packed).arg(&out))?;
            out.join("Perkament.app")
        } else if cfg!(windows) {
            run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command"]).arg(format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                packed.display(),
                out.display()
            )))?;
            out.join("perkament.exe")
        } else {
            run(Command::new("tar").arg("-xzf").arg(&packed).arg("-C").arg(&out))?;
            out.join("perkament")
        };
        if !fresh.exists() {
            return Err(t("upd.bad_archive").to_string());
        }
        // De oude versie opzij zetten (ook een draaiend programma mag hernoemd worden),
        // de nieuwe op zijn plaats, en bij een fout de oude terugzetten.
        let old = parent.join(format!(".perkament-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&target, &old).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&fresh, &target) {
            let _ = std::fs::rename(&old, &target);
            return Err(e.to_string());
        }
        // Op Windows is het draaiende programma niet te verwijderen; dat ruimt de volgende start op.
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_file(&old);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn install(update: &Available) -> Result<(), String> {
    let signature_url = update.signature_url.as_deref().ok_or_else(|| t("upd.unsigned").to_string())?;
    let signature = fetch(signature_url, 64 * 1024)?;
    let archive = fetch(&update.asset_url, MAX_DOWNLOAD)?;
    verify(&archive, &String::from_utf8_lossy(&signature), PUBLIC_KEY)?;
    replace(&archive)
}

/// Controleert op de achtergrond. Bij `quiet` wordt alleen een beschikbare update gemeld.
pub fn spawn_check(tx: Sender<Event>, notify: impl Fn() + Send + 'static, quiet: bool) {
    std::thread::spawn(move || {
        let event = match check() {
            Ok(Some(update)) => Event::Available(update),
            Ok(None) if quiet => return,
            Ok(None) => Event::UpToDate,
            Err(_) if quiet => return,
            Err(e) => Event::Failed(e),
        };
        let _ = tx.send(event);
        notify();
    });
}

pub fn spawn_install(update: Available, tx: Sender<Event>, notify: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let event = match install(&update) {
            Ok(()) => Event::Installed,
            Err(e) => Event::Failed(tf("upd.failed", &[("e", &e)])),
        };
        let _ = tx.send(event);
        notify();
    });
}

/// Start de (zojuist bijgewerkte) app opnieuw; de aanroeper sluit daarna af.
pub fn relaunch() {
    let Ok(target) = install_target() else { return };
    let _ = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open").arg("-n").arg(&target).spawn()
    } else {
        Command::new(&target).spawn()
    };
}

/// Ruimt resten van een eerdere update op (op Windows blijft het oude programma tot de volgende start staan).
pub fn clean_up() {
    let Some(parent) = install_target().ok().and_then(|t| t.parent().map(Path::to_path_buf)) else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".perkament-old-") || name.starts_with(".perkament-update-") {
            let _ = std::fs::remove_dir_all(entry.path());
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wegwerpsleutel, alleen voor deze test, met een handtekening over b"test".
    const TEST_KEY: &str = "RWSOMHFVI1sG+E/scHyAwrS5RpUZgAQOl64OpXlX4Vco70OQvqsSyUf4";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key\nRUSOMHFVI1sG+NYHvn8SMqFcbjQYoiCfpH+Guccm5RWZqCCxOdwBPYN3gJHot5TlS5qBceBUvETmjRMgLhpo1Q9ik8+MpGmaCw4=\ntrusted comment: timestamp:1794312222\tfile:msg\thashed\naIXxhoRgcRFOnq3eZ9WF+VMQPUz9FyzMLNzgXcEoL6Nx71M39WGY53yEEy5ahn3z2RKmmZsKsVaT7ypip93fDw==\n";

    #[test]
    fn build_numbers() {
        assert_eq!(parse_build("v0.1.0-build.42"), Some(42));
        assert_eq!(parse_build("v0.1.0"), None);
        assert_eq!(parse_build("v0.1.0-build.x"), None);
    }

    #[test]
    fn release_selection() {
        let json = |tag: &str, host: &str, signed: bool| {
            let sig = if signed {
                format!(r#",{{"name":"{ASSET}.minisig","browser_download_url":"{host}{tag}/{ASSET}.minisig"}}"#)
            } else {
                String::new()
            };
            format!(
                r#"{{"tag_name":"{tag}","name":"Perkament","assets":[{{"name":"{ASSET}","browser_download_url":"{host}{tag}/{ASSET}"}}{sig}]}}"#
            )
        };
        let newer = newer_release(&json("v0.1.0-build.7", DOWNLOAD_PREFIX, true), 3).unwrap().unwrap();
        assert_eq!(newer.build, 7);
        assert!(newer.signature_url.is_some());
        // Zelfde of oudere build: niets te doen.
        assert!(newer_release(&json("v0.1.0-build.3", DOWNLOAD_PREFIX, true), 3).unwrap().is_none());
        // Een download van een ander adres wordt genegeerd.
        assert!(newer_release(&json("v0.1.0-build.9", "https://evil.example/", true), 3).unwrap().is_none());
        // Zonder handtekening is er wel een update, maar die is niet te installeren.
        let unsigned = newer_release(&json("v0.1.0-build.9", DOWNLOAD_PREFIX, false), 3).unwrap().unwrap();
        assert!(unsigned.signature_url.is_none());
        assert!(install(&unsigned).is_err());
        assert!(newer_release("geen json", 3).is_err());
    }

    #[test]
    fn signatures() {
        assert!(PublicKey::from_base64(PUBLIC_KEY).is_ok(), "de ingebouwde sleutel is ongeldig");
        assert!(verify(b"test", TEST_SIG, TEST_KEY).is_ok());
        assert!(verify(b"gewijzigd", TEST_SIG, TEST_KEY).is_err());
        assert!(verify(b"test", TEST_SIG, PUBLIC_KEY).is_err(), "handtekening van een andere sleutel geaccepteerd");
    }
}
