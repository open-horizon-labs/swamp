//! The built-in default scan roots, as a detector (#41/#44).
//!
//! Which roots those are is per platform, and is the table in
//! [`BuiltinDefaultsDetector::detect`] -- four on macOS, two on Linux.
//!
//! Modeled as a detector -- not a hard-coded list inside `scope.rs` --
//! so `[scan] defaults = false` and `disabled_detectors` are the same
//! mechanism a real detector uses: `crate::scope::resolve_effective_scope`
//! folds `defaults = false` into "also disable `builtin-defaults`" before
//! calling [`super::Registry::resolve`]. See `crate::scope` for exactly
//! how that folding happens; this file only proposes the candidates.

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const BUILTIN_DEFAULTS_DETECTOR_ID: &str = "builtin-defaults";

pub struct BuiltinDefaultsDetector;

impl Detector for BuiltinDefaultsDetector {
    fn id(&self) -> &'static str {
        BUILTIN_DEFAULTS_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Built-in default roots"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "macOS: ~/src, ~/Library/Developer, ~/Library/Caches, $XDG_CACHE_HOME (default ~/.cache); \
         Linux: ~/src, $XDG_CACHE_HOME (default ~/.cache)"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::GenericCacheBuildOutputs,
            anchor: StoreAnchor::Categorized {
                category: StorageCategory::Cache,
                suffix: &[],
            },
        }]
    }

    /// Each platform gets the roots that platform actually has.
    ///
    /// `~/src` is shared: it is a habit, not an OS convention. The other
    /// two macOS-specific roots have no Linux counterpart worth substituting.
    /// `~/Library/Developer` is Xcode's; no Linux directory holds "the
    /// SDK and simulator storage of the platform toolchain", and
    /// proposing `/usr/lib` or a distribution's package cache would mean
    /// walking system-owned storage a user cannot act on without root --
    /// which this tool never asks for. `~/Library/Caches` is the native
    /// macOS cache root. Include `$XDG_CACHE_HOME` (default `~/.cache`)
    /// too, since cross-platform developer tools use that conventional
    /// per-user cache location on macOS as well as Linux.
    ///
    /// A relative `$XDG_CACHE_HOME` is ignored, as the XDG base
    /// directory spec requires ("if an implementation encounters a
    /// relative path in any of these variables it should consider the
    /// path invalid and ignore it") -- honouring one would propose a
    /// root relative to whatever the process's working directory was.
    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let candidates: Vec<(std::path::PathBuf, &str, StorageCategory)> = match env.platform {
            Platform::MacOS => vec![
                (
                    env.home.join("src"),
                    "checkouts a developer keeps under a home-relative src tree",
                    StorageCategory::Unclassified,
                ),
                (
                    env.home.join("Library/Developer"),
                    "Xcode/Android build, SDK, and simulator storage",
                    StorageCategory::Unclassified,
                ),
                (
                    env.home.join("Library/Caches"),
                    "the platform-wide user cache directory",
                    StorageCategory::Cache,
                ),
                (
                    xdg_cache_home(env),
                    "the XDG per-user cache root ($XDG_CACHE_HOME, default ~/.cache), \
                     where cross-platform developer tooling caches accumulate",
                    StorageCategory::Cache,
                ),
            ],
            Platform::Linux => vec![
                (
                    env.home.join("src"),
                    "checkouts a developer keeps under a home-relative src tree",
                    StorageCategory::Unclassified,
                ),
                (
                    xdg_cache_home(env),
                    "the XDG per-user cache root ($XDG_CACHE_HOME, default ~/.cache), \
                     where Linux build and package tooling caches accumulate",
                    StorageCategory::Cache,
                ),
            ],
        };
        candidates
            .into_iter()
            .map(|(path, note, category)| ProposedLocation {
                detector_id: BUILTIN_DEFAULTS_DETECTOR_ID.to_string(),
                path: Some(path),
                category,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(note.to_string()),
            })
            .collect()
    }
}

fn xdg_cache_home(env: &Environment) -> std::path::PathBuf {
    match env.env_var("XDG_CACHE_HOME").map(std::path::PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => env.home.join(".cache"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn macos_proposes_native_and_xdg_cache_roots() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = BuiltinDefaultsDetector.detect(&env);
        let paths: Vec<_> = got.iter().filter_map(|l| l.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Users/dev/src"),
                PathBuf::from("/Users/dev/Library/Developer"),
                PathBuf::from("/Users/dev/Library/Caches"),
                PathBuf::from("/Users/dev/.cache"),
            ]
        );
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }

    #[test]
    fn linux_proposes_src_and_the_xdg_cache_root() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = BuiltinDefaultsDetector.detect(&env);
        let paths: Vec<_> = got.iter().filter_map(|l| l.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/dev/src"),
                PathBuf::from("/home/dev/.cache"),
            ]
        );
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }

    #[test]
    fn linux_honours_an_absolute_xdg_cache_home() {
        let env = Environment::fixture(
            PathBuf::from("/home/dev"),
            HashMap::from([("XDG_CACHE_HOME".to_string(), "/scratch/cache".to_string())]),
            Platform::Linux,
        );
        let paths: Vec<_> = BuiltinDefaultsDetector
            .detect(&env)
            .into_iter()
            .filter_map(|l| l.path)
            .collect();
        assert!(
            paths.contains(&PathBuf::from("/scratch/cache")),
            "an explicit XDG_CACHE_HOME must replace ~/.cache, got {paths:?}"
        );
        assert!(!paths.contains(&PathBuf::from("/home/dev/.cache")));
    }

    #[test]
    fn macos_honours_an_absolute_xdg_cache_home_and_keeps_native_caches() {
        let env = Environment::fixture(
            PathBuf::from("/Users/dev"),
            HashMap::from([("XDG_CACHE_HOME".to_string(), "/Volumes/cache".to_string())]),
            Platform::MacOS,
        );
        let paths: Vec<_> = BuiltinDefaultsDetector
            .detect(&env)
            .into_iter()
            .filter_map(|l| l.path)
            .collect();
        assert!(paths.contains(&PathBuf::from("/Volumes/cache")));
        assert!(paths.contains(&PathBuf::from("/Users/dev/Library/Caches")));
        assert!(!paths.contains(&PathBuf::from("/Users/dev/.cache")));
    }

    /// The XDG spec says a relative value is invalid and must be
    /// ignored. Joining it would propose a scan root relative to the
    /// process's working directory -- a root that means something
    /// different every time swamp is run from a different place.
    #[test]
    fn a_relative_xdg_cache_home_falls_back_to_the_home_default() {
        let env = Environment::fixture(
            PathBuf::from("/home/dev"),
            HashMap::from([("XDG_CACHE_HOME".to_string(), "cache".to_string())]),
            Platform::Linux,
        );
        let paths: Vec<_> = BuiltinDefaultsDetector
            .detect(&env)
            .into_iter()
            .filter_map(|l| l.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/dev/src"),
                PathBuf::from("/home/dev/.cache"),
            ]
        );
    }

    #[test]
    fn macos_ignores_a_relative_xdg_cache_home() {
        let env = Environment::fixture(
            PathBuf::from("/Users/dev"),
            HashMap::from([("XDG_CACHE_HOME".to_string(), "cache".to_string())]),
            Platform::MacOS,
        );
        let paths: Vec<_> = BuiltinDefaultsDetector
            .detect(&env)
            .into_iter()
            .filter_map(|l| l.path)
            .collect();
        assert!(paths.contains(&PathBuf::from("/Users/dev/.cache")));
        assert!(paths.contains(&PathBuf::from("/Users/dev/Library/Caches")));
        assert!(!paths.contains(&PathBuf::from("cache")));
    }

    /// The property that must survive every future edit to this table:
    /// macOS Library roots must not appear in the Linux build.
    #[test]
    fn native_library_conventions_do_not_leak_into_linux() {
        let linux: Vec<String> = BuiltinDefaultsDetector
            .detect(&Environment::fixture(
                PathBuf::from("/home/dev"),
                HashMap::new(),
                Platform::Linux,
            ))
            .into_iter()
            .filter_map(|l| l.path)
            .map(|p| p.display().to_string())
            .collect();
        assert!(
            linux.iter().all(|p| !p.contains("/Library/")),
            "a macOS Library path leaked into the Linux defaults: {linux:?}"
        );
    }
}
