//! Aider home: `~/.aider`, holding only the `caches/` subdirectory
//! (`~/.aider/caches/model_prices_and_context_window.json`,
//! `~/.aider/caches/versioncheck`), confirmed from primary source during
//! implementation (never a real `~/.aider` on this machine -- PRIVACY IS
//! A HARD RULE): `aider/models.py`
//! (`self.cache_dir = Path.home() / ".aider" / "caches"`,
//! `self.cache_file = self.cache_dir / "model_prices_and_context_window.json"`)
//! and `aider/versioncheck.py`
//! (`VERSION_CHECK_FNAME = Path.home() / ".aider" / "caches" / "versioncheck"`),
//! both from <https://github.com/Aider-AI/aider>, current `main` as of
//! this chunk.
//!
//! Aider's storage is materially different from every other tool in this
//! catalog: most of it is **not** under this home directory at all.
//! `aider/args.py` defines `.aider.chat.history.md` and
//! `.aider.input.history` at the git root (or cwd when there is none),
//! and `aider/repomap.py` defines `.aider.tags.cache.v{3,4}` also at the
//! git root -- all three are project-local, inside the checkout itself.
//! Per #96's explicit acceptance ("attach to the existing worktree
//! artifact model as an agent category, not a tool-home unit"), those
//! three are identified by `crate::agents::aider::identify_repo_units`
//! against each *known project worktree root*
//! `crate::agents::discover_and_measure`'s caller supplies, not against
//! this detector's home path. This detector resolves only the home
//! directory itself, for the `caches/` subdirectory and any home-level
//! config (`.aider.conf.yml` is also *allowed* here per
//! <https://aider.chat/docs/config.html>, "in your home directory or at
//! the root of your git repo").
//!
//! No environment-variable override for the home directory itself is
//! documented upstream; `aider/args.py` reads `.aider.conf.yml` search
//! order but does not relocate `~/.aider` as a whole.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const AIDER_DETECTOR_ID: &str = "aider";

pub struct AiderDetector;

impl Detector for AiderDetector {
    fn id(&self) -> &'static str {
        AIDER_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Aider"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "github.com/Aider-AI/aider aider/models.py + aider/versioncheck.py + aider/args.py + \
         aider/repomap.py, current main as of this chunk; the per-repo history/tags-cache files \
         are identified per project worktree by crate::agents::aider, not decomposed from this \
         home path"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        vec![ProposedLocation {
            detector_id: AIDER_DETECTOR_ID.to_string(),
            path: Some(env.home.join(".aider")),
            category: StorageCategory::LocalState,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some(
                "Aider home: caches/ (model-price/context-window and version-check caches, both \
                 wholly re-downloadable) and, if present, .aider.conf.yml; the per-repo chat \
                 history, input history and tags cache live inside each project checkout instead \
                 -- see crate::agents::aider"
                    .to_string(),
            ),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn convention_path_no_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = AiderDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.aider")));
    }

    #[test]
    fn linux_uses_the_same_convention() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = AiderDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.aider")));
    }
}
