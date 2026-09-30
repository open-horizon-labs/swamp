//! #174: Homebrew is measured in two honest parts by default -- the
//! unambiguous developer tooling as units of their own, and everything
//! else under the prefix as one `Homebrew (other)` remainder -- and the
//! two add up to the prefix. Fixture prefix under a temp dir, never the
//! machine's `/opt/homebrew`.
//!
//! Each test names the tempting wrong implementation it fails.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::external::ExternalUnit;
use swamp_core::locations::{Detector, Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};
use swamp_core::walk::resize_artifact;

fn put(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, swamp_core::fs_gate::settle::noise(bytes)).unwrap();
}

struct Fixture {
    _home: tempfile::TempDir,
    home: PathBuf,
    prefix: PathBuf,
}

/// A prefix with dev formulae, look-alike names, ambiguous tools, casks
/// and the Android command-line tools.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let prefix = home.join("brew");
    for (formula, n) in [
        ("llvm@20", 30_000),
        ("llvm@21", 31_000),
        ("go", 20_000),
        ("python@3.12", 12_000),
        // look-alikes that merely contain an allowlisted name
        ("gopls", 9_000),
        ("zigbee2mqtt", 8_000),
        ("nodejs-foo", 7_000),
        // ambiguous tools the maintainer kept out
        ("qemu", 40_000),
        ("ansible", 6_000),
    ] {
        put(&prefix.join("Cellar").join(formula).join("1.0/bin/tool"), n);
    }
    put(&prefix.join("Caskroom/android-platform-tools/1/adb"), 4_000);
    put(
        &prefix.join("Caskroom/microsoft-outlook/1/Outlook.app/blob"),
        50_000,
    );
    put(&prefix.join("lib/libjunk.dylib"), 5_000);
    let sdk = prefix.join("share/android-commandlinetools");
    put(&sdk.join("system-images/android-35/system.img"), 60_000);
    put(&sdk.join("cmdline-tools/latest/bin/sdkmanager"), 3_000);
    Fixture {
        _home: tmp,
        home,
        prefix,
    }
}

fn env(f: &Fixture, android_home: bool) -> Environment {
    let mut vars = HashMap::new();
    vars.insert(
        "HOMEBREW_PREFIX".to_string(),
        f.prefix.display().to_string(),
    );
    if android_home {
        vars.insert(
            "ANDROID_HOME".to_string(),
            f.prefix
                .join("share/android-commandlinetools")
                .display()
                .to_string(),
        );
    }
    Environment::fixture(f.home.clone(), vars, Platform::MacOS)
}

/// Only the named detectors (plus builtin-defaults) run.
fn config_running(keep: &[&str], registry: &Registry) -> ScanConfig {
    ScanConfig {
        defaults: true,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != "builtin-defaults" && !keep.contains(&id.as_str()))
            .collect(),
        enabled_detectors: keep.iter().map(|s| s.to_string()).collect(),
        ..ScanConfig::default()
    }
}

fn measure(env: &Environment, cfg: &ScanConfig) -> Vec<ExternalUnit> {
    swamp_core::fs_gate::settle::settle();
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(env, cfg, &[], &registry, 1_000);
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

fn defaults_only(env: &Environment) -> Vec<ExternalUnit> {
    let registry = Registry::with_builtins();
    let cfg = config_running(
        &["homebrew-devtools", "homebrew-other", "android"],
        &registry,
    );
    measure(env, &cfg)
}

fn total(prefix: &Path) -> u64 {
    swamp_core::fs_gate::settle::settle();
    resize_artifact(prefix, swamp_core::report::ArtifactKind::Unknown, 1_000).bytes
}

fn dev_names(units: &[ExternalUnit], prefix: &Path) -> Vec<String> {
    units
        .iter()
        .filter(|u| u.detector_id == "homebrew-devtools")
        .map(|u| u.path.strip_prefix(prefix).unwrap().display().to_string())
        .collect()
}

/// Tempting wrong patch: match allowlist names by substring or prefix
/// (`name.contains("go")`), or treat every Cellar entry as dev tooling.
#[test]
fn only_allowlisted_names_are_dev_and_lookalikes_and_ambiguous_tools_are_not() {
    let f = fixture();
    let units = defaults_only(&env(&f, false));
    let mut dev = dev_names(&units, &f.prefix);
    dev.sort();
    assert_eq!(
        dev,
        [
            "Caskroom/android-platform-tools",
            "Cellar/go",
            "Cellar/llvm@20",
            "Cellar/llvm@21",
            "Cellar/python@3.12",
            "share/android-commandlinetools",
        ],
        "{units:#?}"
    );
    for not_dev in [
        "gopls",
        "zigbee2mqtt",
        "nodejs-foo",
        "qemu",
        "ansible",
        "microsoft-outlook",
    ] {
        assert!(
            units
                .iter()
                .all(|u| !u.path.ends_with(not_dev) || u.detector_id != "homebrew-devtools"),
            "{not_dev} must not be a dev unit"
        );
    }
}

/// Tempting wrong patch: measure the prefix whole as "other" without
/// subtracting the dev units (double count), or drop the remainder
/// (bytes disappear). Dev plus other must equal the prefix.
#[test]
fn dev_plus_other_equals_the_prefix_total_and_other_holds_the_ambiguous_bytes() {
    let f = fixture();
    let units = defaults_only(&env(&f, false));
    let other = units
        .iter()
        .find(|u| u.detector_id == "homebrew-other")
        .expect("the remainder is reported as one unit");
    assert_eq!(other.detector_name, "Homebrew (other)");
    assert_eq!(other.path, f.prefix);
    let brew_sum: u64 = units
        .iter()
        .filter(|u| u.detector_id.starts_with("homebrew"))
        .map(|u| u.bytes)
        .sum();
    assert_eq!(brew_sum, total(&f.prefix), "{units:#?}");
    // qemu (40k) and the outlook cask (50k) live in the remainder.
    assert!(other.bytes >= 90_000, "{}", other.bytes);
    let dev_sum: u64 = units
        .iter()
        .filter(|u| u.detector_id == "homebrew-devtools")
        .map(|u| u.bytes)
        .sum();
    assert!(dev_sum > 0 && dev_sum + other.bytes == brew_sum);
}

/// Tempting wrong patch: propose `share/android-commandlinetools` from
/// Homebrew without the nested subtraction, so the android detector's
/// system-images are counted in both. Also covers `ANDROID_HOME` pointing
/// into the Homebrew prefix, the reporter's real layout.
#[test]
fn android_commandline_tools_bytes_are_counted_once_across_both_detectors() {
    let f = fixture();
    let units = defaults_only(&env(&f, true));
    let android_bytes: u64 = units
        .iter()
        .filter(|u| u.detector_id == "android")
        .map(|u| u.bytes)
        .sum();
    assert!(
        android_bytes >= 60_000,
        "android detector sees system-images"
    );
    let all: u64 = units
        .iter()
        .filter(|u| u.detector_id.starts_with("homebrew") || u.detector_id == "android")
        .map(|u| u.bytes)
        .sum();
    assert_eq!(all, total(&f.prefix), "{units:#?}");
    let cmdline = units
        .iter()
        .find(|u| u.path == f.prefix.join("share/android-commandlinetools"))
        .expect("the Homebrew-side unit exists");
    assert!(
        cmdline.bytes < 60_000,
        "system-images must not be inside the Homebrew unit: {}",
        cmdline.bytes
    );
}

/// Tempting wrong patch: an absent prefix is an error, or a zero-byte
/// unit. It is a coverage fact: the scope says missing, nothing is
/// measured, nothing panics.
#[test]
fn a_missing_prefix_is_a_coverage_fact_not_an_error_or_a_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert(
        "HOMEBREW_PREFIX".to_string(),
        home.join("no-such-prefix").display().to_string(),
    );
    let env = Environment::fixture(home.clone(), vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = config_running(&["homebrew-devtools", "homebrew-other"], &registry);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);
    let missing = scope
        .roots
        .iter()
        .filter(|r| r.path.starts_with(home.join("no-such-prefix")))
        .collect::<Vec<_>>();
    assert!(!missing.is_empty());
    let prefix_root = scope
        .roots
        .iter()
        .find(|r| r.path == home.join("no-such-prefix"))
        .expect("the prefix is a scope root");
    assert_eq!(prefix_root.status, swamp_core::scope::RootStatus::Missing);
    assert!(
        missing
            .iter()
            .all(|r| r.status != swamp_core::scope::RootStatus::Present)
    );
    assert!(measure(&env, &cfg).is_empty());
}

/// Tempting wrong patch: enabling the full detector on top of the
/// default two adds a third set of units over the same bytes. Turning it
/// on takes the prefix over: still one count, and no `Homebrew (other)`.
#[test]
fn enabling_the_full_detector_still_counts_every_byte_once() {
    let f = fixture();
    let registry = Registry::with_builtins();
    let cfg = config_running(
        &["homebrew", "homebrew-devtools", "homebrew-other"],
        &registry,
    );
    let units = measure(&env(&f, false), &cfg);
    let brew_sum: u64 = units
        .iter()
        .filter(|u| u.detector_id.starts_with("homebrew"))
        .map(|u| u.bytes)
        .sum();
    assert_eq!(brew_sum, total(&f.prefix), "{units:#?}");
    // Cellar is reported whole (the full detector), not split by the
    // allowlist: a `homebrew` unit's path is Cellar itself.
    assert!(
        units
            .iter()
            .any(|u| u.detector_id == "homebrew" && u.path == f.prefix.join("Cellar")),
        "{units:#?}"
    );
    assert!(units.iter().all(|u| {
        u.detector_id != "homebrew-devtools" || u.path.ends_with("share/android-commandlinetools")
    }));
    assert!(units.iter().all(|u| u.detector_id != "homebrew-other"));
}

/// Tempting wrong patch: enable the dev part by default but leave no way
/// to turn a part off. `disabled_detectors` drops the remainder line and
/// the dev units independently.
#[test]
fn the_remainder_and_the_dev_units_can_each_be_disabled() {
    let f = fixture();
    let registry = Registry::with_builtins();
    let only_dev = measure(
        &env(&f, false),
        &config_running(&["homebrew-devtools"], &registry),
    );
    assert!(only_dev.iter().all(|u| u.detector_id != "homebrew-other"));
    assert!(!only_dev.is_empty());
    let only_other = measure(
        &env(&f, false),
        &config_running(&["homebrew-other"], &registry),
    );
    assert!(
        only_other
            .iter()
            .all(|u| u.detector_id != "homebrew-devtools")
    );
    assert_eq!(only_other.len(), 1);
}

/// Tempting wrong patch: an `exclude` of a dev formula is ignored by the
/// expansion, so it is still measured, or is subtracted from nothing and
/// is counted in the remainder. Chosen semantics: an excluded formula is
/// out of scope entirely, so its bytes are in NO unit, and the parent
/// subtracts it (an exclusion is never read as the parent shrinking or
/// growing). The exclusion itself is the named residual: `swamp scope`
/// lists it as excluded by the pattern that names it.
#[test]
fn an_excluded_dev_formula_is_in_no_unit_and_is_subtracted_from_the_remainder() {
    let f = fixture();
    let registry = Registry::with_builtins();
    let mut cfg = config_running(&["homebrew-devtools", "homebrew-other"], &registry);
    cfg.exclude = vec![f.prefix.join("Cellar/llvm@21").display().to_string()];
    let units = measure(&env(&f, false), &cfg);
    assert!(units.iter().all(|u| !u.path.ends_with("Cellar/llvm@21")));
    let sum: u64 = units.iter().map(|u| u.bytes).sum();
    assert_eq!(
        sum + total(&f.prefix.join("Cellar/llvm@21")),
        total(&f.prefix),
        "{units:#?}"
    );
}

/// The remainder has no recovery hint (it is Homebrew itself, bin, lib and
/// GUI casks: no command re-obtains that as a unit); the dev units do.
#[test]
fn the_remainder_has_no_recovery_hint_and_is_marked_by_capability() {
    let registry = Registry::with_builtins();
    let by_id = |id: &str| registry.detectors().iter().find(|d| d.id() == id).unwrap();
    assert!(by_id("homebrew-other").recovery_hint().is_none());
    assert!(by_id("homebrew-devtools").recovery_hint().is_some());
    let r = swamp_core::locations::remainder_of("homebrew-other").expect("a remainder");
    assert!(r.include_all.contains("enabled_detectors"));
    assert!(swamp_core::locations::remainder_of("homebrew-devtools").is_none());
}

/// Tempting wrong patch: measure `/usr/local` whole as the Intel prefix, so
/// every other installer's files there are reported as Homebrew's.
#[test]
fn on_the_shared_usr_local_prefix_only_homebrews_own_directories_are_proposed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars = HashMap::new();
    vars.insert("HOMEBREW_PREFIX".to_string(), "/usr/local".to_string());
    let env = Environment::fixture(tmp.path().to_path_buf(), vars, Platform::MacOS);
    let other = swamp_core::locations::homebrew::HomebrewOtherDetector.detect(&env);
    let paths: Vec<_> = other.iter().filter_map(|l| l.path.clone()).collect();
    assert!(!paths.contains(&PathBuf::from("/usr/local")), "{paths:?}");
    assert!(paths.contains(&PathBuf::from("/usr/local/Homebrew")));
}

/// Tempting wrong patch: a config that disabled Homebrew before the split
/// keeps its meaning: the family id turns off every member, and a member
/// id turns off just that member.
#[test]
fn the_family_id_disables_every_member_and_a_member_id_only_itself() {
    let f = fixture();
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != "builtin-defaults" && !id.starts_with("homebrew"))
            .collect(),
        ..ScanConfig::default()
    };
    let mut family = cfg.clone();
    family.disabled_detectors.push("homebrew".into());
    assert!(measure(&env(&f, false), &family).is_empty());
    let mut one = cfg;
    one.disabled_detectors.push("homebrew-other".into());
    let units = measure(&env(&f, false), &one);
    assert!(units.iter().all(|u| u.detector_id == "homebrew-devtools"));
    assert!(!units.is_empty());
}

/// Tempting wrong patch: print only the detector id for the remainder and
/// leave the setting that includes it in the scope note, where nobody
/// reading the external view finds it.
#[test]
fn the_external_view_names_the_remainder_and_carries_the_setting_on_its_row() {
    let f = fixture();
    let units = defaults_only(&env(&f, false));
    let text = swamp_core::render::render_view_external(&units);
    assert!(text.contains("Homebrew (other) (homebrew-other)"), "{text}");
    assert!(
        text.contains("[scan] enabled_detectors = [\"homebrew\"]"),
        "{text}"
    );
    assert!(
        text.contains("Homebrew (dev tooling) (homebrew-devtools)"),
        "{text}"
    );
    assert_eq!(text.matches("to report it whole").count(), 1, "{text}");
}

/// Tempting wrong patch: a Cellar this user cannot list yields no dev units
/// and no word about it, which reads as "nothing installed". It is a
/// coverage note: named, not measured, never zero.
#[test]
fn an_unlistable_cellar_is_a_coverage_note_not_silence() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let cellar = f.prefix.join("Cellar");
    fs::set_permissions(&cellar, fs::Permissions::from_mode(0o000)).unwrap();
    let registry = Registry::with_builtins();
    let cfg = config_running(&["homebrew-devtools"], &registry);
    let scope = resolve_effective_scope(&env(&f, false), &cfg, &[], &registry, 1_000);
    let got = swamp_core::external::observe_external(
        &swamp_core::report::DiscoveryPass::for_tests(),
        &scope,
        &[],
        None,
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    );
    let bypassed = fs::read_dir(&cellar).is_ok();
    fs::set_permissions(&cellar, fs::Permissions::from_mode(0o755)).unwrap();
    let got = got.unwrap();
    if !bypassed {
        assert!(
            got.notes
                .iter()
                .any(|n| n.contains("Cellar") && n.contains("not measured")),
            "{:?}",
            got.notes
        );
    }
}
