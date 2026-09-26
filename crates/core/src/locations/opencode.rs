//! OpenCode (sst/opencode) storage locations. Unlike Claude Code/Codex/Oh
//! My Pi, OpenCode does not keep everything under one directory: data,
//! config and cache are three independent roots, confirmed by primary
//! source during implementation (never a real `~/.local/share/opencode`
//! on this machine -- PRIVACY IS A HARD RULE):
//! - <https://opencode.ai/docs/troubleshooting/>: data
//!   `~/.local/share/opencode/` (macOS/Linux) holding `auth.json`, `log/`
//!   and per-project session storage; cache `~/.cache/opencode/`
//!   (provider package downloads); config `~/.config/opencode/`
//!   (`opencode.jsonc`/`.json`, local plugin directories).
//! - <https://github.com/anomalyco/opencode/issues/6669> and
//!   <https://github.com/anomalyco/opencode/issues/18633>: config
//!   follows `XDG_CONFIG_HOME`; data/state currently lands under
//!   `XDG_DATA_HOME` even where upstream's own issue tracker argues some
//!   of it belongs under `XDG_STATE_HOME` instead -- this detector
//!   follows the *current* (as of this chunk) behavior, not the
//!   aspirational one, and says so.
//! - There is **no `OPENCODE_DATA_DIR` environment variable**, so this
//!   detector honors none. An earlier revision carried one over from
//!   this epic's own prior research (`crate::agents::matrix`'s
//!   Planned-row note) and honored it "defensively ... flagged as
//!   unconfirmed"; fresh primary-source checking *disproves* it rather
//!   than merely failing to confirm it. sst/opencode `dev` @
//!   `fe3f3a41f79ad292cc3c7c629567385a20ec5130` computes the data
//!   directory in `packages/core/src/global.ts` as
//!   `$XDG_DATA_HOME/opencode` through the `xdg-basedir` package, and
//!   the complete env-var registry in `packages/core/src/flag/flag.ts`
//!   holds only `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG`,
//!   `OPENCODE_CONFIG_CONTENT`, `OPENCODE_DB` and `OPENCODE_TEST_HOME`
//!   (<https://opencode.ai/docs/config> agrees, retrieved 2026-09-21).
//!   The data root is therefore the XDG path unconditionally.
//! - `OPENCODE_CONFIG_DIR` *is* in that registry and *is* the documented
//!   override for the **config** root, so it is honored here -- as a
//!   direct path, not joined with `opencode`, matching `global.ts`'s own
//!   `OPENCODE_CONFIG_DIR ?? <xdg-config>/opencode`. Of the rest,
//!   `OPENCODE_CONFIG`/`OPENCODE_CONFIG_CONTENT` name a config *file*
//!   and inline config text rather than a storage root, and
//!   `OPENCODE_DB`/`OPENCODE_TEST_HOME` are not roots this detector
//!   proposes, so none of those three moves a location here.
//!
//! This detector proposes the **data root first**: `crate::agents`'s
//! shared discovery orchestration (`discover_and_measure`) uses a
//! detector's *first* `Resolved` location as the tool's "home" for
//! interior identification, so the data root -- where sessions,
//! snapshots and the newer SQLite store actually live -- must be first.
//! Config and cache are reported too (so they are visible as ordinary
//! external units with their own byte totals), but are not decomposed
//! into `AgentUnit`s: config is small, protected-by-nature (holds
//! `opencode.jsonc`), and cache is an opaque, wholly re-downloadable
//! provider-package cache -- neither carries session/project linkage,
//! and `crate::agents::opencode`'s interior identification is reserved
//! for the data root where that linkage actually lives.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};
use std::path::PathBuf;

pub const OPENCODE_DETECTOR_ID: &str = "opencode";

pub struct OpenCodeDetector;

fn xdg_or_home(env: &Environment, xdg_var: &str, home_suffix: &[&str]) -> (PathBuf, Provenance) {
    match env.env_var(xdg_var) {
        Some(v) if !v.is_empty() => (
            PathBuf::from(v).join("opencode"),
            Provenance::EnvVar(xdg_var.to_string()),
        ),
        _ => {
            let mut base = env.home.clone();
            for part in home_suffix {
                base = base.join(part);
            }
            (base.join("opencode"), Provenance::BuiltinConvention)
        }
    }
}

impl Detector for OpenCodeDetector {
    fn id(&self) -> &'static str {
        OPENCODE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "OpenCode"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "opencode.ai/docs/troubleshooting and github.com/anomalyco/opencode issues #6669/#18633, \
         current as of this chunk; version-specific storage.ts layout vs. opencode.db is \
         resolved by crate::agents::opencode, not by this detector"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        // No env var overrides the data root: `OPENCODE_DATA_DIR` does
        // not exist upstream (see this module's doc comment for the
        // disproof), so the XDG path is unconditional.
        let (data, data_provenance) = xdg_or_home(env, "XDG_DATA_HOME", &[".local", "share"]);
        // `OPENCODE_CONFIG_DIR` *is* a documented variable, and it names
        // the config directory directly rather than a parent to join
        // `opencode` onto.
        let (config, config_provenance) = match env.env_var("OPENCODE_CONFIG_DIR") {
            Some(v) if !v.is_empty() => (
                PathBuf::from(v),
                Provenance::EnvVar("OPENCODE_CONFIG_DIR".to_string()),
            ),
            _ => xdg_or_home(env, "XDG_CONFIG_HOME", &[".config"]),
        };
        let (cache, cache_provenance) = xdg_or_home(env, "XDG_CACHE_HOME", &[".cache"]);
        vec![
            ProposedLocation {
                detector_id: OPENCODE_DETECTOR_ID.to_string(),
                path: Some(data),
                category: StorageCategory::LocalState,
                provenance: data_provenance,
                status: LocationStatus::Resolved,
                note: Some(
                    "OpenCode data root: auth, logs, session/message/project storage (file tree \
                     or opencode.db depending on version) and git-backed snapshots; see \
                     crate::agents::opencode"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: OPENCODE_DETECTOR_ID.to_string(),
                path: Some(config),
                category: StorageCategory::LocalState,
                provenance: config_provenance,
                status: LocationStatus::Resolved,
                note: Some(
                    "OpenCode config root (opencode.jsonc, plugins); reported as an opaque \
                     external unit, not decomposed into agent units"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: OPENCODE_DETECTOR_ID.to_string(),
                path: Some(cache),
                category: StorageCategory::Cache,
                provenance: cache_provenance,
                status: LocationStatus::Resolved,
                note: Some(
                    "OpenCode provider-package cache; wholly re-downloadable, reported as an \
                     opaque external unit"
                        .to_string(),
                ),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn convention_paths_with_no_overrides() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = OpenCodeDetector.detect(&env);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.local/share/opencode"))
        );
        assert_eq!(
            got[1].path,
            Some(PathBuf::from("/Users/dev/.config/opencode"))
        );
        assert_eq!(
            got[2].path,
            Some(PathBuf::from("/Users/dev/.cache/opencode"))
        );
    }

    /// The inverse of what this test used to assert.
    /// `OPENCODE_DATA_DIR` does not exist upstream: sst/opencode `dev` @
    /// `fe3f3a41f79ad292cc3c7c629567385a20ec5130` computes the data dir
    /// in `packages/core/src/global.ts` as `$XDG_DATA_HOME/opencode` via
    /// `xdg-basedir`, and the complete env-var registry in
    /// `packages/core/src/flag/flag.ts` holds only
    /// `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG`,
    /// `OPENCODE_CONFIG_CONTENT`, `OPENCODE_DB` and
    /// `OPENCODE_TEST_HOME` (opencode.ai/docs/config agrees, retrieved
    /// 2026-09-21). Honoring an invented variable "defensively" is not
    /// free: it silently relocates a real user's data root on any
    /// machine where something else sets that name.
    #[test]
    fn a_nonexistent_data_dir_env_var_is_ignored_in_favor_of_xdg() {
        let mut env_vars = HashMap::new();
        env_vars.insert("OPENCODE_DATA_DIR".to_string(), "/opt/oc-data".to_string());
        env_vars.insert("XDG_DATA_HOME".to_string(), "/opt/xdg-data".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = OpenCodeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/xdg-data/opencode")));
        assert!(
            !got.iter()
                .any(|l| l.provenance == Provenance::EnvVar("OPENCODE_DATA_DIR".to_string())),
            "no location may claim provenance from a variable that does not exist upstream"
        );
    }

    /// `OPENCODE_CONFIG_DIR` *is* in upstream's env-var registry
    /// (`packages/core/src/flag/flag.ts`) and names the config directory
    /// directly, so it is honored -- and only for the config root.
    #[test]
    fn opencode_config_dir_overrides_the_config_root_only() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "OPENCODE_CONFIG_DIR".to_string(),
            "/opt/oc-config".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = OpenCodeDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.local/share/opencode")),
            "the config override must not move the data root"
        );
        assert_eq!(got[1].path, Some(PathBuf::from("/opt/oc-config")));
        assert_eq!(
            got[1].provenance,
            Provenance::EnvVar("OPENCODE_CONFIG_DIR".to_string())
        );
        assert_eq!(
            got[2].path,
            Some(PathBuf::from("/Users/dev/.cache/opencode")),
            "the config override must not move the cache root"
        );
    }

    #[test]
    fn xdg_overrides_are_each_honored_independently() {
        let mut env_vars = HashMap::new();
        env_vars.insert("XDG_DATA_HOME".to_string(), "/opt/xdg-data".to_string());
        env_vars.insert("XDG_CONFIG_HOME".to_string(), "/opt/xdg-config".to_string());
        env_vars.insert("XDG_CACHE_HOME".to_string(), "/opt/xdg-cache".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = OpenCodeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/xdg-data/opencode")));
        assert_eq!(got[1].path, Some(PathBuf::from("/opt/xdg-config/opencode")));
        assert_eq!(got[2].path, Some(PathBuf::from("/opt/xdg-cache/opencode")));
    }

    #[test]
    fn data_root_is_first_so_agent_identification_uses_it() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = OpenCodeDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/home/dev/.local/share/opencode"))
        );
        assert_eq!(got[0].category, StorageCategory::LocalState);
    }
}
