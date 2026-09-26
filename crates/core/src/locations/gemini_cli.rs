//! Gemini CLI home: `~/.gemini`, or the whole of `GEMINI_CLI_HOME` when
//! set. Sourced from primary docs during implementation (never a real
//! `~/.gemini` on this machine -- PRIVACY IS A HARD RULE), from
//! <https://github.com/google-gemini/gemini-cli>, current `main` as of
//! this chunk:
//! - `docs/reference/configuration.md`: `settings.json` (user settings),
//!   a configurable context-memory filename (`GEMINI.md` by default),
//!   `bin/` (e.g. `bin/litert/`, downloaded runtime tools),
//!   `trustedFolders.json`. **`GEMINI_CLI_HOME`** is the documented
//!   override for "the root directory for Gemini CLI's user-level
//!   configuration and storage" -- not `GEMINI_CONFIG_HOME`, which this
//!   detector's own earlier planning row (`crate::agents::matrix`, now
//!   corrected) had guessed before this chunk's verification.
//! - `docs/cli/checkpointing.md`: checkpoints at
//!   `~/.gemini/tmp/<project_hash>/checkpoints`, with Git snapshots in a
//!   separate shadow repository at `~/.gemini/history/<project_hash>`,
//!   independent of the project's own `.git`.
//! - `docs/cli/session-management.md`: `/chat save`/`/chat resume` (an
//!   alias for `/resume`) persist saved sessions at
//!   `~/.gemini/tmp/<project_hash>/chats/`.
//! - `packages/core/src/utils/paths.ts`: `getProjectHash(projectRoot)` is
//!   `sha256(projectRoot).hex()` -- confirmed, not guessed, satisfying
//!   #96's explicit bar for treating the hash as project-linkage
//!   evidence. It is a **one-way** function of an absolute path,
//!   however: `crate::agents::gemini_cli` cannot invert a hash back into
//!   a path without a candidate project-path catalog this crate's
//!   identification layer does not have, so a `tmp/<hash>`/`history/
//!   <hash>` directory's linkage is `Unresolved`, honestly, not guessed
//!   or silently dropped -- see that module's doc comment.
//! - OAuth/account credential file names are not documented on any page
//!   this chunk found (`docs/cli/authentication.md` and
//!   `docs/get-started/authentication.md` both 404 against current
//!   `main`); `crate::agents::gemini_cli` protects any top-level file
//!   whose name defensively looks credential-shaped (`oauth`/`cred`
//!   substring) rather than naming an unconfirmed exact filename.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const GEMINI_CLI_DETECTOR_ID: &str = "gemini-cli";

pub struct GeminiCliDetector;

impl Detector for GeminiCliDetector {
    fn id(&self) -> &'static str {
        GEMINI_CLI_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Gemini CLI"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "github.com/google-gemini/gemini-cli docs/reference/configuration.md, \
         docs/cli/checkpointing.md, docs/cli/session-management.md and \
         packages/core/src/utils/paths.ts, current main as of this chunk; project-hash reversal \
         is out of scope -- see crate::agents::gemini_cli"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("GEMINI_CLI_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("GEMINI_CLI_HOME".to_string()),
            ),
            _ => (env.home.join(".gemini"), Provenance::BuiltinConvention),
        };
        let mut out = vec![ProposedLocation {
            detector_id: GEMINI_CLI_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Gemini CLI home: settings/GEMINI.md/extensions/trustedFolders (protected), \
                 per-project-id tmp/ (shell history, checkpoints, saved chats, tmp/bin \
                 downloaded tools) and history/ (shadow Git checkpoint repos); see \
                 crate::agents::gemini_cli"
                    .to_string(),
            ),
        }];
        // Under macOS Seatbelt the CLI cannot write to the home
        // `.gemini` directory, so it routes *runtime* state -- `tmp/`
        // (with `tmp/bin`), `history/`, `projects.json` -- to
        // `~/.cache/.gemini` instead, while settings and credentials
        // stay at the home location above. Upstream:
        // `Storage.getGlobalRuntimeDir()` returns
        // `join(homeDir, '.cache', GEMINI_DIR)` when
        // `process.env['SANDBOX'] === 'sandbox-exec'`
        // (`packages/core/src/config/storage.ts:86-107` @
        // `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`).
        //
        // Proposed only when this process is itself running under that
        // sandbox, because that is the only evidence there is: a
        // `~/.cache/.gemini` on an unsandboxed machine would be a guess,
        // and this detector never guesses a second root.
        if env.env_var("SANDBOX") == Some("sandbox-exec") {
            out.push(ProposedLocation {
                detector_id: GEMINI_CLI_DETECTOR_ID.to_string(),
                path: Some(env.home.join(".cache").join(".gemini")),
                category: StorageCategory::LocalState,
                provenance: Provenance::EnvVar("SANDBOX".to_string()),
                status: LocationStatus::Resolved,
                note: Some(
                    "Gemini CLI runtime state under macOS Seatbelt (SANDBOX=sandbox-exec): the \
                     home .gemini directory is not writable there, so tmp/ (including tmp/bin), \
                     history/ and projects.json live here instead"
                        .to_string(),
                ),
            });
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
    fn convention_path_no_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = GeminiCliDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.gemini")));
    }

    /// The sandbox root, both ways: proposed when this process is under
    /// Seatbelt, and never proposed otherwise.
    #[test]
    fn seatbelt_moves_the_runtime_dir_to_the_cache_and_only_then() {
        let plain =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        assert_eq!(
            GeminiCliDetector.detect(&plain).len(),
            1,
            "a ~/.cache/.gemini outside the sandbox would be a guess, not a detection"
        );

        let sandboxed = Environment::fixture(
            PathBuf::from("/Users/dev"),
            HashMap::from([("SANDBOX".to_string(), "sandbox-exec".to_string())]),
            Platform::MacOS,
        );
        let got = GeminiCliDetector.detect(&sandboxed);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.gemini")));
        assert_eq!(
            got[1].path,
            Some(PathBuf::from("/Users/dev/.cache/.gemini")),
            "under SANDBOX=sandbox-exec the runtime dir moves to ~/.cache/.gemini"
        );

        // A different sandbox value is a different launcher (the
        // container case), which mounts an ephemeral directory at the
        // ordinary path -- not this one.
        let other = Environment::fixture(
            PathBuf::from("/Users/dev"),
            HashMap::from([("SANDBOX".to_string(), "docker".to_string())]),
            Platform::MacOS,
        );
        assert_eq!(GeminiCliDetector.detect(&other).len(), 1);
    }

    #[test]
    fn env_override_replaces_convention() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "GEMINI_CLI_HOME".to_string(),
            "/opt/gemini-home".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = GeminiCliDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/gemini-home")));
        assert!(matches!(got[0].provenance, Provenance::EnvVar(_)));
    }
}
