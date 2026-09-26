//! Hugging Face Hub cache: `HF_HOME` (default `~/.cache/huggingface`),
//! `HF_HUB_CACHE` (default `$HF_HOME/hub`), and `HF_DATASETS_CACHE`
//! (default `$HF_HOME/datasets`, an older, still-honored override --
//! current library versions store datasets under the hub cache's own
//! `datasets--*` repo folders instead).
//! https://huggingface.co/docs/huggingface_hub/guides/manage-cache
//!
//! Model/dataset/space revisions share files via a `blobs/` directory
//! plus per-revision `snapshots/<rev>/` directories of *symlinks* into
//! it -- this detector proposes `HF_HUB_CACHE` as a single location
//! rather than iterating repos/blobs itself; `walk::resize_artifact`
//! already never follows symlinks (`process_walk`/`process_size` both
//! skip `is_symlink()` entries), so a blob shared across many snapshot
//! symlinks is still measured exactly once, with no detector-side
//! bookkeeping required.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};
use std::path::PathBuf;

pub const HUGGINGFACE_DETECTOR_ID: &str = "huggingface";

fn resolve(env: &Environment, var: &str, default: PathBuf) -> (PathBuf, Provenance) {
    match env.env_var(var).filter(|v| !v.is_empty()) {
        Some(v) => (PathBuf::from(v), Provenance::EnvVar(var.to_string())),
        None => (default, Provenance::BuiltinConvention),
    }
}

pub struct HuggingFaceDetector;

impl Detector for HuggingFaceDetector {
    fn id(&self) -> &'static str {
        HUGGINGFACE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Hugging Face cache"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "huggingface_hub cache management guide, current stable"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (hf_home, home_prov) = resolve(env, "HF_HOME", env.home.join(".cache/huggingface"));
        let (hub_cache, hub_prov) = resolve(env, "HF_HUB_CACHE", hf_home.join("hub"));
        let (datasets_cache, datasets_prov) =
            resolve(env, "HF_DATASETS_CACHE", hf_home.join("datasets"));

        vec![
            ProposedLocation {
                detector_id: HUGGINGFACE_DETECTOR_ID.to_string(),
                path: Some(hf_home),
                category: StorageCategory::LocalState,
                provenance: home_prov,
                status: LocationStatus::Resolved,
                note: Some("assets cache, xet chunk/shard cache, and misc. state".to_string()),
            },
            ProposedLocation {
                detector_id: HUGGINGFACE_DETECTOR_ID.to_string(),
                path: Some(hub_cache),
                category: StorageCategory::Models,
                provenance: hub_prov,
                status: LocationStatus::Resolved,
                note: Some(
                    "model/dataset/space cache: shared blobs, per-revision snapshot \
                     symlinks (never followed, so blobs are counted once)"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: HUGGINGFACE_DETECTOR_ID.to_string(),
                path: Some(datasets_cache),
                category: StorageCategory::Models,
                provenance: datasets_prov,
                status: LocationStatus::Resolved,
                note: Some(
                    "legacy separate datasets cache (current library versions use the \
                     hub cache's own datasets--* folders instead)"
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
    fn convention_when_no_env_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = HuggingFaceDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.cache/huggingface"))
        );
        assert_eq!(
            got[1].path,
            Some(PathBuf::from("/Users/dev/.cache/huggingface/hub"))
        );
        assert_eq!(got[1].category, StorageCategory::Models);
    }

    #[test]
    fn hf_home_override_relocates_hub_and_datasets_defaults() {
        let mut env_vars = HashMap::new();
        env_vars.insert("HF_HOME".to_string(), "/data/hf".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = HuggingFaceDetector.detect(&env);
        assert_eq!(got[1].path, Some(PathBuf::from("/data/hf/hub")));
        assert_eq!(got[2].path, Some(PathBuf::from("/data/hf/datasets")));
    }

    #[test]
    fn hf_hub_cache_overrides_independently_of_hf_home() {
        let mut env_vars = HashMap::new();
        env_vars.insert("HF_HOME".to_string(), "/data/hf".to_string());
        env_vars.insert("HF_HUB_CACHE".to_string(), "/fast-disk/hub".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = HuggingFaceDetector.detect(&env);
        assert_eq!(got[1].path, Some(PathBuf::from("/fast-disk/hub")));
        // HF_HOME still relocates the datasets cache and base dir.
        assert_eq!(got[0].path, Some(PathBuf::from("/data/hf")));
        assert_eq!(got[2].path, Some(PathBuf::from("/data/hf/datasets")));
    }

    #[test]
    fn leftovers_found_without_huggingface_hub_installed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = HuggingFaceDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
