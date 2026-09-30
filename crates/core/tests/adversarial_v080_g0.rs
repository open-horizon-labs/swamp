//! Adversarial tests for v0.8.0 group G0 (#164 espressif, #165 android
//! SDK folders, #166 /Library/Developer siblings, #172 claude-code
//! scratch). Each test names the tempting wrong implementation it is
//! aimed at.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::external::{ExternalUnit, discover_and_measure};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

const MB: usize = 1 << 20;

fn env_of(home: &Path, extra: &[(&str, String)]) -> Environment {
    let mut env: HashMap<String, String> = HashMap::new();
    for (k, v) in extra {
        env.insert((*k).to_string(), v.clone());
    }
    Environment::fixture(home.to_path_buf(), env, Platform::MacOS)
}

fn only(ids: &[&str]) -> ScanConfig {
    ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: ids.iter().map(|s| s.to_string()).collect(),
    }
}

fn put(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

fn measure(env: &Environment, ids: &[&str]) -> Vec<ExternalUnit> {
    swamp_core::fs_gate::settle::settle();
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(env, &only(ids), &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();
    discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap()
}

fn total(units: &[ExternalUnit]) -> u64 {
    units.iter().map(|u| u.bytes).sum()
}

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap()
}

// ---------------------------------------------------------------- android

/// Tempting wrong patch: `ndk.starts_with(&sdk)` on the raw env string.
/// `<sdk>/../ndk-outside/26` begins with the SDK's components but, once
/// `..` is resolved (scope normalizes lexically), lies OUTSIDE the SDK.
/// The detector drops it as "already inside ndk/", so its bytes are never
/// measured at all.
#[test]
fn an_ndk_env_path_that_dotdots_out_of_the_sdk_is_still_measured() {
    let home = tempfile::tempdir().unwrap();
    let sdk = home.path().join("sdk");
    put(&sdk.join("platforms/android-34/android.jar"), 64 * 1024);
    let outside = home.path().join("ndk-outside/26.1");
    put(&outside.join("toolchains/clang"), 2 * MB);
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", sdk.display().to_string()),
            (
                "ANDROID_NDK_HOME",
                sdk.join("../ndk-outside/26.1").display().to_string(),
            ),
        ],
    );
    let units = measure(&env, &["android"]);
    let want = canon(&outside);
    assert!(
        units.iter().any(|u| u.path == want),
        "the NDK outside the SDK root is not measured; units: {:?}",
        units.iter().map(|u| &u.path).collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: compare the NDK path to the SDK path without
/// resolving symlinks. An `ANDROID_NDK_HOME` that is a symlink to a
/// version directory inside `<sdk>/ndk/` must not add its bytes again.
#[test]
fn an_ndk_env_symlink_into_the_sdk_is_counted_once() {
    let home = tempfile::tempdir().unwrap();
    let sdk = home.path().join("sdk");
    let real = sdk.join("ndk/26.1");
    put(&real.join("toolchains/clang"), 4 * MB);
    let link = home.path().join("ndk-link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", sdk.display().to_string()),
            ("ANDROID_NDK_HOME", link.display().to_string()),
        ],
    );
    let units = measure(&env, &["android"]);
    let t = total(&units);
    assert!(
        t < 6 * MB as u64,
        "4 MB NDK summed to {t} bytes across {:?}",
        units.iter().map(|u| (&u.path, u.bytes)).collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: only guard "NDK inside SDK", never "SDK inside
/// NDK". An `ANDROID_NDK_ROOT` that names an ANCESTOR of the SDK root
/// covers every SDK package; they must not be summed twice.
#[test]
fn an_ndk_env_that_is_an_ancestor_of_the_sdk_does_not_double_count() {
    let home = tempfile::tempdir().unwrap();
    let parent = home.path().join("android");
    let sdk = parent.join("sdk");
    put(&sdk.join("platforms/android-34/android.jar"), 4 * MB);
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", sdk.display().to_string()),
            ("ANDROID_NDK_ROOT", parent.display().to_string()),
        ],
    );
    let units = measure(&env, &["android"]);
    let t = total(&units);
    assert!(
        t < 6 * MB as u64,
        "4 MB summed to {t}: {:?}",
        units.iter().map(|u| (&u.path, u.bytes)).collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: string prefix. `/x/sdk2/ndk` is not inside
/// `/x/sdk`, so it must be proposed (and measured) as its own NDK.
#[test]
fn an_ndk_beside_an_sdk_whose_name_is_a_prefix_is_measured() {
    let home = tempfile::tempdir().unwrap();
    let sdk = home.path().join("sdk");
    put(&sdk.join("platforms/p/a.jar"), 64 * 1024);
    let ndk = home.path().join("sdk2");
    put(&ndk.join("toolchains/clang"), MB);
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", sdk.display().to_string()),
            ("ANDROID_NDK_HOME", ndk.display().to_string()),
        ],
    );
    let units = measure(&env, &["android"]);
    assert!(units.iter().any(|u| u.path == canon(&ndk)));
}

/// Tempting wrong patch: propose both env vars when they name the same
/// directory. ANDROID_NDK_HOME == ANDROID_NDK_ROOT (outside the SDK)
/// must be one unit.
#[test]
fn ndk_home_and_root_naming_one_directory_are_one_unit() {
    let home = tempfile::tempdir().unwrap();
    let ndk = home.path().join("ndk");
    put(&ndk.join("toolchains/clang"), MB);
    let env = env_of(
        home.path(),
        &[
            (
                "ANDROID_HOME",
                home.path().join("sdk").display().to_string(),
            ),
            ("ANDROID_NDK_HOME", ndk.display().to_string()),
            ("ANDROID_NDK_ROOT", format!("{}/", ndk.display())),
        ],
    );
    let units = measure(&env, &["android"]);
    assert_eq!(
        units.iter().filter(|u| u.path == canon(&ndk)).count(),
        1,
        "{:?}",
        units.iter().map(|u| &u.path).collect::<Vec<_>>()
    );
    assert!(total(&units) < 2 * MB as u64);
}

/// Tempting wrong patch: treat an env var naming a FILE (or a missing
/// path, or a symlink loop) as a zero-byte unit.
#[test]
fn ndk_env_naming_a_file_a_missing_path_or_a_loop_is_never_a_zero_unit() {
    for kind in ["file", "missing", "loop"] {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("ndk-thing");
        match kind {
            "file" => put(&target, 10),
            "loop" => {
                std::os::unix::fs::symlink(home.path().join("ndk-other"), &target).unwrap();
                std::os::unix::fs::symlink(&target, home.path().join("ndk-other")).unwrap();
            }
            _ => {}
        }
        let env = env_of(
            home.path(),
            &[
                (
                    "ANDROID_HOME",
                    home.path().join("sdk").display().to_string(),
                ),
                ("ANDROID_NDK_HOME", target.display().to_string()),
            ],
        );
        let units = measure(&env, &["android"]);
        assert!(
            !units
                .iter()
                .any(|u| u.path.ends_with("ndk-thing") && u.bytes == 0 && u.note.is_none()),
            "{kind}: a zero unit with no note: {:?}",
            units
                .iter()
                .map(|u| (&u.path, u.bytes, &u.note))
                .collect::<Vec<_>>()
        );
    }
}

/// Tempting wrong patch: accept a relative env value verbatim. A
/// relative `ANDROID_NDK_HOME` would resolve against whatever the cwd of
/// `observe` happens to be; nothing measured may carry a relative path.
#[test]
fn a_relative_env_path_is_never_an_authorized_relative_root() {
    let home = tempfile::tempdir().unwrap();
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", "rel-sdk".to_string()),
            ("ANDROID_NDK_HOME", "rel-ndk".to_string()),
            ("IDF_TOOLS_PATH", "rel-idf".to_string()),
        ],
    );
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &only(&["android", "espressif"]), &[], &registry, 1);
    let (authorized, _) = scope.authorized_roots();
    let relative: Vec<_> = authorized
        .iter()
        .filter(|r| r.path.is_relative())
        .map(|r| r.path.clone())
        .collect();
    assert!(
        relative.is_empty(),
        "relative roots authorized: {relative:?}"
    );
}

/// SDK folders that are symlinks into a Homebrew prefix: with both
/// detectors in scope the bytes are counted once.
#[test]
fn an_sdk_folder_symlinked_into_homebrew_is_counted_once_across_detectors() {
    let home = tempfile::tempdir().unwrap();
    let prefix = home.path().join("brew");
    let real = prefix.join("share/android-commandlinetools/cmdline-tools");
    put(&real.join("latest/bin/sdkmanager"), 4 * MB);
    let sdk = home.path().join("sdk");
    fs::create_dir_all(&sdk).unwrap();
    std::os::unix::fs::symlink(&real, sdk.join("cmdline-tools")).unwrap();
    let env = env_of(
        home.path(),
        &[
            ("ANDROID_HOME", sdk.display().to_string()),
            ("HOMEBREW_PREFIX", prefix.display().to_string()),
        ],
    );
    let units = measure(&env, &["android", "homebrew"]);
    let t = total(&units);
    assert!(
        t < 6 * MB as u64,
        "4 MB counted as {t}: {:?}",
        units
            .iter()
            .map(|u| (&u.detector_id, &u.path, u.bytes))
            .collect::<Vec<_>>()
    );
}

// -------------------------------------------------------------- espressif

/// Only `dist/` exists: the other two are not zero-byte units.
#[test]
fn espressif_with_only_dist_measures_dist_and_invents_nothing() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".espressif");
    put(&root.join("dist/xtensa.tar.xz"), MB);
    let units = measure(&env_of(home.path(), &[]), &["espressif"]);
    assert_eq!(
        units.iter().map(|u| u.path.clone()).collect::<Vec<_>>(),
        vec![canon(&root.join("dist"))]
    );
    assert!(units[0].bytes >= MB as u64);
}

/// Tempting wrong patch: logical length instead of allocated blocks. A
/// 1 GiB sparse file in `dist/` occupies almost nothing.
#[test]
fn espressif_sparse_archive_is_measured_by_allocated_bytes() {
    let home = tempfile::tempdir().unwrap();
    let dist = home.path().join(".espressif/dist");
    fs::create_dir_all(&dist).unwrap();
    let f = fs::File::create(dist.join("sparse.tar.xz")).unwrap();
    f.set_len(1 << 30).unwrap();
    let units = measure(&env_of(home.path(), &[]), &["espressif"]);
    let u = units.iter().find(|u| u.path.ends_with("dist")).unwrap();
    assert!(
        u.bytes < 64 * MB as u64,
        "sparse file counted as {}",
        u.bytes
    );
}

/// Tempting wrong patch: a subdirectory that cannot be read contributes
/// zero silently. It must mark the unit incomplete (a lower bound).
#[test]
fn espressif_unreadable_subdirectory_is_labelled_not_silently_zero() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc_geteuid() } == 0 {
        return; // root reads through mode 000
    }
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join(".espressif/tools");
    put(&tools.join("xtensa/bin/gcc"), MB);
    let locked = tools.join("locked");
    put(&locked.join("x/big"), MB);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let units = measure(&env_of(home.path(), &[]), &["espressif"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let u = units.iter().find(|u| u.path.ends_with("tools")).unwrap();
    assert!(
        u.note.as_deref().is_some_and(|n| n.contains("incomplete")),
        "unreadable child left no mark: bytes={} note={:?}",
        u.bytes,
        u.note
    );
}

/// An unreadable ROOT (tools/ itself mode 000) is not a zero unit.
#[test]
fn espressif_unreadable_root_is_not_a_zero_unit() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc_geteuid() } == 0 {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join(".espressif/tools");
    put(&tools.join("xtensa/bin/gcc"), MB);
    fs::set_permissions(&tools, fs::Permissions::from_mode(0o000)).unwrap();
    let units = measure(&env_of(home.path(), &[]), &["espressif"]);
    fs::set_permissions(&tools, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !units
            .iter()
            .any(|u| u.path.ends_with("tools") && u.bytes == 0 && u.note.is_none()),
        "{:?}",
        units
            .iter()
            .map(|u| (&u.path, u.bytes, &u.note))
            .collect::<Vec<_>>()
    );
}

/// IDF_TOOLS_PATH naming a file / nothing: no units, and not the
/// `~/.espressif` fallback either (the override was set).
#[test]
fn idf_tools_path_to_a_file_does_not_fall_back_to_the_home_convention() {
    let home = tempfile::tempdir().unwrap();
    put(&home.path().join(".espressif/dist/a"), MB);
    let file = home.path().join("idf-file");
    put(&file, 10);
    let env = env_of(
        home.path(),
        &[("IDF_TOOLS_PATH", file.display().to_string())],
    );
    let units = measure(&env, &["espressif"]);
    assert!(
        units.is_empty(),
        "{:?}",
        units.iter().map(|u| &u.path).collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: the walker follows a symlink out of the root.
/// `tools/link -> /elsewhere/huge` must not add `huge`'s bytes.
#[test]
fn espressif_symlink_out_of_the_root_is_not_followed() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = home.path().join("elsewhere");
    put(&elsewhere.join("huge"), 8 * MB);
    let tools = home.path().join(".espressif/tools");
    put(&tools.join("a"), 4096);
    std::os::unix::fs::symlink(&elsewhere, tools.join("link")).unwrap();
    let units = measure(&env_of(home.path(), &[]), &["espressif"]);
    let u = units.iter().find(|u| u.path.ends_with("tools")).unwrap();
    assert!(u.bytes < MB as u64, "followed a symlink: {}", u.bytes);
}

unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

// ------------------------------------------------ catalog-wide invariants

/// Tempting wrong patch: `i32` uid, or `format!("{:?}")`. uid 0 and
/// u32::MAX render as plain decimals, and nothing else about the path
/// depends on the environment.
#[test]
fn scratch_uid_edges_render_as_decimal() {
    use swamp_core::locations::Detector;
    use swamp_core::locations::claude_code::ClaudeCodeScratchDetector;
    for (uid, want) in [
        (0u32, "/private/tmp/claude-0"),
        (501, "/private/tmp/claude-501"),
        (u32::MAX, "/private/tmp/claude-4294967295"),
    ] {
        let mut env = env_of(
            Path::new("/Users/dev"),
            &[("TMPDIR", "/elsewhere".to_string())],
        );
        env.uid = uid;
        let got = ClaudeCodeScratchDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path.as_deref(), Some(Path::new(want)));
    }
}

/// Registry detector ids are unique, including the two new ones.
#[test]
fn detector_ids_are_unique_and_include_the_new_ones() {
    let r = Registry::with_builtins();
    let mut ids: Vec<&str> = r.detectors().iter().map(|d| d.id()).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n);
    for id in ["espressif", "xcode-system", "claude-code-scratch"] {
        assert!(ids.contains(&id), "{id} not registered");
    }
}

/// Docs-matches-code: every path the new detectors propose on macOS is
/// named in docs/locations.md (in its `<...>` or literal spelling).
#[test]
fn every_new_location_is_named_in_docs_locations() {
    let docs = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/locations.md"
    ))
    .unwrap();
    for needle in [
        "IDF_TOOLS_PATH",
        "python_env",
        "dist",
        "ANDROID_NDK_HOME",
        "ANDROID_NDK_ROOT",
        "cmdline-tools",
        "platform-tools",
        "cmake",
        "CommandLineTools",
        "DeveloperDiskImages",
        "CoreDevice",
        "DeviceKit",
        "Cryptex",
        "Profiles",
        "claude-<uid>",
        "AssetsV2",
    ] {
        assert!(
            docs.contains(needle),
            "docs/locations.md does not name {needle}"
        );
    }
}

/// Catalog version is bumped past the v0.7.5 value and is the value
/// the scope records.
#[test]
fn catalog_version_is_bumped_and_recorded_in_scope() {
    use swamp_core::locations::CATALOG_VERSION;
    assert!(CATALOG_VERSION > "2026-09-21.4", "{CATALOG_VERSION}");
    let home = tempfile::tempdir().unwrap();
    let scope = resolve_effective_scope(
        &env_of(home.path(), &[]),
        &only(&["espressif"]),
        &[],
        &Registry::with_builtins(),
        1,
    );
    assert_eq!(scope.catalog_version, CATALOG_VERSION);
}
