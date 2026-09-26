//! Continue (continuedev/continue) home: `~/.continue`. Module named
//! `continue_dev` rather than `continue` -- the latter is a Rust
//! keyword.
//!
//! Sourced from <https://docs.continue.dev/customize/deep-dives/configuration>
//! and <https://docs.continue.dev/reference> (fetched during
//! implementation, never from a real `~/.continue` on this machine --
//! PRIVACY IS A HARD RULE), current as of this chunk: "Local user-level
//! configuration is stored and can be edited in your home directory in
//! `config.yaml`" at `~/.continue/config.yaml` (macOS/Linux), with a
//! legacy `config.json` accepted at the same location and a deprecated
//! `config.ts` for programmatic configuration.
//!
//! `sessions/`, `index/` and `dev_data/` are named in this catalog's own
//! prior research (`crate::agents::matrix`'s pre-existing `Planned` row)
//! but this chunk's own documentation fetch did not independently
//! re-confirm their exact shape (the configuration reference page covers
//! `config.yaml` only) -- honored defensively by
//! `crate::agents::continue_dev`, same "check for real markers, report
//! an explicit unknown-layout residual otherwise" discipline every other
//! adapter in this catalog uses, rather than presented as independently
//! verified this chunk. No environment-variable override for the home
//! directory was found in either page.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const CONTINUE_DETECTOR_ID: &str = "continue";

pub struct ContinueDetector;

impl Detector for ContinueDetector {
    fn id(&self) -> &'static str {
        CONTINUE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Continue"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "docs.continue.dev/customize/deep-dives/configuration and docs.continue.dev/reference, \
         current as of this chunk; config.yaml location is confirmed, sessions/index/dev_data \
         are this catalog's own prior research, not independently re-confirmed this chunk -- see \
         crate::agents::continue_dev"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        vec![ProposedLocation {
            detector_id: CONTINUE_DETECTOR_ID.to_string(),
            path: Some(env.home.join(".continue")),
            category: StorageCategory::LocalState,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some(
                "Continue home: config.yaml/config.json (protected), sessions/ (per-session \
                 conversation state plus an index file), index/ (embeddings/tag caches), \
                 dev_data/ (anonymized usage events); see crate::agents::continue_dev"
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
    fn convention_path() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = ContinueDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.continue")));
    }

    #[test]
    fn linux_uses_the_same_convention() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = ContinueDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.continue")));
    }
}
