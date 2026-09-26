//! Ollama: `~/.ollama` (config, logs, keys), and its model store --
//! `OLLAMA_MODELS`, else `~/.ollama/models` -- holding `blobs/` (content-
//! addressed layer files, shared across every model manifest that
//! references them) and `manifests/` (small per-model/tag manifest
//! files pointing at blob digests). https://docs.ollama.com/faq
//!
//! Blobs are referenced by filename (their digest), not by symlink, so
//! one whole-directory measurement of the models root already counts
//! each blob file once -- there is no snapshot-symlink layer to worry
//! about the way Hugging Face's cache has.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const OLLAMA_DETECTOR_ID: &str = "ollama";

pub struct OllamaDetector;

impl Detector for OllamaDetector {
    fn id(&self) -> &'static str {
        OLLAMA_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Ollama"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Ollama FAQ, current stable ~/.ollama layout"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let base = env.home.join(".ollama");
        let (models, models_prov) = match env.env_var("OLLAMA_MODELS").filter(|v| !v.is_empty()) {
            Some(v) => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("OLLAMA_MODELS".to_string()),
            ),
            None => (base.join("models"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: OLLAMA_DETECTOR_ID.to_string(),
                path: Some(base),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("config, logs, and host keys".to_string()),
            },
            ProposedLocation {
                detector_id: OLLAMA_DETECTOR_ID.to_string(),
                path: Some(models),
                category: StorageCategory::Models,
                provenance: models_prov,
                status: LocationStatus::Resolved,
                note: Some(
                    "blobs/ (content-addressed, shared layers) + manifests/ \
                     (per-model/tag pointers)"
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
    use std::path::PathBuf;

    #[test]
    fn convention_when_no_env_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = OllamaDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.ollama")));
        assert_eq!(
            got[1].path,
            Some(PathBuf::from("/Users/dev/.ollama/models"))
        );
        assert_eq!(got[1].category, StorageCategory::Models);
    }

    #[test]
    fn ollama_models_env_var_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "OLLAMA_MODELS".to_string(),
            "/data/ollama-models".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = OllamaDetector.detect(&env);
        assert_eq!(got[1].path, Some(PathBuf::from("/data/ollama-models")));
        // ~/.ollama itself (config/logs) is untouched by the override.
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.ollama")));
    }

    #[test]
    fn leftovers_found_without_ollama_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = OllamaDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
