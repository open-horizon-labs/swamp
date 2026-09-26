//! Maven local repository: `~/.m2/repository`, or the
//! `<localRepository>` value from `~/.m2/settings.xml` when present
//! (read as plain text -- a single documented tag, not a general XML
//! parser). https://maven.apache.org/repositories/local.html
//!
//! Unlike Cargo/Gradle/Go, a Maven local repository conventionally
//! mixes artifacts Maven *downloaded* from a remote repository with
//! artifacts a developer `mvn install`ed locally (e.g. from a sibling
//! multi-module build) -- both live in the same directory tree with no
//! reliable directory-level signal. Maven *does* leave file-level
//! evidence per artifact (a `_remote.repositories` marker file and
//! `*.lastUpdated` sentinel next to a downloaded artifact, absent for a
//! locally installed one), but that is a per-artifact classification,
//! not a location to propose -- squarely the build-artifact-identification
//! epic's job (#74), not this registry's. This detector proposes the
//! repository location with an honest "unknown mix" note rather than
//! fabricating a whole-directory download/local split it cannot support.

use super::{
    BuildStoreDecl, BuildStoreKind, ConventionRole, Detector, Environment, LocationStatus,
    ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost, RecoveryHint,
    StorageCategory, StoreAnchor, StoreEntryLookup,
};

pub const MAVEN_DETECTOR_ID: &str = "maven";

/// A single `<localRepository>...</localRepository>` tag, read as plain
/// text. Anything more exotic (a namespaced tag, an entity reference, a
/// multi-line value) is simply not recognized.
fn read_local_repository(settings_xml: &str) -> Option<String> {
    let start = settings_xml.find("<localRepository>")? + "<localRepository>".len();
    let end = settings_xml[start..].find("</localRepository>")?;
    let value = settings_xml[start..start + end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub struct MavenDetector;

impl Detector for MavenDetector {
    fn id(&self) -> &'static str {
        MAVEN_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Maven local repository"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Maven local-repository reference, current stable"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("maven"),
            role: ConventionRole::DependencyStore {
                // The local repository is the only location this
                // detector proposes.
                anchor: StoreAnchor::SoleLocation,
                lookup: StoreEntryLookup::MavenLayout,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "mvn dependency:go-offline",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::MavenRepository,
            anchor: StoreAnchor::SoleLocation,
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let settings_path = env.home.join(".m2/settings.xml");
        let (path, provenance) = match crate::fs_gate::read::bounded_string(
            &settings_path,
            crate::fs_gate::read::BoundedCap::MANIFEST,
        )
        .ok()
        .as_deref()
        .and_then(read_local_repository)
        {
            Some(dir) => (
                std::path::PathBuf::from(dir),
                Provenance::ConfigField("localRepository".to_string()),
            ),
            None => (
                env.home.join(".m2/repository"),
                Provenance::BuiltinConvention,
            ),
        };
        vec![ProposedLocation {
            detector_id: MAVEN_DETECTOR_ID.to_string(),
            path: Some(path),
            category: StorageCategory::Unclassified,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "downloaded and locally-installed artifacts share this tree with no \
                 reliable directory-level signal; per-artifact origin evidence \
                 (_remote.repositories / *.lastUpdated) is a future classification \
                 concern, not this location"
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
    fn convention_when_no_settings_xml() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = MavenDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.m2/repository"))
        );
        assert_eq!(got[0].category, StorageCategory::Unclassified);
    }

    #[test]
    fn local_repository_override_from_settings_xml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".m2")).unwrap();
        std::fs::write(
            tmp.path().join(".m2/settings.xml"),
            "<settings>\n  <localRepository>/data/m2-repo</localRepository>\n</settings>\n",
        )
        .unwrap();
        let env = Environment::fixture(tmp.path().to_path_buf(), HashMap::new(), Platform::MacOS);
        let got = MavenDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/data/m2-repo")));
        assert_eq!(
            got[0].provenance,
            Provenance::ConfigField("localRepository".to_string())
        );
    }

    #[test]
    fn empty_local_repository_tag_falls_back_to_convention() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".m2")).unwrap();
        std::fs::write(
            tmp.path().join(".m2/settings.xml"),
            "<settings><localRepository></localRepository></settings>",
        )
        .unwrap();
        let env = Environment::fixture(tmp.path().to_path_buf(), HashMap::new(), Platform::MacOS);
        let got = MavenDetector.detect(&env);
        assert_eq!(got[0].path, Some(tmp.path().join(".m2/repository")));
    }

    #[test]
    fn leftovers_found_without_mvn_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = MavenDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
