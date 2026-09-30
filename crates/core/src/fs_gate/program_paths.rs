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
/// has no fixed list.
pub(super) fn candidates(program: Program, home: &Path) -> Vec<PathBuf> {
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
        // Tool-managed removal's simulator runtimes (#177). Not migrated for
        // the detector's own `simctl list devices -j` yet (#199).
        Program::Xcrun => vec![PathBuf::from("/usr/bin/xcrun")],
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

fn trusted_owner(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == 0 || meta.uid() == super::current_uid()
}

/// A regular executable owned by root or the current user that neither
/// the group nor the world can write.
fn trusted_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_file() || m.mode() & 0o111 == 0 {
        return Err(format!("{} is not an executable file", path.display()));
    }
    if !trusted_owner(&m) || m.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is writable by another user or owned by one; swamp runs a manager only you or \
             root can change",
            path.display()
        ));
    }
    Ok(())
}

/// A directory owned by root or the current user that neither the group
/// nor the world can write.
pub(super) fn trusted_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_dir() || !trusted_owner(&m) || m.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is writable by another user or owned by one",
            path.display()
        ));
    }
    Ok(())
}

/// A resolved program: the canonical executable, and the directory that
/// heads the child's `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Resolved {
    pub exe: PathBuf,
    pub path_head: PathBuf,
}

/// The first of `candidates` that exists, checked; it is never skipped for
/// a later one (an untrusted `/opt/homebrew/bin/mise` must not silently
/// become `~/.cargo/bin/mise`). The real file (symlinks followed:
/// Homebrew's `bin/mise` links into the Cellar) and its directory must be
/// trusted. The child's `PATH` starts with the candidate's own directory
/// only when that is trusted too; otherwise (Homebrew's `bin` is
/// group-writable by design) with the real file's directory.
pub(super) fn resolve_strict(candidates: &[PathBuf]) -> Result<Resolved, String> {
    let Some(found) = candidates
        .iter()
        .find(|c| std::fs::symlink_metadata(c).is_ok())
    else {
        let dirs: Vec<String> = candidates
            .iter()
            .filter_map(|c| c.parent().map(|d| d.display().to_string()))
            .collect();
        return Err(format!(
            "not found in {} (swamp does not search PATH)",
            if dirs.is_empty() {
                "any directory swamp checks".to_string()
            } else {
                dirs.join(", ")
            }
        ));
    };
    let exe = std::fs::canonicalize(found)
        .map_err(|e| format!("{} could not be resolved: {e}", found.display()))?;
    trusted_file(&exe)?;
    let real_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    trusted_dir(&real_dir)?;
    let path_head = match found.parent() {
        Some(d) if trusted_dir(d).is_ok() => d.to_path_buf(),
        _ => real_dir,
    };
    Ok(Resolved { exe, path_head })
}

/// The only variables of swamp's own environment a mise child sees.
/// Its tracked configs live under the state dir (`XDG_STATE_HOME`,
/// `MISE_STATE_DIR`), so those pass too: without them mise's prune would
/// answer for a different set of configs than the user's shell.
pub const MISE_PASSTHROUGH: [&str; 9] = [
    "MISE_DATA_DIR",
    "MISE_CONFIG_DIR",
    "MISE_CACHE_DIR",
    "MISE_STATE_DIR",
    "MISE_GLOBAL_CONFIG_FILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// The environment a migrated program's child gets.
fn scrubbed_env(program: Program, exe: &Path, home: &str) -> Vec<(String, String)> {
    let parent: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    child_env(
        program,
        exe.parent().unwrap_or(Path::new("/")),
        home,
        &parent,
    )
    .env
}

/// A child environment, and what swamp noted while building it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChildEnv {
    pub env: Vec<(String, String)>,
    /// Plain notes for a confirm (`DEVELOPER_DIR ignored: ...`).
    pub notes: Vec<String>,
    /// The Xcode developer dir passed on, when one was.
    pub developer_dir: Option<String>,
}

/// The one child-environment builder: from scratch, `path_head` first on
/// `PATH`, and only the named variables of `parent`: mise's own directory
/// settings for mise, a validated `DEVELOPER_DIR` for xcrun.
pub(super) fn child_env(
    program: Program,
    path_head: &Path,
    home: &str,
    parent: &[(String, String)],
) -> ChildEnv {
    let dir = path_head.display().to_string();
    let mut notes = Vec::new();
    let mut developer_dir = None;
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
    let get = |name: &str| {
        parent
            .iter()
            .find(|(k, v)| k == name && !v.is_empty())
            .map(|(_, v)| v.clone())
    };
    if program == Program::Mise {
        // Exactly what the mise detector honors to find the store, plus the
        // XDG directories mise itself reads (checked with mise under
        // `env -i`): the probe must describe the store the unit measures.
        for name in MISE_PASSTHROUGH {
            if let Some(v) = get(name) {
                env.push((name.to_string(), v));
            }
        }
    }
    if program == Program::Xcrun
        && let Some(d) = get("DEVELOPER_DIR")
    {
        match developer_dir_ok(Path::new(&d)) {
            Ok(()) => {
                env.push(("DEVELOPER_DIR".to_string(), d.clone()));
                developer_dir = Some(d);
            }
            Err(why) => notes.push(format!("DEVELOPER_DIR ignored: {why}")),
        }
    }
    ChildEnv {
        env,
        notes,
        developer_dir,
    }
}

/// A `DEVELOPER_DIR` swamp passes on: a real (not linked) directory only
/// the user or root can change, holding a trusted `simctl`.
fn developer_dir_ok(d: &Path) -> Result<(), String> {
    if !d.is_absolute() {
        return Err(format!("{} is not absolute", d.display()));
    }
    let m = std::fs::symlink_metadata(d).map_err(|e| format!("{}: {e}", d.display()))?;
    if m.file_type().is_symlink() {
        return Err(format!("{} is a symlink", d.display()));
    }
    trusted_dir(d)?;
    let simctl = [
        d.join("usr/bin/simctl"),
        d.join("Contents/Developer/usr/bin/simctl"),
    ]
    .into_iter()
    .find(|p| std::fs::symlink_metadata(p).is_ok())
    .ok_or_else(|| format!("{} holds no usr/bin/simctl", d.display()))?;
    trusted_file(&simctl)?;
    if let Some(parent) = simctl.parent() {
        trusted_dir(parent)?;
    }
    Ok(())
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
        None => match resolve_strict(&candidates(program, Path::new(&home))) {
            Ok(r) => Some(r.exe),
            Err(why) if why.starts_with("not found") => None,
            Err(why) => return Err(io::Error::new(io::ErrorKind::PermissionDenied, why)),
        },
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

    /// Owner decision (#177 review): a candidate that exists but cannot
    /// be trusted refuses; it is never skipped for a later one. A missing
    /// one is skipped.
    #[test]
    fn an_untrusted_candidate_refuses_and_a_missing_one_is_skipped() {
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
        assert!(resolve_strict(&[not_exec, good.clone()]).is_err());
        assert!(resolve_strict(&[writable, good.clone()]).is_err());
        let r = resolve_strict(&[dir.path().join("missing"), link]).unwrap();
        assert_eq!(
            r.exe,
            std::fs::canonicalize(&good).unwrap(),
            "a symlink is followed"
        );
    }

    #[test]
    fn mise_gets_its_state_dirs_and_nothing_else_of_mise() {
        let parent: Vec<(String, String)> = [
            ("XDG_STATE_HOME", "/s"),
            ("MISE_STATE_DIR", "/m"),
            ("MISE_YES", "1"),
            ("MISE_NODE_VERSION", "20"),
            ("RUSTUP_TOOLCHAIN", "x"),
            ("DEVELOPER_DIR", "/nonexistent"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let e = child_env(
            Program::Mise,
            Path::new("/opt/homebrew/bin"),
            "/Users/x",
            &parent,
        );
        let names: Vec<&str> = e.env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"XDG_STATE_HOME") && names.contains(&"MISE_STATE_DIR"));
        for dropped in [
            "MISE_YES",
            "MISE_NODE_VERSION",
            "RUSTUP_TOOLCHAIN",
            "DEVELOPER_DIR",
        ] {
            assert!(!names.contains(&dropped), "{dropped}");
        }
        let x = child_env(Program::Xcrun, Path::new("/usr/bin"), "/Users/x", &parent);
        assert!(!x.env.iter().any(|(k, _)| k == "DEVELOPER_DIR"));
        assert!(
            x.notes[0].starts_with("DEVELOPER_DIR ignored"),
            "{:?}",
            x.notes
        );
    }
}
