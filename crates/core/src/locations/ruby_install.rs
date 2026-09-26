//! ruby-install: installs Ruby interpreters under `~/.rubies` (its
//! per-user default) or `/opt/rubies` (a common system-wide convention),
//! both always proposed the same way Homebrew's two prefixes are.
//! https://github.com/postmodern/ruby-install
//!
//! chruby (https://github.com/postmodern/ruby-install /
//! https://github.com/postmodern/chruby) is a switcher, not an
//! installer: it has no storage of its own beyond reading `~/.rubies`
//! and `/opt/rubies` (the same directories ruby-install populates) and
//! a tiny `~/.chruby` config file. This detector's `~/.rubies`/
//! `/opt/rubies` candidates already cover the storage chruby switches
//! between; a config-only tool never needs its own storage detector.

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const RUBY_INSTALL_DETECTOR_ID: &str = "ruby-install";

pub struct RubyInstallDetector;

impl Detector for RubyInstallDetector {
    fn id(&self) -> &'static str {
        RUBY_INSTALL_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "ruby-install"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "ruby-install/chruby READMEs, current stable convention paths"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("ruby"),
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".ruby-version"],
                layout: InstalledVersionLayout::VersionPerEntry,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "ruby-install ruby <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        vec![
            ProposedLocation {
                detector_id: RUBY_INSTALL_DETECTOR_ID.to_string(),
                path: Some(env.home.join(".rubies")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("per-user installed Ruby interpreters (chruby-visible)".to_string()),
            },
            ProposedLocation {
                detector_id: RUBY_INSTALL_DETECTOR_ID.to_string(),
                path: Some(std::path::PathBuf::from("/opt/rubies")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("system-wide installed Ruby interpreters (chruby-visible)".to_string()),
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
    fn both_conventional_prefixes_are_always_proposed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RubyInstallDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.rubies")))
        );
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/opt/rubies")))
        );
    }

    #[test]
    fn leftovers_found_without_ruby_install_or_chruby_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RubyInstallDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
