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
//!   variables passed through are mise's own directory settings
//!   (`MISE_DATA_DIR`, `MISE_CONFIG_DIR`, `MISE_CACHE_DIR`,
//!   `MISE_GLOBAL_CONFIG_FILE`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
//!   `XDG_CACHE_HOME`): the same ones the mise detector honors, so the
//!   probe describes the store the unit measures. Nothing else of
//!   `HOMEBREW_*`, `MISE_*` or `RUSTUP_*` in swamp's own environment
//!   reaches the child.
//! * **A fixed working directory** (`/`): a manager that resolves local
//!   configuration up the tree (mise) answers the same wherever swamp was
//!   started.
//!
//! A program with no candidate list keeps its old behavior (its
//! `Program::executable`, the inherited environment) until it is migrated.
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
    /// The program is not migrated to the scrubbed environment: it runs
    /// as `Program::executable` says (an absolute path where one is
    /// fixed, else a `PATH` lookup) with the inherited environment.
    Inherit(&'static str),
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
        Program::Mise => {
            let mut c = vec![
                PathBuf::from("/opt/homebrew/bin/mise"),
                PathBuf::from("/usr/local/bin/mise"),
            ];
            // A relative HOME would name these against the working
            // directory, which is whatever checkout swamp was started in.
            if home.is_absolute() {
                c.push(home.join(".local/bin/mise"));
                c.push(home.join(".cargo/bin/mise"));
            }
            c
        }
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
    Some(
        std::fs::metadata(&path)
            .is_ok_and(|m| m.is_file())
            .then_some(path),
    )
}

#[cfg(not(feature = "testing"))]
fn test_override(_program: Program) -> Option<Option<PathBuf>> {
    None
}

/// Whether `path` may be run: symlinks are followed (Homebrew's
/// `bin/mise` is a link into the Cellar), and the real file must be a
/// regular executable owned by root or the current user that neither the
/// group nor the world can write.
fn usable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(m) = std::fs::metadata(&real) else {
        return false;
    };
    m.is_file()
        && m.mode() & 0o111 != 0
        && m.mode() & 0o022 == 0
        && (m.uid() == 0 || m.uid() == super::current_uid())
}

/// The first of `candidates` that [`usable`] accepts: one that is not
/// executable, or not trustworthy, falls through to the next.
fn first_usable(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|c| usable(c))
}

/// The only variables of swamp's own environment a mise child sees.
const MISE_PASSTHROUGH: [&str; 7] = [
    "MISE_DATA_DIR",
    "MISE_CONFIG_DIR",
    "MISE_CACHE_DIR",
    "MISE_GLOBAL_CONFIG_FILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

/// The environment a migrated program's child gets.
fn scrubbed_env(program: Program, exe: &Path, home: &str) -> Vec<(String, String)> {
    let dir = exe
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
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
    if program == Program::Mise {
        // Exactly what the mise detector honors to find the store, plus the
        // XDG directories mise itself reads (checked with mise under
        // `env -i`): the probe must describe the store the unit measures.
        for name in MISE_PASSTHROUGH {
            if let Some(v) = std::env::var_os(name)
                && let Some(v) = v.to_str()
                && !v.is_empty()
            {
                env.push((name.to_string(), v.to_string()));
            }
        }
    }
    env
}

/// How to start `program`. `NotFound` when it is migrated and no
/// candidate exists.
pub fn plan(program: Program) -> io::Result<Plan> {
    if !migrated(program) {
        return Ok(Plan::Inherit(program.executable()));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let exe = match test_override(program) {
        Some(found) => found,
        None => first_usable(candidates(program, Path::new(&home))),
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
        assert_eq!(plan(Program::Git).unwrap(), Plan::Inherit("git"));
        assert_eq!(plan(Program::Lsof).unwrap(), Plan::Inherit("lsof"));
        assert_eq!(
            plan(Program::Tmutil).unwrap(),
            Plan::Inherit("/usr/bin/tmutil")
        );
    }

    #[test]
    fn the_environment_is_built_from_scratch_and_names_only_what_is_documented() {
        let env = scrubbed_env(
            Program::Brew,
            Path::new("/opt/homebrew/bin/brew"),
            "/Users/x",
        );
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

    #[test]
    fn a_relative_home_yields_no_home_candidates() {
        for home in ["", "relative/home"] {
            let c = candidates(Program::Mise, Path::new(home));
            assert!(c.iter().all(|p| p.is_absolute()), "{c:?}");
            assert_eq!(c.len(), 2);
        }
    }

    #[test]
    fn a_candidate_that_cannot_run_falls_through_to_the_next() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, mode: u32| {
            let p = dir.path().join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        let not_exec = mk("a", 0o644);
        let writable = mk("b", 0o777);
        let good = mk("c", 0o755);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert_eq!(
            first_usable(vec![not_exec.clone(), writable.clone(), good.clone()]),
            Some(good.clone())
        );
        assert_eq!(first_usable(vec![not_exec, writable]), None);
        assert_eq!(
            first_usable(vec![dir.path().join("missing"), link.clone()]),
            Some(link),
            "a symlink to a trustworthy executable is followed"
        );
    }
}
