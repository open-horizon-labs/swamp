//! The Codex **desktop app** (`Codex.app`, bundle id `com.openai.codex` --
//! <https://github.com/openai/codex/blob/main/codex-rs/cli/src/doctor/desktop/platform.rs>,
//! `inspect_macos_bundle`/`identity: "com.openai.codex"`): a materially
//! different client from the Codex CLI, with its own storage, modeled by
//! its own detector rather than by extrapolating the CLI's `CODEX_HOME`
//! schema onto it (#93's explicit acceptance).
//!
//! Only the desktop app's **log directory** is confirmed by primary
//! source this chunk:
//! <https://github.com/openai/codex/blob/main/codex-rs/cli/src/doctor/desktop.rs>,
//! `desktop_log_root`: macOS `~/Library/Logs/<identity>`, i.e.
//! `~/Library/Logs/com.openai.codex`, further split into `%Y/%m/%d`
//! session-date directories the same doctor module reads to find the
//! running app's own handshake log.
//!
//! Everything else the desktop app might keep -- settings, session
//! cache, any local database -- is **not** confirmed by primary source
//! this chunk and is deliberately not modeled: this detector and
//! `crate::agents::codex_desktop` cover logs only, and both say so
//! explicitly rather than silently treating an unconfirmed directory as
//! empty. No Linux desktop build is confirmed either, so this detector
//! reports `NotPresent` there instead of guessing a path.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const CODEX_DESKTOP_DETECTOR_ID: &str = "codex-desktop";

/// The bundle identifier `desktop_log_root` keys its `~/Library/Logs/`
/// subdirectory on.
const MACOS_LOG_IDENTITY: &str = "com.openai.codex";

pub struct CodexDesktopDetector;

impl Detector for CodexDesktopDetector {
    fn id(&self) -> &'static str {
        CODEX_DESKTOP_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Codex desktop app"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "log directory only, from codex-rs/cli/src/doctor/desktop.rs (current main as of this \
         chunk); the desktop app's settings/session storage is not confirmed by primary source \
         and is not modeled -- see crate::agents::codex_desktop"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        if env.platform != Platform::MacOS {
            return vec![ProposedLocation {
                detector_id: CODEX_DESKTOP_DETECTOR_ID.to_string(),
                path: None,
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::NotPresent,
                note: Some(
                    "no confirmed Codex desktop app log location on this platform".to_string(),
                ),
            }];
        }
        let base = env.home.join("Library/Logs").join(MACOS_LOG_IDENTITY);
        vec![ProposedLocation {
            detector_id: CODEX_DESKTOP_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some(
                "Codex desktop app log directory only; settings/session storage is a \
                 documented unknown, not modeled here (see crate::agents::codex_desktop)"
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
    fn macos_resolves_the_log_directory() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CodexDesktopDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/Library/Logs/com.openai.codex"))
        );
        assert_eq!(got[0].status, LocationStatus::Resolved);
    }

    #[test]
    fn linux_is_not_present_not_guessed() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = CodexDesktopDetector.detect(&env);
        assert_eq!(got[0].status, LocationStatus::NotPresent);
        assert_eq!(got[0].path, None);
    }
}
