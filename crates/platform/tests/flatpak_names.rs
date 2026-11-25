//! Inside the Flatpak the session bus goes through a filter that lets the
//! app own and call only the names its manifest lists (`finish-args`). A
//! name missing there fails only in the Flatpak: without
//! `--own-name=org.freedesktop.ReserveDevice1.*` the DAC could never be
//! taken from PipeWire ("the sound card is in use by another application"),
//! as the 2.0.0 bundle shipped.

#![cfg(target_os = "linux")]

use std::path::Path;

use sonkore_platform::{mpris, reservation, tray};

/// (own, talk) name patterns of the manifest's finish-args.
fn policy() -> (Vec<String>, Vec<String>) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/flatpak/com.axiokyn.Sonkore.yml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let (mut own, mut talk) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let arg = line.trim().trim_start_matches("- ").trim();
        if let Some(name) = arg.strip_prefix("--own-name=") {
            own.push(name.to_string());
        } else if let Some(name) = arg.strip_prefix("--talk-name=") {
            talk.push(name.to_string());
        }
    }
    (own, talk)
}

/// Flatpak's matching: `a.b.*` is `a.b` and every name under it.
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => name == prefix || name.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')),
        None => name == pattern,
    }
}

#[test]
fn every_name_the_app_owns_is_in_the_manifest() {
    let (own, _) = policy();
    let owned = [
        reservation::service_name(0),
        reservation::service_name(12),
        mpris::SERVICE.to_string(),
        mpris::instance_name(4242),
    ];
    for name in owned {
        assert!(own.iter().any(|p| matches(p, &name)), "{name} is not in the manifest's --own-name: {own:?}");
    }
}

#[test]
fn every_name_the_app_calls_is_in_the_manifest() {
    let (own, talk) = policy();
    // Owning a name also allows talking to it (the reservation asks its
    // current holder, PipeWire, to let the card go).
    for name in [tray::WATCHER.to_string(), reservation::service_name(1)] {
        assert!(
            own.iter().chain(&talk).any(|p| matches(p, &name)),
            "{name} is not in the manifest's --talk-name or --own-name"
        );
    }
}

#[test]
fn no_name_has_two_policies() {
    // One policy per name: a --talk-name for an owned name would replace
    // the own permission, and taking the name would fail again.
    let (own, talk) = policy();
    for t in &talk {
        assert!(!own.contains(t), "{t} is both --own-name and --talk-name");
    }
}

#[test]
fn flatpak_patterns_match_as_flatpak_does() {
    assert!(matches("org.freedesktop.ReserveDevice1.*", "org.freedesktop.ReserveDevice1.Audio3"));
    assert!(!matches("org.freedesktop.ReserveDevice1.*", "org.freedesktop.ReserveDevice10.Audio3"));
    assert!(matches("org.mpris.MediaPlayer2.sonkore", "org.mpris.MediaPlayer2.sonkore"));
    assert!(!matches("org.mpris.MediaPlayer2.sonkore", "org.mpris.MediaPlayer2.sonkore.instance1"));
}
