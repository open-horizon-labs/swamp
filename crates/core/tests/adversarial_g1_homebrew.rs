//! Adversarial tests for #174 (Homebrew policy), written by the v0.8.0 G1
//! audit. Each test names the wrong-but-plausible implementation it is
//! aimed at. A test that fails on the PR head is a finding. Fixture prefix
//! under a temp dir, never the machine's `/opt/homebrew`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::external::ExternalUnit;
use swamp_core::locations::homebrew::HomebrewDevToolsDetector;
use swamp_core::locations::{Detector, Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};
use swamp_core::walk::resize_artifact;

fn put(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, swamp_core::fs_gate::settle::noise(bytes)).unwrap();
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    prefix: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let prefix = home.join("brew");
    put(&prefix.join("Cellar/go/1.0/bin/go"), 20_000);
    put(&prefix.join("Cellar/qemu/1.0/bin/qemu"), 40_000);
    put(&prefix.join("lib/libjunk.dylib"), 5_000);
    Fx {
        _tmp: tmp,
        home,
        prefix,
    }
}

fn env_with_prefix(home: &Path, prefix: &Path) -> Environment {
    let mut vars = HashMap::new();
    vars.insert("HOMEBREW_PREFIX".to_string(), prefix.display().to_string());
    Environment::fixture(home.to_path_buf(), vars, Platform::MacOS)
}

/// Every detector off except `keep`, without naming any in
/// `enabled_detectors` (a v0.7.x style deny-list config).
fn deny_all_but(keep: &[&str], extra_disabled: &[&str]) -> ScanConfig {
    let registry = Registry::with_builtins();
    let mut disabled: Vec<String> = registry
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != "builtin-defaults" && !keep.contains(&id.as_str()))
        // `homebrew` is the family id: naming it in a deny-list turns off
        // every Homebrew detector (a_config_that_disabled_homebrew...), so
        // a fixture that keeps a member must not name it. The full
        // detector is off by default anyway. (Fixture change by the G1
        // builder; the assertions are untouched.)
        .filter(|id| !(id == "homebrew" && keep.iter().any(|k| k.starts_with("homebrew-"))))
        .collect();
    for e in extra_disabled {
        if !disabled.iter().any(|d| d == e) {
            disabled.push(e.to_string());
        }
    }
    ScanConfig {
        defaults: true,
        disabled_detectors: disabled,
        ..ScanConfig::default()
    }
}

fn measure(env: &Environment, cfg: &ScanConfig) -> Vec<ExternalUnit> {
    swamp_core::fs_gate::settle::settle();
    let scope = resolve_effective_scope(env, cfg, &[], &Registry::with_builtins(), 1_000);
    swamp_core::external::discover_and_measure(
        &scope,
        None,
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .expect("discover_and_measure")
}

fn brew(units: &[ExternalUnit]) -> Vec<&ExternalUnit> {
    units
        .iter()
        .filter(|u| u.detector_id.starts_with("homebrew"))
        .collect()
}

fn sum(units: &[ExternalUnit]) -> u64 {
    brew(units).iter().map(|u| u.bytes).sum()
}

fn total(p: &Path) -> u64 {
    swamp_core::fs_gate::settle::settle();
    resize_artifact(p, swamp_core::report::ArtifactKind::Unknown, 1_000).bytes
}

fn default_two(f: &Fx) -> Vec<ExternalUnit> {
    measure(
        &env_with_prefix(&f.home, &f.prefix),
        &deny_all_but(&["homebrew-devtools", "homebrew-other"], &[]),
    )
}

/// Tempting wrong patch: substring/prefix match, case folding, or a base
/// computed by splitting on `-`.
#[test]
fn allowlist_boundaries() {
    let names: Vec<String> = [
        "python@3.14",
        "python-tk@3.14",
        "openjdk",
        "openjdk@17",
        "llvm",
        "llvm@20",
        "llvm-tools",
        "go-task",
        "node-red",
        "nodejs-foo",
        "gopls",
        "zigbee2mqtt",
        "LLVM",
        "Go",
        "homebrew/core/llvm",
        "@llvm",
        ".hidden",
        "rust@",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let picked = HomebrewDevToolsDetector
        .select_children(Path::new("/p/Cellar"), &names)
        .unwrap();
    assert_eq!(
        picked,
        [
            "python@3.14",
            "openjdk",
            "openjdk@17",
            "llvm",
            "llvm@20",
            "rust@"
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
    );
    let casks: Vec<String> = [".metadata", "android-studio", "Android-Studio"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        HomebrewDevToolsDetector
            .select_children(Path::new("/p/Caskroom"), &casks)
            .unwrap(),
        vec!["android-studio".to_string()]
    );
}

/// Tempting wrong patch: `homebrew-devtools`/`homebrew-other` are new ids,
/// so a v0.7.x config that turned Homebrew off with
/// `disabled_detectors = ["homebrew"]` silently starts measuring the whole
/// prefix again after the upgrade.
#[test]
fn a_config_that_disabled_homebrew_does_not_start_measuring_the_prefix() {
    let f = fx();
    let cfg = deny_all_but(&["homebrew-devtools", "homebrew-other"], &["homebrew"]);
    let units = measure(&env_with_prefix(&f.home, &f.prefix), &cfg);
    assert!(
        brew(&units).is_empty(),
        "disabled_detectors = [\"homebrew\"] still measures: {:#?}",
        brew(&units)
            .iter()
            .map(|u| (&u.detector_id, &u.path, u.bytes))
            .collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: key the stored snapshot by the scope's root list,
/// so adding two detectors on upgrade orphans the stored observation and
/// `report` says "no observation yet" until the next `observe`.
#[test]
fn upgrade_keeps_the_snapshot_key_of_an_existing_observation() {
    let f = fx();
    let env = env_with_prefix(&f.home, &f.prefix);
    let registry = Registry::with_builtins();
    let v075 = ScanConfig {
        disabled_detectors: vec!["homebrew-devtools".into(), "homebrew-other".into()],
        ..ScanConfig::default()
    };
    let old = swamp_core::report::scope_snapshot_key(&resolve_effective_scope(
        &env,
        &v075,
        &[],
        &registry,
        1,
    ));
    let new = swamp_core::report::scope_snapshot_key(&resolve_effective_scope(
        &env,
        &ScanConfig::default(),
        &[],
        &registry,
        1,
    ));
    assert_eq!(
        old, new,
        "the v0.7.5 observation is not found under the v0.8.0 key"
    );
}

/// Tempting wrong patch: size each unit with its own inode set, so a file
/// hardlinked between a Cellar keg and elsewhere in the prefix is counted
/// in both the dev unit and the remainder.
#[test]
fn a_hardlink_between_a_dev_keg_and_the_remainder_is_counted_once() {
    let f = fx();
    put(&f.prefix.join("Cellar/go/1.0/lib/big"), 200_000);
    fs::hard_link(
        f.prefix.join("Cellar/go/1.0/lib/big"),
        f.prefix.join("lib/big"),
    )
    .unwrap();
    let units = default_two(&f);
    assert_eq!(sum(&units), total(&f.prefix), "{units:#?}");
}

/// Tempting wrong patch: apparent size (`len`) instead of allocated
/// blocks, so a sparse file inflates the dev unit.
#[test]
fn a_sparse_file_counts_its_allocated_bytes_and_parts_still_sum() {
    let f = fx();
    let p = f.prefix.join("Cellar/go/1.0/sparse.img");
    let file = fs::File::create(&p).unwrap();
    file.set_len(1 << 30).unwrap();
    let units = default_two(&f);
    let go = units
        .iter()
        .find(|u| u.path.ends_with("Cellar/go"))
        .expect("go unit");
    assert!(go.bytes < 10 << 20, "sparse counted as {}", go.bytes);
    assert_eq!(sum(&units), total(&f.prefix));
}

/// Tempting wrong patch: `is_dir()` following symlinks, so a Cellar entry
/// that links outside the prefix is measured as a dev unit with bytes that
/// are not in the prefix at all.
#[test]
fn a_symlinked_or_file_cellar_entry_is_not_a_dev_unit() {
    let f = fx();
    put(&f.home.join("outside/node/1/bin/node"), 70_000);
    std::os::unix::fs::symlink(f.home.join("outside/node"), f.prefix.join("Cellar/node")).unwrap();
    put(&f.prefix.join("Cellar/rust"), 3_000);
    let units = default_two(&f);
    assert!(
        units
            .iter()
            .all(|u| !u.path.ends_with("Cellar/node") && !u.path.ends_with("Cellar/rust")),
        "{units:#?}"
    );
    assert_eq!(sum(&units), total(&f.prefix), "{units:#?}");
}

/// Tempting wrong patch: compare the listed Cellar path with the
/// canonical unit path, so a symlinked prefix (`/opt/homebrew -> ...`)
/// yields no dev units or counts the remainder twice.
#[test]
fn a_symlinked_prefix_still_splits_and_sums() {
    let f = fx();
    let link = f.home.join("brew-link");
    std::os::unix::fs::symlink(&f.prefix, &link).unwrap();
    let units = measure(
        &env_with_prefix(&f.home, &link),
        &deny_all_but(&["homebrew-devtools", "homebrew-other"], &[]),
    );
    assert!(
        units.iter().any(|u| u.detector_id == "homebrew-devtools"),
        "{units:#?}"
    );
    assert_eq!(sum(&units), total(&f.prefix), "{units:#?}");
}

/// Tempting wrong patch: a Caskroom's hidden `.metadata` directory treated
/// as a cask, or dropped from the remainder.
#[test]
fn caskroom_hidden_dirs_stay_in_the_remainder() {
    let f = fx();
    put(&f.prefix.join("Caskroom/.metadata/x"), 9_000);
    put(
        &f.prefix.join("Caskroom/android-studio/.metadata/1/y"),
        4_000,
    );
    put(
        &f.prefix.join("Caskroom/android-studio/2024.1/Studio.app/z"),
        8_000,
    );
    let units = default_two(&f);
    let studio = units
        .iter()
        .find(|u| u.path.ends_with("Caskroom/android-studio"))
        .expect("android-studio unit");
    assert!(studio.bytes >= 12_000, "{}", studio.bytes);
    assert_eq!(sum(&units), total(&f.prefix));
}

// --- strings ------------------------------------------------------------

const VERDICT: &[&str] = &["unused", "obsolete", "stale", "safe", "orphan"];

fn check_text(what: &str, text: &str) {
    let lower = text.to_lowercase();
    for w in VERDICT {
        assert!(!lower.contains(w), "{what}: verdict word {w:?} in {text:?}");
    }
    assert!(!text.contains('\u{2014}'), "{what}: em dash in {text:?}");
}

/// Every new detector name, note and version note, and every roots error
/// message, carries no verdict word and no em dash.
#[test]
fn new_strings_have_no_verdict_words_or_em_dashes() {
    let f = fx();
    let env = env_with_prefix(&f.home, &f.prefix);
    for d in Registry::with_builtins().detectors() {
        if !d.id().starts_with("homebrew") {
            continue;
        }
        check_text(d.id(), d.name());
        check_text(d.id(), d.version_note());
        for l in d.detect(&env) {
            if let Some(n) = &l.note {
                check_text(d.id(), n);
            }
        }
    }
    use swamp_core::roots::RootError::*;
    let p = PathBuf::from("/x");
    for e in [
        Missing(p.clone()),
        NotADirectory(p.clone()),
        Unreadable {
            path: p.clone(),
            reason: "r".into(),
        },
        NestedUnder {
            typed: "a".into(),
            declared: "b".into(),
        },
        TooBroad(p.clone()),
        NotDeclared("a".into()),
        NotUtf8(p.clone()),
    ] {
        check_text("roots error", &e.to_string());
    }
}
