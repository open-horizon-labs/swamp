//! Cursor editor-profile storage: `~/Library/Application Support/Cursor`
//! (macOS), plus the separate `~/.cursor` CLI/chat root.
//!
//! No single official Cursor documentation page describing this layout
//! was found this chunk (checked during implementation, never learned
//! from a real Cursor installation on this machine -- PRIVACY IS A HARD
//! RULE); sources are community-reverse-engineered, lower confidence
//! than the primary-source-backed rows in this catalog, and should be
//! re-verified against an installed Cursor version before trusting this
//! adapter's byte totals in a review:
//! - <https://github.com/Callum-Ward/cursaves/blob/main/docs/how-cursor-stores-chats.md>:
//!   two SQLite databases, both `ItemTable`/`cursorDiskKV` key-value
//!   stores in WAL mode (so each carries `-wal`/`-shm` sidecars). The
//!   **global** database at `User/globalStorage/state.vscdb` holds
//!   actual conversation content for every project
//!   (`composerData:{uuid}`, `bubbleId:...`, `checkpointId:...`,
//!   content-addressed `composer.content.{hash}` blobs). The
//!   **per-workspace** database at
//!   `User/workspaceStorage/{workspace-id}/state.vscdb` is an index for
//!   that workspace's own conversation list, not the content itself, and
//!   sits beside a `workspace.json` naming that workspace's folder as a
//!   `file://` URI -- real, tool-written linkage evidence, not a
//!   basename guess.
//! - <https://github.com/thomas-pedersen/cursor-chat-browser>: same
//!   two-database shape, independently corroborating the above.
//! - The issue text's own home-path note additionally lists `History/`
//!   (VS Code's built-in local-file-history/timeline feature, unrelated
//!   to AI chat) and `CachedExtensionVSIXs/`, `Cache/`, `CachedData/`,
//!   `logs/` as Electron/VS-Code-fork conventional siblings of `User/`
//!   under the same Application Support root -- not Cursor-specific and
//!   not independently re-verified this chunk, but a standard enough
//!   Electron-app shape to model defensively.
//!
//! Platform-gated to macOS only this chunk (guardrail: "platform-
//! specific code is target-gated"); the Linux equivalent
//! (`~/.config/Cursor/User/...`) is the independent Linux track's job
//! (#77-#89), not re-derived here as a guess.
//!
//! `~/.cursor` (chats/, projects/, CLI state) is proposed as a second,
//! non-decomposed location -- same "reported, not decomposed" treatment
//! `crate::locations::opencode` gives its own config/cache roots --
//! because this chunk found no primary-source confirmation of its
//! interior shape at all.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const CURSOR_DETECTOR_ID: &str = "cursor";

pub struct CursorDetector;

impl Detector for CursorDetector {
    fn id(&self) -> &'static str {
        CURSOR_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Cursor"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "community-reverse-engineered (github.com/Callum-Ward/cursaves, \
         github.com/thomas-pedersen/cursor-chat-browser), no official Cursor documentation page \
         found this chunk -- lower confidence than primary-source-backed rows; macOS only, Linux \
         deferred to the Linux track (#77-#89); see crate::agents::cursor"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let app_support = env
            .home
            .join("Library")
            .join("Application Support")
            .join("Cursor");
        let cli_root = env.home.join(".cursor");
        vec![
            ProposedLocation {
                detector_id: CURSOR_DETECTOR_ID.to_string(),
                path: Some(app_support),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Cursor editor-profile root: User/globalStorage (chat/composer content, \
                     protected SQLite), User/workspaceStorage (per-workspace index + \
                     workspace.json linkage), User/History (local file-history snapshots), plus \
                     Cache/CachedData/CachedExtensionVSIXs/logs; see crate::agents::cursor"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: CURSOR_DETECTOR_ID.to_string(),
                path: Some(cli_root),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Cursor CLI root (chats/, projects/, CLI state); no confirmed interior \
                     layout this chunk -- reported as an opaque external unit, not decomposed"
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
    fn convention_paths() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CursorDetector.detect(&env);
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from(
                "/Users/dev/Library/Application Support/Cursor"
            ))
        );
        assert_eq!(got[1].path, Some(PathBuf::from("/Users/dev/.cursor")));
    }

    #[test]
    fn macos_only() {
        assert_eq!(CursorDetector.platforms(), &[Platform::MacOS]);
    }
}
