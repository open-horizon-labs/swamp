//! Android SDK: `ANDROID_HOME`, else `ANDROID_SDK_ROOT` (deprecated but
//! still honored), else `~/Library/Android/sdk` -- `platforms/`,
//! `system-images/`, `build-tools/`, `emulator/`. AVDs (mutable emulator
//! instance data, analogous to CoreSimulator's `Devices/`):
//! `ANDROID_AVD_HOME`, else `~/.android/avd`.
//! https://developer.android.com/tools/variables
//!
//! Gradle's own caches (including Android Gradle Plugin downloads) are
//! `crate::locations::gradle`'s job, not this detector's.

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};
use std::path::PathBuf;

pub const ANDROID_DETECTOR_ID: &str = "android";

fn sdk_root(env: &Environment) -> (PathBuf, Provenance) {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(v) = env.env_var(var).filter(|v| !v.is_empty()) {
            return (PathBuf::from(v), Provenance::EnvVar(var.to_string()));
        }
    }
    (
        env.home.join("Library/Android/sdk"),
        Provenance::BuiltinConvention,
    )
}

pub struct AndroidDetector;

impl Detector for AndroidDetector {
    fn id(&self) -> &'static str {
        ANDROID_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Android SDK"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Android tools environment variables reference, current stable"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::AndroidSdkPackages,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Installation,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::AndroidVirtualDevices,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Environments,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (sdk, sdk_prov) = sdk_root(env);
        let mut out = vec![
            ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(sdk.join("platforms")),
                category: StorageCategory::Installation,
                provenance: sdk_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed Android platform SDKs".to_string()),
            },
            ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(sdk.join("system-images")),
                category: StorageCategory::Installation,
                provenance: sdk_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed emulator system images".to_string()),
            },
            ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(sdk.join("build-tools")),
                category: StorageCategory::Installation,
                provenance: sdk_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed build-tools versions".to_string()),
            },
            ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(sdk.join("emulator")),
                category: StorageCategory::Installation,
                provenance: sdk_prov,
                status: LocationStatus::Resolved,
                note: Some("installed emulator binaries".to_string()),
            },
        ];

        let (avd, avd_prov) = match env.env_var("ANDROID_AVD_HOME").filter(|v| !v.is_empty()) {
            Some(v) => (
                PathBuf::from(v),
                Provenance::EnvVar("ANDROID_AVD_HOME".to_string()),
            ),
            None => (env.home.join(".android/avd"), Provenance::BuiltinConvention),
        };
        out.push(ProposedLocation {
            detector_id: ANDROID_DETECTOR_ID.to_string(),
            path: Some(avd),
            category: StorageCategory::Environments,
            provenance: avd_prov,
            status: LocationStatus::Resolved,
            note: Some("mutable AVD (emulator instance) data".to_string()),
        });

        out
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
        let got = AndroidDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/Library/Android/sdk/platforms")))
        );
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.android/avd")))
        );
    }

    #[test]
    fn android_home_wins_over_android_sdk_root_and_convention() {
        let mut env_vars = HashMap::new();
        env_vars.insert("ANDROID_HOME".to_string(), "/opt/android-home".to_string());
        env_vars.insert(
            "ANDROID_SDK_ROOT".to_string(),
            "/opt/android-sdk-root".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/opt/android-home/platforms")))
        );
        assert!(!got.iter().any(|l| {
            l.path
                .as_ref()
                .is_some_and(|p| p.starts_with("/opt/android-sdk-root"))
        }));
    }

    #[test]
    fn deprecated_android_sdk_root_still_honored_when_android_home_absent() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ANDROID_SDK_ROOT".to_string(),
            "/opt/android-sdk-root".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/opt/android-sdk-root/platforms")))
        );
    }

    #[test]
    fn android_avd_home_overrides_avd_convention_independently_of_sdk_root() {
        let mut env_vars = HashMap::new();
        env_vars.insert("ANDROID_AVD_HOME".to_string(), "/data/avd".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/data/avd")))
        );
        // SDK root is untouched by ANDROID_AVD_HOME.
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/Library/Android/sdk/platforms")))
        );
    }

    #[test]
    fn leftovers_found_without_sdkmanager_installed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
