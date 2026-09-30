//! Homebrew: prefix from `HOMEBREW_PREFIX`, else the conventional
//! `/opt/homebrew` (Apple Silicon) and `/usr/local` (Intel) prefixes,
//! optionally corroborated by the read-only `brew --prefix` query.
//! https://docs.brew.sh/Manpage
//!
//! The conventional prefixes are always proposed, whether or not the
//! tool query runs or succeeds: a leftover Homebrew install/cache must
//! still be found after `brew` itself is gone from `PATH`. The tool
//! query is a bonus corroboration, not the only path to resolution, and
//! its failure is reported on its own entry rather than swallowed.
//!
//! Three detectors share the prefix, so a machine's Homebrew bytes are
//! reported in two honest parts by default and can be reported in full
//! (#174):
//!
//! * `homebrew-devtools` (on by default): only the unambiguous
//!   developer tooling: language toolchains and build tools on
//!   [`DEV_FORMULAE`], casks on [`DEV_CASKS`], and Homebrew's Android
//!   command-line tools. Each such formula is its own unit.
//! * `homebrew-other` (on by default): the rest of the prefix, measured
//!   as one remainder unit with the dev units subtracted, so the two add
//!   up to the prefix.
//! * `homebrew` (off by default): the full detector, whole Cellar and
//!   Caskroom. `[scan] enabled_detectors = ["homebrew"]` turns it on;
//!   it then takes the prefix, Cellar and Caskroom paths over from the
//!   other two, so nothing is counted twice.

use super::{
    CommandOutcome, Detector, Environment, LastUseDecl, LastUseSource, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const HOMEBREW_DETECTOR_ID: &str = "homebrew";
pub const HOMEBREW_DEVTOOLS_DETECTOR_ID: &str = "homebrew-devtools";
pub const HOMEBREW_OTHER_DETECTOR_ID: &str = "homebrew-other";

/// Formulae that are unambiguously developer tooling: language toolchains
/// and build tools. Matched on the whole name after [`formula_base`]
/// strips a `@version` suffix, never on a substring: `gopls` is not `go`.
/// Documented row by row in `docs/locations.md` (a test keeps the two in
/// step). Ambiguous tools (qemu, ansible, pandoc, duckdb, mlx) are
/// deliberately absent: they land in `Homebrew (other)`.
pub const DEV_FORMULAE: &[&str] = &[
    "llvm",
    "gcc",
    "swift",
    "bun",
    "deno",
    "bazel",
    "bazelisk",
    "uv",
    "kotlin",
    "openjdk",
    "dotnet",
    "zig",
    "go",
    "rust",
    "rustup",
    "node",
    "python",
    "ruby",
    "cmake",
    "gradle",
    "maven",
    "ninja",
    "mise",
    "terraform",
];

/// Casks that are unambiguously developer tooling.
pub const DEV_CASKS: &[&str] = &["android-platform-tools", "android-studio"];

/// Where Homebrew puts the Android command-line tools, relative to a
/// prefix. The `android` detector already measures the SDK directories
/// inside it (`system-images`, `emulator`, ...); the observation pass
/// subtracts those from this unit, so each byte is counted once.
pub const ANDROID_CMDLINE_TOOLS: &str = "share/android-commandlinetools";

/// A formula name without its `@version` suffix: `llvm@20` is `llvm`.
pub fn formula_base(name: &str) -> &str {
    name.split_once('@').map_or(name, |(base, _)| base)
}

fn is_dev_formula(name: &str) -> bool {
    DEV_FORMULAE.contains(&formula_base(name))
}

fn is_dev_cask(name: &str) -> bool {
    DEV_CASKS.contains(&name)
}

/// Every prefix this pass resolves without running a tool: the
/// environment override, else the platform's conventions.
fn prefixes(env: &Environment) -> Vec<(std::path::PathBuf, Provenance)> {
    if let Some(prefix) = env.env_var("HOMEBREW_PREFIX").filter(|v| !v.is_empty()) {
        return vec![(
            std::path::PathBuf::from(prefix),
            Provenance::EnvVar("HOMEBREW_PREFIX".to_string()),
        )];
    }
    let conventional: &[&str] = match env.platform {
        Platform::MacOS => &["/opt/homebrew", "/usr/local"],
        Platform::Linux => &["/home/linuxbrew/.linuxbrew"],
    };
    conventional
        .iter()
        .map(|p| (std::path::PathBuf::from(p), Provenance::BuiltinConvention))
        .collect()
}

/// Whether `prefix` is a Homebrew prefix rather than a directory Homebrew
/// merely may share. `/opt/homebrew` and the Linux prefix are Homebrew's
/// alone; `/usr/local` belongs to the system and is claimed as Homebrew's
/// only where a Cellar or the Intel `Homebrew` directory exists.
fn is_homebrew_prefix(prefix: &std::path::Path, provenance: &Provenance) -> bool {
    if *provenance != Provenance::BuiltinConvention || prefix != std::path::Path::new("/usr/local")
    {
        return true;
    }
    crate::fs_gate::exists(prefix.join("Cellar")) || crate::fs_gate::exists(prefix.join("Homebrew"))
}

const REINSTALL: super::RecoveryHint = super::RecoveryHint {
    command: "brew reinstall <formula>",
    cost: super::RecoveryCost::NetworkRefetch,
};

/// The default-on part: unambiguous developer tooling only.
pub struct HomebrewDevToolsDetector;

impl Detector for HomebrewDevToolsDetector {
    fn id(&self) -> &'static str {
        HOMEBREW_DEVTOOLS_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Homebrew (dev tooling)"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Homebrew Cellar/Caskroom layout; allowlist in docs/locations.md"
    }

    fn recovery_hint(&self) -> Option<super::RecoveryHint> {
        Some(REINSTALL)
    }

    fn group(&self) -> Option<&'static str> {
        Some(HOMEBREW_DETECTOR_ID)
    }

    fn last_use_sources(&self) -> &'static [LastUseDecl] {
        // One unit per allowlisted formula (`Cellar/<formula>`): its
        // `<version>/bin/*` are the key files. The legacy whole-Cellar
        // detector declares the same layout one level up.
        &[LastUseDecl {
            anchor: StoreAnchor::Categorized {
                category: StorageCategory::Installation,
                suffix: &[],
            },
            source: LastUseSource::KeyFileAtime { max_depth: 2 },
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let mut out = Vec::new();
        for (prefix, provenance) in prefixes(env) {
            if !is_homebrew_prefix(&prefix, &provenance) {
                continue;
            }
            for (rel, note) in [
                (
                    ANDROID_CMDLINE_TOOLS,
                    "Android command-line tools installed by Homebrew; the android \
                     detector's SDK directories inside it are counted there, once",
                ),
                (
                    "Cellar",
                    "installed formulae: only the allowlisted toolchains and build \
                     tools are measured, each on its own",
                ),
                (
                    "Caskroom",
                    "installed casks: only the allowlisted developer casks are \
                     measured, each on its own",
                ),
            ] {
                out.push(ProposedLocation {
                    detector_id: HOMEBREW_DEVTOOLS_DETECTOR_ID.to_string(),
                    path: Some(prefix.join(rel)),
                    category: StorageCategory::Installation,
                    provenance: provenance.clone(),
                    status: LocationStatus::Resolved,
                    note: Some(note.to_string()),
                });
            }
        }
        out
    }

    fn select_children(
        &self,
        container: &std::path::Path,
        names: &[String],
    ) -> Option<Vec<String>> {
        let keep: fn(&str) -> bool = match container.file_name()?.to_str()? {
            "Cellar" => is_dev_formula,
            "Caskroom" => is_dev_cask,
            _ => return None,
        };
        Some(names.iter().filter(|n| keep(n)).cloned().collect())
    }
}

/// The default-on remainder: the rest of the prefix as one unit.
pub struct HomebrewOtherDetector;

impl Detector for HomebrewOtherDetector {
    fn id(&self) -> &'static str {
        HOMEBREW_OTHER_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Homebrew (other)"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Homebrew prefix conventions; the remainder after the dev tooling units"
    }

    // No recovery hint: the remainder is the prefix minus the dev tooling
    // (Homebrew itself, `bin`, `lib`, GUI casks); no one command
    // re-obtains it as a unit.
    fn group(&self) -> Option<&'static str> {
        Some(HOMEBREW_DETECTOR_ID)
    }

    fn remainder_of(&self) -> Option<super::Remainder> {
        Some(super::Remainder {
            of: HOMEBREW_DEVTOOLS_DETECTOR_ID,
            include_all: "[scan] enabled_detectors = [\"homebrew\"]",
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let mut out = Vec::new();
        for (prefix, provenance) in prefixes(env) {
            if !is_homebrew_prefix(&prefix, &provenance) {
                continue;
            }
            // `/usr/local` is the system's; on an Intel Mac Homebrew owns
            // only Cellar, Caskroom and Homebrew inside it, so only those
            // are measured, never the directory whole.
            let paths: Vec<std::path::PathBuf> = if prefix == std::path::Path::new("/usr/local") {
                ["Cellar", "Caskroom", "Homebrew"]
                    .iter()
                    .map(|d| prefix.join(d))
                    .collect()
            } else {
                vec![prefix]
            };
            for path in paths {
                out.push(ProposedLocation {
                    detector_id: HOMEBREW_OTHER_DETECTOR_ID.to_string(),
                    path: Some(path),
                    category: StorageCategory::Installation,
                    provenance: provenance.clone(),
                    status: LocationStatus::Resolved,
                    note: Some(
                        "everything Homebrew owns here that is not dev tooling (other \
                         formulae, casks, libraries); `[scan] enabled_detectors = \
                         [\"homebrew\"]` reports Cellar and Caskroom whole instead"
                            .to_string(),
                    ),
                });
            }
        }
        out
    }
}

pub struct HomebrewDetector;

impl Detector for HomebrewDetector {
    fn id(&self) -> &'static str {
        HOMEBREW_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Homebrew"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Homebrew manpage, current stable prefixes (macOS /opt/homebrew, /usr/local; \
         Linux /home/linuxbrew/.linuxbrew)"
    }

    /// A system-wide install tree, not a per-user one: `/opt/homebrew`
    /// (or `/usr/local`) is shared by every account on the machine,
    /// installed once regardless of which developer runs swamp, and its
    /// Cellar/Caskroom can hold GUI applications and system tools with
    /// nothing to do with any one project. Off by default; `[scan]
    /// enabled_detectors = ["homebrew"]` turns it on.
    fn default_enabled(&self) -> bool {
        false
    }

    fn last_use_sources(&self) -> &'static [LastUseDecl] {
        &[LastUseDecl {
            anchor: StoreAnchor::Categorized {
                category: StorageCategory::Installation,
                suffix: &["Cellar"],
            },
            source: LastUseSource::KeyFileAtime { max_depth: 3 },
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let mut out = Vec::new();
        // Every prefix this pass resolved, so Cellar/Caskroom (#49) can
        // be proposed under each one below without duplicating the
        // env-var-vs-convention branching above.
        let mut prefixes: Vec<std::path::PathBuf> = Vec::new();

        if let Some(prefix) = env.env_var("HOMEBREW_PREFIX").filter(|v| !v.is_empty()) {
            let path = std::path::PathBuf::from(prefix);
            out.push(ProposedLocation {
                detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                path: Some(path.clone()),
                category: StorageCategory::Installation,
                provenance: Provenance::EnvVar("HOMEBREW_PREFIX".to_string()),
                status: LocationStatus::Resolved,
                note: Some("Homebrew install prefix (from HOMEBREW_PREFIX)".to_string()),
            });
            prefixes.push(path);
        } else {
            // Homebrew on Linux installs to one prefix, not two: the
            // Apple-silicon/Intel split does not exist there, and
            // /usr/local on Linux is a distribution-owned directory
            // Homebrew does not claim. Proposing it would be a scan root
            // that has nothing to do with Homebrew.
            let conventional: &[&str] = match env.platform {
                Platform::MacOS => &["/opt/homebrew", "/usr/local"],
                Platform::Linux => &["/home/linuxbrew/.linuxbrew"],
            };
            for prefix in conventional.iter().copied() {
                out.push(ProposedLocation {
                    detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                    path: Some(std::path::PathBuf::from(prefix)),
                    category: StorageCategory::Installation,
                    provenance: Provenance::BuiltinConvention,
                    status: LocationStatus::Resolved,
                    note: Some("conventional Homebrew install prefix".to_string()),
                });
                prefixes.push(std::path::PathBuf::from(prefix));
            }
            match env.run_command("brew", &["--prefix"]) {
                Ok(CommandOutcome {
                    stdout,
                    success: true,
                }) if !stdout.is_empty() => {
                    let path = std::path::PathBuf::from(&stdout);
                    out.push(ProposedLocation {
                        detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                        path: Some(path.clone()),
                        category: StorageCategory::Installation,
                        provenance: Provenance::ToolQuery("brew --prefix".to_string()),
                        status: LocationStatus::Resolved,
                        note: Some("corroborated by `brew --prefix`".to_string()),
                    });
                    if !prefixes.contains(&path) {
                        prefixes.push(path);
                    }
                }
                Ok(CommandOutcome { success: false, .. }) => {
                    out.push(ProposedLocation {
                        detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                        path: None,
                        category: StorageCategory::Installation,
                        provenance: Provenance::ToolQuery("brew --prefix".to_string()),
                        status: LocationStatus::UnresolvedWithReason {
                            reason: "brew --prefix exited non-zero".to_string(),
                        },
                        note: None,
                    });
                }
                Err(reason) => {
                    out.push(ProposedLocation {
                        detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                        path: None,
                        category: StorageCategory::Installation,
                        provenance: Provenance::ToolQuery("brew --prefix".to_string()),
                        status: LocationStatus::UnresolvedWithReason { reason },
                        note: Some(
                            "conventional prefixes above are still proposed; brew may simply be absent"
                                .to_string(),
                        ),
                    });
                }
                Ok(CommandOutcome { stdout, .. }) if stdout.is_empty() => {}
                Ok(_) => {}
            }
        }

        // Cellar (installed formula versions) and Caskroom (installed
        // cask app bundles) under every resolved prefix -- #49's
        // refinement over the earlier "just the prefix" registration.
        for prefix in &prefixes {
            out.push(ProposedLocation {
                detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                path: Some(prefix.join("Cellar")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("installed formula versions".to_string()),
            });
            out.push(ProposedLocation {
                detector_id: HOMEBREW_DETECTOR_ID.to_string(),
                path: Some(prefix.join("Caskroom")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("installed cask app bundles".to_string()),
            });
        }

        out.push(ProposedLocation {
            detector_id: HOMEBREW_DETECTOR_ID.to_string(),
            path: Some(env.home.join("Library/Caches/Homebrew")),
            category: StorageCategory::Downloads,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some("downloaded bottles/sources cache".to_string()),
        });

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Tempting wrong patch: compare `@version` names literally, so
    /// `llvm@20` and `llvm@21` miss `llvm`; or use `contains`/`starts_with`.
    #[test]
    fn versioned_names_match_and_substrings_never_do() {
        let cellar = std::path::Path::new("/opt/homebrew/Cellar");
        let picked = HomebrewDevToolsDetector
            .select_children(
                cellar,
                &names(&[
                    "llvm@20",
                    "llvm@21",
                    "openjdk@17",
                    "zig@0.15",
                    "go",
                    "gopls",
                    "zigbee2mqtt",
                    "nodejs-foo",
                    "node-build",
                    "qemu",
                    "pandoc",
                    "duckdb",
                    "mlx",
                    "ansible",
                ]),
            )
            .unwrap();
        assert_eq!(
            picked,
            names(&["llvm@20", "llvm@21", "openjdk@17", "zig@0.15", "go"])
        );
    }

    /// Tempting wrong patch: one allowlist for formulae and casks, so a
    /// cask named like a formula, or Outlook, leaks in.
    #[test]
    fn casks_use_their_own_allowlist() {
        let caskroom = std::path::Path::new("/opt/homebrew/Caskroom");
        let picked = HomebrewDevToolsDetector
            .select_children(
                caskroom,
                &names(&[
                    "android-platform-tools",
                    "android-studio",
                    "microsoft-outlook",
                    "zoom",
                    "go",
                ]),
            )
            .unwrap();
        assert_eq!(picked, names(&["android-platform-tools", "android-studio"]));
    }

    /// The observation pass asks with no names to learn whether a path is a
    /// container; only Cellar and Caskroom are, everything else is a plain
    /// location.
    #[test]
    fn only_cellar_and_caskroom_are_containers() {
        let d = HomebrewDevToolsDetector;
        assert!(
            d.select_children(std::path::Path::new("/p/Cellar"), &[])
                .is_some()
        );
        assert!(
            d.select_children(std::path::Path::new("/p/Caskroom"), &[])
                .is_some()
        );
        assert!(
            d.select_children(
                std::path::Path::new("/p/share/android-commandlinetools"),
                &[]
            )
            .is_none()
        );
        assert!(
            HomebrewOtherDetector
                .select_children(std::path::Path::new("/p/Cellar"), &[])
                .is_none()
        );
    }

    /// Docs-matches-code: every allowlisted name is documented in
    /// `docs/locations.md`, and the ambiguous tools the maintainer ruled
    /// out are not on the list.
    #[test]
    fn every_allowlisted_name_is_documented_and_ambiguous_tools_are_absent() {
        let docs = include_str!("../../../../docs/locations.md");
        let section = docs
            .split("### Homebrew dev-tooling allowlist")
            .nth(1)
            .expect("docs/locations.md has the Homebrew allowlist section")
            .split("\n## ")
            .next()
            .unwrap();
        for name in DEV_FORMULAE.iter().chain(DEV_CASKS) {
            assert!(
                section.contains(&format!("`{name}`")),
                "{name} is allowlisted but not documented in docs/locations.md"
            );
        }
        for ambiguous in ["qemu", "ansible", "pandoc", "duckdb", "mlx"] {
            assert!(!DEV_FORMULAE.contains(&ambiguous));
            assert!(!DEV_CASKS.contains(&ambiguous));
        }
        assert!(docs.contains(ANDROID_CMDLINE_TOOLS));
    }

    #[test]
    fn a_shared_system_prefix_is_not_claimed_without_a_cellar() {
        // /usr/local only counts as Homebrew's where a Cellar or Homebrew
        // directory exists; an explicit or private prefix always counts.
        assert!(is_homebrew_prefix(
            std::path::Path::new("/opt/homebrew"),
            &Provenance::BuiltinConvention
        ));
        assert!(is_homebrew_prefix(
            std::path::Path::new("/usr/local"),
            &Provenance::EnvVar("HOMEBREW_PREFIX".into())
        ));
    }
    use crate::locations::FakeCommandRunner;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn conventional_prefixes_when_no_runner_configured() {
        // Default fixture environment has no command runner wired in;
        // the detector must still propose both conventional prefixes
        // plus the downloads cache, and must not panic or block.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = HomebrewDetector.detect(&env);
        let paths: Vec<_> = got.iter().filter_map(|l| l.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("/opt/homebrew")));
        assert!(paths.contains(&PathBuf::from("/usr/local")));
        assert!(paths.contains(&PathBuf::from("/Users/dev/Library/Caches/Homebrew")));
        assert!(
            got.iter()
                .any(|l| matches!(l.status, LocationStatus::UnresolvedWithReason { .. })),
            "the failed tool query must be reported, not silently dropped"
        );
    }

    /// #49: Cellar/Caskroom are proposed under *every* resolved prefix,
    /// distinctly from the prefix itself and from the downloads cache --
    /// the tempting shortcut this catches is treating the bare prefix
    /// path as "installed formulas" without descending into Cellar.
    #[test]
    fn cellar_and_caskroom_are_proposed_under_each_prefix() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = HomebrewDetector.detect(&env);
        for prefix in ["/opt/homebrew", "/usr/local"] {
            assert!(
                got.iter()
                    .any(|l| l.path == Some(PathBuf::from(prefix).join("Cellar"))),
                "missing Cellar under {prefix}"
            );
            assert!(
                got.iter()
                    .any(|l| l.path == Some(PathBuf::from(prefix).join("Caskroom"))),
                "missing Caskroom under {prefix}"
            );
        }
    }

    #[test]
    fn env_var_skips_tool_query_entirely() {
        let fake = Arc::new(FakeCommandRunner::new());
        let mut env_vars = HashMap::new();
        env_vars.insert("HOMEBREW_PREFIX".to_string(), "/custom/prefix".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS)
            .with_runner(fake.clone());
        let got = HomebrewDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/custom/prefix")))
        );
        assert!(
            fake.calls().is_empty(),
            "an env override must not still shell out"
        );
    }

    #[test]
    fn tool_query_only_ever_calls_the_allow_listed_command() {
        let fake =
            Arc::new(FakeCommandRunner::new().with_answer("brew", &["--prefix"], "/opt/homebrew"));
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS)
                .with_runner(fake.clone());
        let _ = HomebrewDetector.detect(&env);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "brew");
        assert_eq!(calls[0].1, vec!["--prefix".to_string()]);
    }

    #[test]
    fn a_disallowed_command_is_refused_never_executed() {
        // Simulates a hypothetical detector bug asking for a command
        // outside the allow-list: FakeCommandRunner records the attempt
        // and refuses it exactly like SystemCommandRunner would, proving
        // no side-effecting call would have gone out for real.
        let fake = Arc::new(FakeCommandRunner::new());
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS)
                .with_runner(fake.clone());
        let outcome = env.run_command("brew", &["install", "something"]);
        assert!(outcome.is_err());
        assert_eq!(
            fake.calls(),
            vec![(
                "brew".to_string(),
                vec!["install".to_string(), "something".to_string()]
            )]
        );
    }
}
