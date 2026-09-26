//! stack/26 item 3: Homebrew (a system-wide install tree -- shared by
//! every account on the machine, not a per-user `$HOME` path) is off by
//! default under ordinary `defaults = true` scope, distinctly from a
//! detector the user disabled themselves, and `[scan] enabled_detectors
//! = ["homebrew"]` turns it back on. Both directions, in both the text
//! and `--json` renderers of `swamp scope`.
//!
//! Every other detector is disabled here so this fixture's `swamp
//! scope` run only ever stats two things: the fixture `$HOME`'s own
//! tree (via the builtin-defaults detector) and Homebrew's real,
//! machine-wide, absolute prefixes -- read-only presence/permission
//! stats, never a walk of their contents (`swamp scope` never measures
//! bytes). This is the same discipline
//! `external_units.rs::a_detector_that_escapes_the_fixture_home_is_named_here_not_discovered_by_a_byte_total`
//! already relies on for the identical reason.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn run(store: &std::path::Path, home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .output()
        .expect("run swamp")
}

/// `defaults = true`, every detector but Homebrew disabled -- isolates
/// Homebrew's own default-enablement without any other detector's real,
/// machine-wide paths in the mix.
fn config_with_homebrew(enable_homebrew: bool) -> String {
    let disabled: Vec<String> = swamp_core::locations::Registry::with_builtins()
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| {
            id != swamp_core::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID
                && id != swamp_core::locations::homebrew::HOMEBREW_DETECTOR_ID
        })
        .collect();
    let disabled_toml = disabled
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let enabled_toml = if enable_homebrew { "\"homebrew\"" } else { "" };
    format!(
        "[scan]\ndefaults = true\ndisabled_detectors = [{disabled_toml}]\n\
         enabled_detectors = [{enabled_toml}]\n"
    )
}

#[test]
fn homebrew_is_off_by_default_in_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        store.path().join("config.toml"),
        config_with_homebrew(false),
    )
    .unwrap();

    let text_out = run(store.path(), home.path(), &["scope"]);
    assert!(
        text_out.status.success(),
        "{}",
        String::from_utf8_lossy(&text_out.stderr)
    );
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        text.contains("homebrew") && text.contains("disabled (default off)"),
        "text scope output must name Homebrew as disabled specifically because it defaults off, \
         not merely `disabled`: {text}"
    );

    let json_out = run(store.path(), home.path(), &["scope", "--json"]);
    assert!(json_out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    let default_off = json["default_off_detectors"]
        .as_array()
        .expect("default_off_detectors array");
    assert!(
        default_off.iter().any(|v| v.as_str() == Some("homebrew")),
        "JSON must carry the same default-off fact: {json}"
    );
    let roots = json["roots"].as_array().expect("roots array");
    assert!(
        !roots.iter().any(|r| r["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["detector_id"].as_str() == Some("homebrew"))),
        "a default-off detector must contribute no root: {json}"
    );
}

#[test]
fn enabled_detectors_turns_homebrew_on_in_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("config.toml"), config_with_homebrew(true)).unwrap();

    let text_out = run(store.path(), home.path(), &["scope"]);
    assert!(
        text_out.status.success(),
        "{}",
        String::from_utf8_lossy(&text_out.stderr)
    );
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        text.contains("homebrew") && text.contains("resolved"),
        "text scope output must show Homebrew resolved once explicitly enabled: {text}"
    );
    assert!(
        !text.contains("disabled (default off)"),
        "an explicitly enabled detector must not still read as default-off: {text}"
    );

    let json_out = run(store.path(), home.path(), &["scope", "--json"]);
    assert!(json_out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    let default_off = json["default_off_detectors"]
        .as_array()
        .expect("default_off_detectors array");
    assert!(
        !default_off.iter().any(|v| v.as_str() == Some("homebrew")),
        "an explicitly enabled detector is not default-off: {json}"
    );
    let roots = json["roots"].as_array().expect("roots array");
    assert!(
        roots.iter().any(|r| r["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["detector_id"].as_str() == Some("homebrew"))),
        "an explicitly enabled Homebrew must contribute its resolved prefix as a root: {json}"
    );
}
