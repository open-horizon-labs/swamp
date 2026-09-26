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

use super::{
    CommandOutcome, Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance,
    StorageCategory,
};

pub const HOMEBREW_DETECTOR_ID: &str = "homebrew";

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
