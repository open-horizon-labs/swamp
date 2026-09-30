//! Single-directory tool stores that have no interior worth naming:
//! ESP-IDF's `dist/`, `tools/` and `python_env/` under `IDF_TOOLS_PATH`
//! (or `~/.espressif`), and Claude Code's per-user session scratch
//! directory. Each is one unit, its own root row, with what removing it
//! costs in the tool's own words.
//!
//! Nothing here reads a file's content, runs a tool, or offers an
//! action: `swamp` reports, the human removes.

use super::layout::name_of;
use super::{BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, NestedUnitBuilder};
use crate::artifact::{ArtifactRole, Membership, NestedArtifact};
use crate::locations::BuildStoreKind;
use std::path::{Path, PathBuf};

pub struct Adapter;

impl BuildAdapter for Adapter {
    fn id(&self) -> &'static str {
        "tool-stores"
    }

    fn name(&self) -> &'static str {
        "Tool stores (ESP-IDF, agent scratch)"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: false,
            actions_available: false,
        }
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[
            BuildStoreKind::EspressifDist,
            BuildStoreKind::EspressifTools,
            BuildStoreKind::EspressifPythonEnv,
            BuildStoreKind::AgentScratch,
        ]
    }

    fn containers(&self, _project_root: &Path, _candidates: &[PathBuf]) -> Vec<BuildContainer> {
        Vec::new()
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        let (role, reason, reason_short, consequence, no_action) = match container.store_kind {
            Some(BuildStoreKind::EspressifDist) => (
                ArtifactRole::Intermediate,
                "ESP-IDF's `dist/`: the tool archives idf_tools.py downloaded",
                "ESP-IDF's `dist/`",
                "idf_tools.py downloads the archive again the next time it installs that tool -- \
                 a download",
                "ESP-IDF installs read these archives",
            ),
            Some(BuildStoreKind::EspressifTools) => (
                ArtifactRole::Installation,
                "ESP-IDF's `tools/`: the installed compilers, debuggers and other tools",
                "ESP-IDF's `tools/`",
                "reinstall with ESP-IDF's `install.sh` (or `idf_tools.py install`)",
                "ESP-IDF projects on this machine use these tools",
            ),
            Some(BuildStoreKind::EspressifPythonEnv) => (
                ArtifactRole::Installation,
                "ESP-IDF's `python_env/`: one Python virtual environment per ESP-IDF version",
                "ESP-IDF's `python_env/`",
                "recreate with ESP-IDF's `install.sh` (or `idf_tools.py install-python-env`)",
                "ESP-IDF's export script activates these environments",
            ),
            Some(BuildStoreKind::AgentScratch) => (
                ArtifactRole::Intermediate,
                "Claude Code's per-user session scratch directory",
                "Claude Code's session scratch",
                "session scratch; removing it during a session breaks that session \
                 (swamp's statement; Claude Code documents no such directory)",
                "a running Claude Code session may be writing here",
            ),
            _ => return Vec::new(),
        };
        let mut units = vec![
            NestedUnitBuilder::container_root(container, ctx, role.clone())
                .supported_with_reason(reason)
                .membership(Membership::Unknown)
                .consequence(consequence)
                .no_action_because(no_action)
                .build(),
        ];
        // One unit per directory directly inside, named for what it is:
        // an ESP-IDF tool, a Python environment, a per-project scratch
        // directory. `dist/` holds archive *files*, which the folded
        // index (directories only) does not list, so it stays one row.
        for child in ctx.folded().children(&container.path) {
            let name = name_of(&child.path);
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    role.clone(),
                    child,
                    format!("`{name}` under {reason_short}"),
                    consequence,
                )
                .no_action_because(no_action)
                .build(),
            );
        }
        units
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_adapters::{ContainerCache, FoldedDir, FoldedIndex};
    use crate::fs_events::EventCoverage;

    fn run(kind: BuildStoreKind, path: &Path, measured: bool) -> Vec<NestedArtifact> {
        let dirs = if measured {
            vec![FoldedDir {
                path: path.to_path_buf(),
                allocated_total: 8192,
                mtime_max: 5,
                complete: true,
            }]
        } else {
            Vec::new()
        };
        let idx = FoldedIndex::from_dirs(dirs);
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let c = BuildContainer::shared_store_of("tool-stores", path.to_path_buf(), kind);
        Adapter.identify(&c, &BuildCtx::new(1_000_000, &idx, &none, &cache))
    }

    #[test]
    fn each_store_is_one_root_unit_with_the_tools_own_consequence() {
        let cases = [
            (BuildStoreKind::EspressifDist, "downloads the archive again"),
            (BuildStoreKind::EspressifTools, "install.sh"),
            (BuildStoreKind::EspressifPythonEnv, "install-python-env"),
            (
                BuildStoreKind::AgentScratch,
                "session scratch; removing it during a session breaks that session",
            ),
        ];
        for (kind, needle) in cases {
            let path = PathBuf::from("/fixture/store");
            let units = run(kind, &path, true);
            assert_eq!(units.len(), 1, "{kind:?}: no child directories, one row");
            assert_eq!(units[0].path, path);
            let text = units[0].consequence.clone().unwrap();
            assert!(text.contains(needle), "{kind:?}: {text}");
            assert!(
                matches!(
                    units[0].action,
                    crate::artifact::NestedActionCapability::Unsupported { .. }
                ),
                "{kind:?} offers an action"
            );
        }
    }

    #[test]
    fn each_directory_inside_is_its_own_unit_with_the_same_consequence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("claude-501");
        for d in ["-Users-a-proj", "-Users-b-proj"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("cache-break-state-1.json"), b"{}").unwrap();
        let dirs = vec![
            FoldedDir {
                path: root.join("-Users-a-proj"),
                allocated_total: 4096,
                mtime_max: 5,
                complete: true,
            },
            FoldedDir {
                path: root.join("-Users-b-proj"),
                allocated_total: 8192,
                mtime_max: 5,
                complete: true,
            },
            FoldedDir {
                path: root.clone(),
                allocated_total: 20480,
                mtime_max: 5,
                complete: true,
            },
        ];
        let idx = FoldedIndex::from_dirs(dirs);
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let c = BuildContainer::shared_store_of(
            "tool-stores",
            root.clone(),
            BuildStoreKind::AgentScratch,
        );
        let units = Adapter.identify(&c, &BuildCtx::new(1_000_000, &idx, &none, &cache));
        assert_eq!(units.len(), 3);
        for u in &units {
            assert!(
                u.consequence
                    .clone()
                    .unwrap()
                    .contains("breaks that session"),
                "{u:?}"
            );
        }
    }

    #[test]
    fn an_unmeasured_store_says_so_instead_of_reporting_zero() {
        let units = run(
            BuildStoreKind::EspressifTools,
            Path::new("/fixture/tools"),
            false,
        );
        assert!(
            units[0]
                .coverage
                .limits
                .iter()
                .any(|l| l.contains("not measured"))
        );
    }

    #[test]
    fn no_consequence_renders_a_verdict() {
        for kind in [
            BuildStoreKind::EspressifDist,
            BuildStoreKind::EspressifTools,
            BuildStoreKind::EspressifPythonEnv,
            BuildStoreKind::AgentScratch,
        ] {
            let u = &run(kind, Path::new("/fixture/x"), true)[0];
            let text = u.consequence.clone().unwrap().to_ascii_lowercase();
            for banned in [
                "safe to delete",
                "can be deleted",
                // Fragments: the repo's grep gate bans these words whole
                // in source.
                concat!("un", "used"),
                concat!("obso", "lete"),
                concat!("st", "ale"),
            ] {
                assert!(!text.contains(banned), "{kind:?}: {text}");
            }
        }
    }
    /// Adversarial (audit/v080-g0): every user-facing string any adapter
    /// renders for the v0.8.0 store kinds, and every note/name/version
    /// note of the new detectors, carries no verdict word and no em
    /// dash. Tempting wrong patch: scanning only `consequence` and only
    /// this adapter, while `xcode_swift`'s system rows, reasons and
    /// no-action texts go unchecked.
    #[test]
    fn audit_no_verdict_or_em_dash_in_any_new_catalog_string() {
        let banned = [
            concat!("un", "used"),
            concat!("obso", "lete"),
            concat!("st", "ale"),
            concat!("orph", "an"),
            "safe to",
            "can be deleted",
        ];
        let words = |t: &str| -> Vec<String> {
            t.to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .map(str::to_string)
                .collect()
        };
        let check = |who: &str, text: &str| {
            assert!(!text.contains('\u{2014}'), "{who}: em dash in {text}");
            let lower = text.to_ascii_lowercase();
            for b in banned {
                let hit = if b.contains(' ') {
                    lower.contains(b)
                } else {
                    words(text).iter().any(|w| w == b)
                };
                assert!(!hit, "{who}: verdict word {b:?} in {text}");
            }
            assert!(
                !words(text).iter().any(|w| w == concat!("sa", "fe")),
                "{who}: verdict word in {text}"
            );
        };

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("store");
        std::fs::create_dir_all(root.join("child")).unwrap();
        let dirs = vec![
            FoldedDir {
                path: root.join("child"),
                allocated_total: 4096,
                mtime_max: 5,
                complete: true,
            },
            FoldedDir {
                path: root.clone(),
                allocated_total: 8192,
                mtime_max: 5,
                complete: true,
            },
        ];
        let idx = FoldedIndex::from_dirs(dirs);
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let kinds = [
            BuildStoreKind::EspressifDist,
            BuildStoreKind::EspressifTools,
            BuildStoreKind::EspressifPythonEnv,
            BuildStoreKind::AgentScratch,
            BuildStoreKind::XcodeCommandLineTools,
            BuildStoreKind::XcodeDeveloperDiskImages,
            BuildStoreKind::SimulatorSystemSupport,
            BuildStoreKind::AndroidSdkPackages,
        ];
        let registry = crate::build_adapters::registry::Registry::with_builtins();
        let mut seen = 0;
        for adapter in registry.adapters() {
            for kind in kinds {
                if !adapter.store_kinds().contains(&kind) {
                    continue;
                }
                for leaf in ["store", "Images", "Cryptex", "Profiles", "ndk"] {
                    let path = if leaf == "store" {
                        root.clone()
                    } else {
                        tmp.path().join(leaf)
                    };
                    let c = BuildContainer::shared_store_of(adapter.id(), path, kind);
                    for u in adapter.identify(&c, &BuildCtx::new(1_000_000, &idx, &none, &cache)) {
                        seen += 1;
                        // Debug, not serde_json: the repo gate keeps serializer calls out of
                        // non-writer files, and every string field is in the Debug form.
                        let json = format!("{u:?}");
                        check(&format!("{}/{kind:?}", adapter.id()), &json);
                        assert!(
                            !matches!(u.action, crate::artifact::NestedActionCapability::TrashPath),
                            "{}/{kind:?}: a system or tool store offers Trash",
                            adapter.id()
                        );
                    }
                }
            }
        }
        assert!(seen >= 16, "only {seen} units rendered");

        let env = crate::locations::Environment::fixture(
            PathBuf::from("/Users/dev"),
            std::collections::HashMap::new(),
            crate::locations::Platform::MacOS,
        );
        for d in crate::locations::Registry::with_builtins().detectors() {
            if ![
                "espressif",
                "android",
                "xcode-system",
                "core-simulator",
                "claude-code-scratch",
            ]
            .contains(&d.id())
            {
                continue;
            }
            check(d.id(), d.name());
            check(d.id(), d.version_note());
            for l in d.detect(&env) {
                check(d.id(), l.note.as_deref().unwrap_or(""));
            }
        }
    }

    /// Adversarial (audit/v080-g0): `label` ids unique across ALL
    /// kinds, and every new kind is served by exactly one adapter.
    /// Tempting wrong patch: add a variant to the enum and `label` but
    /// forget `ALL`, or claim a kind from two adapters.
    #[test]
    fn audit_store_kind_ids_unique_and_each_new_kind_has_one_adapter() {
        let mut ids: Vec<&str> = BuildStoreKind::ALL.iter().map(|k| k.label()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate BuildStoreKind ids");
        let registry = crate::build_adapters::registry::Registry::with_builtins();
        for kind in [
            BuildStoreKind::EspressifDist,
            BuildStoreKind::EspressifTools,
            BuildStoreKind::EspressifPythonEnv,
            BuildStoreKind::AgentScratch,
            BuildStoreKind::XcodeCommandLineTools,
            BuildStoreKind::XcodeDeveloperDiskImages,
            BuildStoreKind::SimulatorSystemSupport,
        ] {
            assert!(BuildStoreKind::ALL.contains(&kind), "{kind:?} not in ALL");
            let owners: Vec<_> = registry
                .adapters()
                .iter()
                .filter(|a| a.store_kinds().contains(&kind))
                .map(|a| a.id())
                .collect();
            assert_eq!(owners.len(), 1, "{kind:?}: {owners:?}");
        }
    }

    #[test]
    fn no_unit_of_any_store_offers_an_action_and_the_adapter_claims_none() {
        // Tempting wrong patch: flipping `actions_available` later, which
        // would make an active session's scratch directory removable
        // with nothing else objecting.
        assert!(!Adapter.capabilities().actions_available);
        assert!(Adapter.trash_roles().is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("store");
        std::fs::create_dir_all(root.join("child")).unwrap();
        for kind in Adapter.store_kinds() {
            let dirs = vec![
                FoldedDir {
                    path: root.join("child"),
                    allocated_total: 4096,
                    mtime_max: 5,
                    complete: true,
                },
                FoldedDir {
                    path: root.clone(),
                    allocated_total: 8192,
                    mtime_max: 5,
                    complete: true,
                },
            ];
            let idx = FoldedIndex::from_dirs(dirs);
            let none = EventCoverage::untrusted();
            let cache = ContainerCache::disabled();
            let c = BuildContainer::shared_store_of("tool-stores", root.clone(), *kind);
            let units = Adapter.identify(&c, &BuildCtx::new(1_000_000, &idx, &none, &cache));
            assert!(units.len() >= 2, "{kind:?}");
            for u in units {
                assert!(
                    matches!(
                        u.action,
                        crate::artifact::NestedActionCapability::Unsupported { .. }
                    ),
                    "{kind:?}: {:?}",
                    u.action
                );
            }
        }
    }
}
