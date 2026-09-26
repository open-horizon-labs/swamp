//! Windsurf / Devin Desktop editor-profile storage: macOS
//! `~/Library/Application Support/Devin` (current) and
//! `~/Library/Application Support/Windsurf` (legacy), plus the separate
//! `~/.codeium/windsurf` root.
//!
//! Lower confidence than every other row in this catalog, disclosed
//! honestly rather than hidden. Re-checked 2026-09-22 against
//! <https://docs.devin.ai/desktop/devin-desktop-faq> (docs.windsurf.com
//! 307-redirects there; checked during implementation, never learned
//! from a real installation on this machine -- PRIVACY IS A HARD RULE;
//! vendored at
//! `crates/core/tests/fixtures/upstream/windsurf-devin/2026-09-22/devin-desktop-faq.md`).
//!
//! What that page **does** establish:
//! - the rename, and both profile roots: macOS
//!   `~/Library/Application Support/Windsurf/` (legacy, read) ->
//!   `~/Library/Application Support/Devin/` (current, read + write);
//!   Windows `%APPDATA%\Windsurf\` -> `%APPDATA%\Devin\`; Linux
//!   `~/.config/Windsurf/` -> `~/.config/Devin/`. Both are modelled: an
//!   installation mid-migration has bytes in both, and naming only the
//!   legacy root under-reports a current install entirely.
//! - what a profile root contains: `User/settings.json`,
//!   `User/keybindings.json`, `User/snippets/`, `globalStorage/`,
//!   `Workspaces/`, `argv.json`.
//! - that `~/.codeium/` is **not** changing in the rename and stays
//!   read-write, holding `user_settings.pb`, `mcp_config.json` and
//!   `windsurf/{global_workflows,skills,bin}/`. So that root is
//!   *current*, not legacy -- the opposite of what a "Windsurf is the
//!   old name" reading would assume.
//!
//! What it does **not** establish, which is why the matrix row stays
//! `Unverified`: no `User/workspaceStorage/<id>/`, no per-session or
//! per-workspace file layout, and none of the
//! `Cache`/`CachedData`/`CachedExtensionVSIXs`/`logs` siblings this
//! adapter also models. Windsurf is a VS Code fork, so this catalog
//! *assumes* the same `User/{globalStorage,workspaceStorage,History}`
//! shape Cursor's fork uses (`crate::locations::cursor`), reusing
//! `crate::agents::vscode_family`'s shared identification for exactly
//! that reason. Anything that assumption gets wrong shows up as an
//! honest "(unsupported layout version)" residual rather than a silent
//! miscount, via the same version-marker check
//! `crate::agents::vscode_family` gives every VS-Code-family adapter.
//!
//! `~/.codeium/windsurf` is additionally corroborated by
//! <https://registry.coder.com/modules/coder/windsurf> (a Coder
//! community module) as holding MCP and agent config; treated as a
//! second, non-decomposed location, same as Cursor's `~/.cursor`.
//!
//! Platform-gated to macOS only this chunk, same reasoning as Cursor's,
//! even though the FAQ names the Linux and Windows roots -- those belong
//! to the Linux/Windows tracks, and naming a path is not the same as
//! having a tested adapter for it.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const WINDSURF_DETECTOR_ID: &str = "windsurf";

pub struct WindsurfDetector;

impl Detector for WindsurfDetector {
    fn id(&self) -> &'static str {
        WINDSURF_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Windsurf"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "docs.windsurf.com 307-redirects to docs.devin.ai, whose desktop FAQ (retrieved \
         2026-09-22) confirms the rename and both profile roots -- macOS \
         ~/Library/Application Support/Windsurf (legacy, read) -> .../Devin (current, \
         read-write) -- and names User/settings.json, User/keybindings.json, User/snippets/, \
         globalStorage/, Workspaces/ and argv.json inside them. It does NOT name \
         workspaceStorage/ or the Cache/CachedData/CachedExtensionVSIXs/logs siblings this \
         adapter also models, so the row stays Unverified -- see this module's doc comment and \
         crate::agents::vscode_family's version-marker fallback"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let app_support = env
            .home
            .join("Library")
            .join("Application Support")
            .join("Windsurf");
        // The **current** profile root. Upstream renamed the product and
        // moved the read-write profile here, keeping the Windsurf
        // directory readable:
        // "macOS ~/Library/Application Support/Windsurf/ ->
        //  ~/Library/Application Support/Devin/"
        // (https://docs.devin.ai/desktop/devin-desktop-faq, retrieved
        // 2026-09-22; vendored at
        // `crates/core/tests/fixtures/upstream/windsurf-devin/2026-09-22/devin-desktop-faq.md`).
        // Both are modelled, because a user mid-migration has bytes in
        // both and a row that names only the legacy one under-reports a
        // current installation entirely.
        let devin_support = env
            .home
            .join("Library")
            .join("Application Support")
            .join("Devin");
        let codeium_root = env.home.join(".codeium").join("windsurf");
        vec![
            ProposedLocation {
                detector_id: WINDSURF_DETECTOR_ID.to_string(),
                path: Some(devin_support),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Devin Desktop editor-profile root (the current read-write location after \
                     the Windsurf -> Devin rename); the profile root and its User/ + \
                     globalStorage/ contents are primary-source confirmed, the rest of the \
                     VS-Code-fork shape is not -- see crate::agents::windsurf"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: WINDSURF_DETECTOR_ID.to_string(),
                path: Some(app_support),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Windsurf editor-profile root: the legacy, read-only location after the \
                     Windsurf -> Devin rename. Still modelled because an installation that has \
                     not migrated keeps its bytes here; assumed VS-Code-fork shape beyond the \
                     profile root itself -- see crate::agents::windsurf"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: WINDSURF_DETECTOR_ID.to_string(),
                path: Some(codeium_root),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Windsurf/Devin MCP, workflow, skills and CLI-binary root \
                     (~/.codeium/windsurf). Upstream states this tree is NOT changing in the \
                     rename and remains read-write, so it is current rather than legacy; \
                     reported as an opaque external unit, not decomposed"
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
        let got = WindsurfDetector.detect(&env);
        assert_eq!(
            got.len(),
            3,
            "the current Devin profile, the legacy Windsurf profile, and ~/.codeium/windsurf"
        );
        assert_eq!(
            got[0].path,
            Some(PathBuf::from(
                "/Users/dev/Library/Application Support/Devin"
            )),
            "the current read-write profile root comes first"
        );
        assert_eq!(
            got[1].path,
            Some(PathBuf::from(
                "/Users/dev/Library/Application Support/Windsurf"
            )),
            "the legacy profile still holds bytes on an unmigrated install"
        );
        assert_eq!(
            got[2].path,
            Some(PathBuf::from("/Users/dev/.codeium/windsurf"))
        );
    }

    #[test]
    fn macos_only() {
        assert_eq!(WindsurfDetector.platforms(), &[Platform::MacOS]);
    }
}
