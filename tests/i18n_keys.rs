//! Controleert dat elke vertaalsleutel die in de broncode wordt gebruikt ook bestaat.
use std::collections::HashSet;

#[test]
fn every_used_key_exists() {
    let known: HashSet<&str> = perkament::i18n::keys().collect();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(src).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == "i18n.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for call in ["t(\"", "tf(\""] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(call) {
                let before = rest[..at].chars().next_back();
                rest = &rest[at + call.len()..];
                // Alleen losse aanroepen van `t`/`tf`, niet bv. `format(`.
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let key = &rest[..rest.find('"').unwrap()];
                assert!(known.contains(key), "{}: onbekende vertaalsleutel {key:?}", path.display());
                checked += 1;
            }
        }
    }
    assert!(checked > 20, "te weinig sleutels gevonden ({checked}); klopt de scan nog?");
}
