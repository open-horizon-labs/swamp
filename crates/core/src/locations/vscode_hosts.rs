//! Shared host list for a VS Code **extension** (as opposed to a VS Code
//! **fork**, `crate::locations::cursor`/`windsurf`) that keeps its data
//! under some editor's `globalStorage/<extension-id>/`. The same
//! extension ID can be installed into several editor hosts at once, each
//! with its own, genuinely separate, on-disk `globalStorage` -- #99's
//! explicit acceptance is to "model each host as a separate detector
//! location, dedupe nothing that is genuinely separate storage", so a
//! detector using this list proposes one location *per host*, never a
//! single merged path.
//!
//! Extending this for a host not yet named here (a VS Code fork this
//! catalog doesn't otherwise track) is one line, not a new detector.
//!
//! macOS only this chunk (guardrail: platform-specific code is target-
//! gated); the Linux equivalents (`~/.config/Code/User/...`,
//! `~/.vscode-server/...` is itself already a remote/Linux-shaped path,
//! included here defensively since a devcontainer/remote target can be
//! this very machine even when the *client* is on macOS) are the Linux
//! track's job for the client-local paths (#77-#89).

use std::path::{Path, PathBuf};

/// (display label, `Application Support` subdirectory name) for every
/// macOS VS-Code-family editor host this catalog checks.
pub const MACOS_HOSTS: &[(&str, &str)] = &[
    ("VS Code", "Code"),
    ("VS Code Insiders", "Code - Insiders"),
    ("Cursor", "Cursor"),
    ("Windsurf", "Windsurf"),
];

/// Every `globalStorage/<extension_id>` path this catalog checks for
/// `home`, paired with a human-readable host label used both in the
/// detector's own locations and, unprefixed by any parent directory
/// naming, by the interior adapter (`crate::agents::vscode_family`) to
/// tag every unit it identifies with which host it came from.
pub fn globalstorage_candidates(home: &Path, extension_id: &str) -> Vec<(&'static str, PathBuf)> {
    let mut out: Vec<(&'static str, PathBuf)> = MACOS_HOSTS
        .iter()
        .map(|(label, dir)| {
            (
                *label,
                home.join("Library")
                    .join("Application Support")
                    .join(dir)
                    .join("User")
                    .join("globalStorage")
                    .join(extension_id),
            )
        })
        .collect();
    out.push((
        "VS Code Server (remote)",
        home.join(".vscode-server")
            .join("data")
            .join("User")
            .join("globalStorage")
            .join(extension_id),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_host_is_proposed_once() {
        let home = PathBuf::from("/Users/dev");
        let got = globalstorage_candidates(&home, "some.extension-id");
        assert_eq!(got.len(), MACOS_HOSTS.len() + 1);
        let labels: Vec<&str> = got.iter().map(|(l, _)| *l).collect();
        assert!(labels.contains(&"VS Code"));
        assert!(labels.contains(&"Cursor"));
        assert!(labels.contains(&"VS Code Server (remote)"));
        // Every path is distinct -- no accidental collision between hosts.
        let mut paths: Vec<&PathBuf> = got.iter().map(|(_, p)| p).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), got.len());
    }
}
