//! Where an allow-listed program is found, and what environment its
//! child gets. One facility for every spawn that has been migrated to it
//! (the package managers today; the rest under #199, and the tool-managed
//! removal of #177 reuses it as is).
//!
//! The rules, for a program with a candidate list:
//!
//! * **Absolute path from a fixed list.** The executable is the first
//!   candidate that is a regular file. The inherited `PATH` is never
//!   consulted, so a `brew` or `mise` placed earlier on it (a shim, a
//!   wrapper, a hostile checkout's `bin`) is never what swamp runs. None
//!   present is `NotFound`, the same "not installed" a missing binary was.
//! * **Environment from scratch.** The child starts with an empty
//!   environment and gets only: a `PATH` of the program's own directory
//!   plus `/usr/bin:/bin`, `HOME`, `NO_COLOR=1`, `LC_ALL=C`, pagers off and
//!   Homebrew's auto-update, analytics, cleanup and hints off. The one
//!   variable passed through is `MISE_GLOBAL_CONFIG_FILE` for mise (a
//!   person's deliberate choice of global configuration). Nothing else of
//!   `HOMEBREW_*`, `MISE_*` or `RUSTUP_*` in swamp's own environment
//!   reaches the child.
//! * **A fixed working directory** (`/`): a manager that resolves local
//!   configuration up the tree (mise) answers the same wherever swamp was
//!   started.
//!
//! A program with no candidate list keeps its old behavior (a `PATH`
//! lookup, the inherited environment) until it is migrated.
//!
//! **Tests.** With swamp-core's `testing` feature (never in a shipped
//! build graph) `SWAMP_TEST_PROGRAM_DIR` names a directory holding fakes:
//! a program is then `<dir>/<binary>` or `NotFound`, never the real one.
//! Without the feature the variable is not read.

use super::spawn::Program;
use std::io;
use std::path::{Path, PathBuf};

/// How a program is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The program is not migrated: `PATH` lookup, inherited environment.
    Inherit,
    /// Absolute executable, a from-scratch environment and a fixed
    /// working directory.
    Scrubbed(Scrubbed),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrubbed {
    pub exe: PathBuf,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// The fixed locations of `program`, in order. Empty for a program that
/// is not migrated yet.
fn candidates(program: Program, home: &Path) -> Vec<PathBuf> {
    match program {
        Program::Brew => vec![
            PathBuf::from("/opt/homebrew/bin/brew"),
            PathBuf::from("/usr/local/bin/brew"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin/brew"),
        ],
        Program::Mise => vec![
            PathBuf::from("/opt/homebrew/bin/mise"),
            PathBuf::from("/usr/local/bin/mise"),
            home.join(".local/bin/mise"),
            home.join(".cargo/bin/mise"),
        ],
        _ => Vec::new(),
    }
}

/// Whether `program` is migrated to this facility.
fn migrated(program: Program) -> bool {
    matches!(program, Program::Brew | Program::Mise)
}

#[cfg(feature = "testing")]
fn test_override(program: Program) -> Option<Option<PathBuf>> {
    let dir = std::env::var_os("SWAMP_TEST_PROGRAM_DIR")?;
    let path = Path::new(&dir).join(program.binary());
    Some(std::fs::metadata(&path).is_ok_and(|m| m.is_file()).then_some(path))
}

#[cfg(not(feature = "testing"))]
fn test_override(_program: Program) -> Option<Option<PathBuf>> {
    None
}

fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// The environment a migrated program's child gets.
fn scrubbed_env(program: Program, exe: &Path, home: &str) -> Vec<(String, String)> {
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let mut env: Vec<(String, String)> = [
        ("PATH", format!("{dir}:/usr/bin:/bin")),
        ("HOME", home.to_string()),
        ("NO_COLOR", "1".to_string()),
        ("LC_ALL", "C".to_string()),
        ("PAGER", "cat".to_string()),
        ("GIT_PAGER", "cat".to_string()),
        ("HOMEBREW_PAGER", "cat".to_string()),
        ("HOMEBREW_NO_AUTO_UPDATE", "1".to_string()),
        ("HOMEBREW_NO_ANALYTICS", "1".to_string()),
        ("HOMEBREW_NO_INSTALL_CLEANUP", "1".to_string()),
        ("HOMEBREW_NO_ENV_HINTS", "1".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    if program == Program::Mise
        && let Some(v) = std::env::var_os("MISE_GLOBAL_CONFIG_FILE")
        && let Some(v) = v.to_str()
    {
        env.push(("MISE_GLOBAL_CONFIG_FILE".to_string(), v.to_string()));
    }
    env
}

/// How to start `program`. `NotFound` when it is migrated and no
/// candidate exists.
pub fn plan(program: Program) -> io::Result<Plan> {
    if !migrated(program) {
        return Ok(Plan::Inherit);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let exe = match test_override(program) {
        Some(found) => found,
        None => candidates(program, Path::new(&home))
            .into_iter()
            .find(|c| is_file(c)),
    };
    let Some(exe) = exe else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} was not found in its known locations", program.binary()),
        ));
    };
    Ok(Plan::Scrubbed(Scrubbed {
        env: scrubbed_env(program, &exe, &home),
        exe,
        cwd: PathBuf::from("/"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_is_not_migrated_keeps_its_old_behavior() {
        assert_eq!(plan(Program::Git).unwrap(), Plan::Inherit);
        assert_eq!(plan(Program::Lsof).unwrap(), Plan::Inherit);
    }

    #[test]
    fn the_environment_is_built_from_scratch_and_names_only_what_is_documented() {
        let env = scrubbed_env(Program::Brew, Path::new("/opt/homebrew/bin/brew"), "/Users/x");
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        for k in names.iter() {
            assert!(
                ["PATH", "HOME", "NO_COLOR", "LC_ALL", "PAGER", "GIT_PAGER"].contains(k)
                    || k.starts_with("HOMEBREW_NO_")
                    || *k == "HOMEBREW_PAGER",
                "{k}"
            );
        }
        assert!(env.contains(&("PATH".into(), "/opt/homebrew/bin:/usr/bin:/bin".into())));
        assert!(!names.iter().any(|k| k.starts_with("MISE_")));
    }
}
