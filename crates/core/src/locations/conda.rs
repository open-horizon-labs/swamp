//! Conda: conventional base-environment prefixes (`~/miniconda3`,
//! `~/anaconda3`, `~/miniforge3`), the default shared-environment root
//! (`~/.conda/envs`), and `~/.condarc`'s `envs_dirs`/`pkgs_dirs` lists,
//! parsed read-only, plus the `CONDA_PKGS_DIRS` env var override.
//! https://docs.conda.io/projects/conda/en/stable/user-guide/configuration/custom-env-and-pkg-locations.html
//!
//! `.condarc` is YAML; rather than add a YAML dependency for two list
//! keys, this module reads it as plain lines and recognizes exactly the
//! documented `key:` followed by `  - value` list-item shape. Any other
//! `.condarc` shape (a flow-style list, a comment on the same line, ...)
//! is simply not recognized -- never executed, never guessed at with a
//! best-effort regex that could misparse into the wrong path.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};
use std::path::PathBuf;

pub const CONDA_DETECTOR_ID: &str = "conda";

pub struct CondaDetector;

/// Every value in the YAML list following a `key:` line (each an
/// indented `- value` line), stopping at the first line that is not
/// blank and not an indented list item.
fn read_condarc_list(condarc: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in condarc.lines() {
        let trimmed = line.trim_start();
        if !in_list {
            if trimmed.starts_with(key) && trimmed[key.len()..].trim_start().starts_with(':') {
                in_list = true;
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix('-') {
            out.push(item.trim().trim_matches('"').trim_matches('\'').to_string());
        } else {
            break; // a non-list-item line ends this key's list
        }
    }
    out
}

impl Detector for CondaDetector {
    fn id(&self) -> &'static str {
        CONDA_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Conda"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "conda custom-env-and-pkg-locations guide, current stable"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let mut out = Vec::new();

        for prefix in ["miniconda3", "anaconda3", "miniforge3"] {
            out.push(ProposedLocation {
                detector_id: CONDA_DETECTOR_ID.to_string(),
                path: Some(env.home.join(prefix)),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(format!("conventional base-environment prefix (~/{prefix})")),
            });
        }

        out.push(ProposedLocation {
            detector_id: CONDA_DETECTOR_ID.to_string(),
            path: Some(env.home.join(".conda/envs")),
            category: StorageCategory::Environments,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some("default shared environment root".to_string()),
        });

        let condarc_path = env.home.join(".condarc");
        if let Ok(text) = crate::fs_gate::read::bounded_string(
            &condarc_path,
            crate::fs_gate::read::BoundedCap::MANIFEST,
        ) {
            for dir in read_condarc_list(&text, "envs_dirs") {
                out.push(ProposedLocation {
                    detector_id: CONDA_DETECTOR_ID.to_string(),
                    path: Some(PathBuf::from(dir)),
                    category: StorageCategory::Environments,
                    provenance: Provenance::ConfigField("envs_dirs".to_string()),
                    status: LocationStatus::Resolved,
                    note: Some("environment root from ~/.condarc envs_dirs".to_string()),
                });
            }
            for dir in read_condarc_list(&text, "pkgs_dirs") {
                out.push(ProposedLocation {
                    detector_id: CONDA_DETECTOR_ID.to_string(),
                    path: Some(PathBuf::from(dir)),
                    category: StorageCategory::Downloads,
                    provenance: Provenance::ConfigField("pkgs_dirs".to_string()),
                    status: LocationStatus::Resolved,
                    note: Some("package cache root from ~/.condarc pkgs_dirs".to_string()),
                });
            }
        } else {
            out.push(ProposedLocation {
                detector_id: CONDA_DETECTOR_ID.to_string(),
                path: None,
                category: StorageCategory::Downloads,
                provenance: Provenance::ConfigField("pkgs_dirs".to_string()),
                status: LocationStatus::NotPresent,
                note: Some(
                    "no ~/.condarc; default package cache is under the base prefix".to_string(),
                ),
            });
        }

        // CONDA_PKGS_DIRS always wins over ~/.condarc's pkgs_dirs when
        // set (matches conda's own documented precedence), and is
        // proposed independently of whether .condarc parsed.
        if let Some(v) = env.env_var("CONDA_PKGS_DIRS").filter(|v| !v.is_empty()) {
            for dir in v.split(',').filter(|s| !s.is_empty()) {
                out.push(ProposedLocation {
                    detector_id: CONDA_DETECTOR_ID.to_string(),
                    path: Some(PathBuf::from(dir)),
                    category: StorageCategory::Downloads,
                    provenance: Provenance::EnvVar("CONDA_PKGS_DIRS".to_string()),
                    status: LocationStatus::Resolved,
                    note: Some("package cache root from CONDA_PKGS_DIRS".to_string()),
                });
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn all_three_conventional_prefixes_are_always_proposed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CondaDetector.detect(&env);
        for name in ["miniconda3", "anaconda3", "miniforge3"] {
            assert!(
                got.iter()
                    .any(|l| l.path == Some(PathBuf::from("/Users/dev").join(name))),
                "missing {name}"
            );
        }
    }

    #[test]
    fn condarc_envs_dirs_and_pkgs_dirs_are_parsed_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(
            home.join(".condarc"),
            "channels:\n  - conda-forge\nenvs_dirs:\n  - /data/conda-envs\n  - /Volumes/x/envs\npkgs_dirs:\n  - /data/conda-pkgs\n",
        )
        .unwrap();
        let env = Environment::fixture(home.to_path_buf(), HashMap::new(), Platform::MacOS);
        let got = CondaDetector.detect(&env);
        let envs_dirs: Vec<_> = got
            .iter()
            .filter(|l| matches!(&l.provenance, Provenance::ConfigField(f) if f == "envs_dirs"))
            .filter_map(|l| l.path.clone())
            .collect();
        assert_eq!(
            envs_dirs,
            vec![
                PathBuf::from("/data/conda-envs"),
                PathBuf::from("/Volumes/x/envs")
            ]
        );
        let pkgs_dirs: Vec<_> = got
            .iter()
            .filter(|l| matches!(&l.provenance, Provenance::ConfigField(f) if f == "pkgs_dirs"))
            .filter_map(|l| l.path.clone())
            .collect();
        assert_eq!(pkgs_dirs, vec![PathBuf::from("/data/conda-pkgs")]);
    }

    #[test]
    fn missing_condarc_is_not_present_never_a_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let env = Environment::fixture(tmp.path().to_path_buf(), HashMap::new(), Platform::MacOS);
        let got = CondaDetector.detect(&env);
        assert!(got.iter().any(|l| l.status == LocationStatus::NotPresent));
    }

    #[test]
    fn conda_pkgs_dirs_env_var_overrides_and_supports_multiple_entries() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "CONDA_PKGS_DIRS".to_string(),
            "/fast/pkgs,/slow/pkgs".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = CondaDetector.detect(&env);
        let from_env: Vec<_> = got
            .iter()
            .filter(|l| matches!(&l.provenance, Provenance::EnvVar(v) if v == "CONDA_PKGS_DIRS"))
            .filter_map(|l| l.path.clone())
            .collect();
        assert_eq!(
            from_env,
            vec![PathBuf::from("/fast/pkgs"), PathBuf::from("/slow/pkgs")]
        );
    }

    #[test]
    fn leftovers_found_without_conda_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CondaDetector.detect(&env);
        assert!(
            got.iter().any(|l| l.status == LocationStatus::Resolved
                && l.category == StorageCategory::Installation)
        );
    }
}
