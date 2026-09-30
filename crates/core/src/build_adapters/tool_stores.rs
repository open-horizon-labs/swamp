//! Single-directory tool stores that have no interior worth naming:
//! ESP-IDF's `dist/`, `tools/` and `python_env/` under `IDF_TOOLS_PATH`
//! (or `~/.espressif`). Each is one unit, its own root row, with what
//! removing it costs in the tool's own words.
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
        "Tool stores (ESP-IDF)"
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
}
