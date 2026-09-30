//! Android SDK: `ANDROID_HOME`, else `ANDROID_SDK_ROOT` (deprecated but
//! still honored), else `~/Library/Android/sdk` -- `platforms/`,
//! `system-images/`, `build-tools/`, `emulator/`, `ndk/`,
//! `cmdline-tools/`, `platform-tools/`, `cmake/` (an NDK that
//! `ANDROID_NDK_HOME`/`ANDROID_NDK_ROOT` names outside the SDK root is
//! proposed too). `licenses/` is left out. AVDs (mutable emulator
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

/// The SDK root's package folders this detector measures, each an
/// `installation` (`sdkmanager` puts them back). `licenses/` (tiny, and
/// the record of accepted licences) and the loose files beside these
/// folders (`ndk-install.log`, `.knownPackages`, `.temp`) are not
/// measured, so a `du` of the SDK root exceeds the sum of its units by
/// exactly those.
const SDK_FOLDERS: &[(&str, &str)] = &[
    ("platforms", "installed Android platform SDKs"),
    ("system-images", "installed emulator system images"),
    ("build-tools", "installed build-tools versions"),
    ("emulator", "installed emulator binaries"),
    ("ndk", "installed NDK versions, one folder per version"),
    (
        "cmdline-tools",
        "installed command-line tools (sdkmanager, avdmanager)",
    ),
    ("platform-tools", "installed platform-tools (adb, fastboot)"),
    ("cmake", "installed CMake versions"),
];

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

/// `..` and `.` resolved without touching the filesystem, so that
/// "inside the SDK root" is decided on where a path points, not on how
/// it is spelled (`<sdk>/../elsewhere/ndk` starts with the SDK's
/// components and is not inside it).
fn lexically_normalized(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
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
        let mut out: Vec<ProposedLocation> = SDK_FOLDERS
            .iter()
            .map(|(folder, note)| ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(sdk.join(folder)),
                category: StorageCategory::Installation,
                provenance: sdk_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some((*note).to_string()),
            })
            .collect();

        // An NDK the environment names, when it lives outside the SDK
        // root. One inside the root is already `ndk/`'s child, and
        // proposing it again would count its bytes twice.
        for var in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"] {
            let Some(v) = env.env_var(var).filter(|v| !v.is_empty()) else {
                continue;
            };
            let ndk = lexically_normalized(&PathBuf::from(v));
            let already_proposed = ndk.starts_with(lexically_normalized(&sdk))
                || out.iter().any(|l| l.path.as_deref() == Some(ndk.as_path()));
            if already_proposed {
                continue;
            }
            out.push(ProposedLocation {
                detector_id: ANDROID_DETECTOR_ID.to_string(),
                path: Some(ndk),
                category: StorageCategory::Installation,
                provenance: Provenance::EnvVar(var.to_string()),
                status: LocationStatus::Resolved,
                note: Some("an installed NDK outside the SDK root".to_string()),
            });
        }

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
    use std::path::Path;

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

    fn paths(got: &[ProposedLocation]) -> Vec<PathBuf> {
        got.iter().filter_map(|l| l.path.clone()).collect()
    }

    #[test]
    fn every_sdk_package_folder_is_proposed_and_licenses_is_not() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = paths(&AndroidDetector.detect(&env));
        for folder in [
            "platforms",
            "system-images",
            "build-tools",
            "emulator",
            "ndk",
            "cmdline-tools",
            "platform-tools",
            "cmake",
        ] {
            assert!(
                got.contains(&PathBuf::from(format!(
                    "/Users/dev/Library/Android/sdk/{folder}"
                ))),
                "{folder} is not proposed"
            );
        }
        assert!(
            !got.iter().any(|p| p.ends_with("licenses")),
            "licenses/ is the record of accepted licences and stays out"
        );
    }

    #[test]
    fn the_new_folders_follow_the_sdk_root_override() {
        let mut env_vars = HashMap::new();
        env_vars.insert("ANDROID_HOME".to_string(), "/opt/android-home".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        for folder in ["ndk", "cmdline-tools", "platform-tools", "cmake"] {
            let loc = got
                .iter()
                .find(|l| l.path == Some(PathBuf::from("/opt/android-home").join(folder)))
                .unwrap_or_else(|| panic!("{folder} did not follow ANDROID_HOME"));
            assert_eq!(loc.category, StorageCategory::Installation);
            assert_eq!(
                loc.provenance,
                Provenance::EnvVar("ANDROID_HOME".to_string())
            );
        }
    }

    #[test]
    fn an_ndk_outside_the_sdk_is_proposed_with_its_own_provenance() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ANDROID_NDK_HOME".to_string(),
            "/opt/ndk/26.1.10909125".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AndroidDetector.detect(&env);
        let ndk = got
            .iter()
            .find(|l| l.path == Some(PathBuf::from("/opt/ndk/26.1.10909125")))
            .expect("an NDK outside the SDK root is measured");
        assert_eq!(
            ndk.provenance,
            Provenance::EnvVar("ANDROID_NDK_HOME".to_string())
        );
    }

    #[test]
    fn an_ndk_inside_the_sdk_is_not_counted_a_second_time() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ANDROID_NDK_ROOT".to_string(),
            "/Users/dev/Library/Android/sdk/ndk/26.1.10909125".to_string(),
        );
        env_vars.insert(
            "ANDROID_NDK_HOME".to_string(),
            "/Users/dev/Library/Android/sdk/ndk".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = paths(&AndroidDetector.detect(&env));
        assert!(
            !got.iter().any(|p| {
                p.starts_with("/Users/dev/Library/Android/sdk/ndk")
                    && p.as_path() != Path::new("/Users/dev/Library/Android/sdk/ndk")
            }),
            "a child of ndk/ would count its bytes under ndk/ and again on its own: {got:?}"
        );
        assert_eq!(
            got.iter()
                .filter(|p| p.as_path() == Path::new("/Users/dev/Library/Android/sdk/ndk"))
                .count(),
            1
        );
    }

    #[test]
    fn a_missing_sdk_root_is_still_proposed_so_scope_can_say_missing() {
        // Presence is scope's finding, not the detector's: every folder
        // is proposed whether or not the SDK (or an NDK) exists.
        let env = Environment::fixture(
            PathBuf::from("/nonexistent"),
            HashMap::new(),
            Platform::MacOS,
        );
        let got = AndroidDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path.as_ref().is_some_and(|p| p.ends_with("ndk")))
        );
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }

    #[test]
    fn a_dotdot_that_stays_inside_the_sdk_is_still_not_counted_twice() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ANDROID_NDK_HOME".to_string(),
            "/Users/dev/Library/Android/sdk/cmake/../ndk/26.1".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = paths(&AndroidDetector.detect(&env));
        assert!(got.iter().all(|p| !p.to_string_lossy().contains("26.1")));
    }

    #[test]
    fn a_dotdot_that_leaves_the_sdk_is_measured_at_its_normalized_path() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ANDROID_NDK_HOME".to_string(),
            "/Users/dev/Library/Android/sdk/../ndk-outside/26.1".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = paths(&AndroidDetector.detect(&env));
        assert!(got.contains(&PathBuf::from(
            "/Users/dev/Library/Android/ndk-outside/26.1"
        )));
    }
}
