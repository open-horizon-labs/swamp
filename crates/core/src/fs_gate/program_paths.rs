//! Where every allow-listed program is found, and what environment its
//! child gets (#199). The one facility for every spawn: the package
//! managers, git, gh, docker, the system tools, and tool-managed removal
//! (#177).
//!
//! The rules:
//!
//! * **Absolute path from a fixed list** ([`candidates`], per program and
//!   platform). The first candidate that exists is used or refused, never
//!   skipped for a later one. The inherited `PATH` is never consulted, so
//!   a program placed earlier on it (a shim, a wrapper, a hostile
//!   checkout's `bin`) is never what swamp runs. None present is
//!   `NotFound`: "not available", the same as not installed.
//! * **Checked file and directories.** The real file is an executable
//!   owned by the user or root, not group- or world-writable; the
//!   candidate's directory, every directory a symlink chain passes
//!   through, and the real file's directory are owned by the user or root,
//!   not world-writable, and group-writable only for the macOS `admin`
//!   group.
//! * **Home-relative candidates** (`~/.local/bin`, `~/.cargo/bin`,
//!   `~/.docker/bin`, `~/.orbstack/bin`) come after every system location:
//!   any process the user runs can plant a file there, so they are used
//!   only when no system location has the program, which is about the
//!   trust the user's own `PATH` gives them.
//! * **Environment.** `brew` and `mise` start from an empty environment
//!   and get only: a `PATH` of the program's own directory plus
//!   `/usr/bin:/bin`, `HOME`, `NO_COLOR=1`, `LC_ALL=C`, pagers off and
//!   Homebrew's auto-update, analytics, cleanup and hints off, plus mise's
//!   own directory settings (the ones the mise detector honors). Every
//!   other program ([`Plan::Fixed`]) keeps the inherited environment for
//!   its credentials and contexts, minus loader injection and exec hooks
//!   ([`removed_for_fixed`]: `DYLD_*`, `LD_*`, `GIT_CONFIG*`, `GIT_SSH*`,
//!   `GIT_EXEC_PATH`, ...) and with a fixed `PATH` ([`fixed_path`]).
//! * **A fixed working directory** (`/`) for the managers: a manager that
//!   resolves local configuration up the tree (mise) answers the same
//!   wherever swamp was started.
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
    /// Unit tests of the spawn layer only: a bare name (`sh`) looked up
    /// on `PATH`. No production spawn is planned this way (#199).
    #[cfg(test)]
    Inherit(&'static str),
    /// A program found at one of its fixed locations and checked like any
    /// other (owner, mode, directory), run with the inherited environment:
    /// `git`, `gh` and `docker` need the user's own credentials and
    /// contexts, which a from-scratch environment would drop. Never a
    /// `PATH` lookup (#199). `exe` is the checked, symlink-resolved file;
    /// `arg0` is the program's own name, so a multi-call binary reached
    /// through a link (OrbStack's `docker` -> `docker-tools`) still sees
    /// the name it dispatches on.
    Fixed {
        exe: PathBuf,
        arg0: &'static str,
        /// The child's `PATH`: the program's own directory, then the
        /// system directories, then Homebrew's and `/usr/local/bin` only
        /// when they pass the directory check.
        path: String,
    },
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

/// The fixed locations of `program`, in order: the first that exists is
/// the one used (and checked); none present is "not available". Never the
/// inherited `PATH` (#199): a shim earlier on it is never what swamp runs.
/// macOS and Linux lists differ where the programs live in different
/// places; a location that does not exist on a platform simply never
/// matches.
pub(super) fn candidates(program: Program, home: &Path) -> Vec<PathBuf> {
    let name = program.binary();
    let at =
        |dirs: &[&str]| -> Vec<PathBuf> { dirs.iter().map(|d| Path::new(d).join(name)).collect() };
    // Linux: the system directories in search order, then Linuxbrew.
    const LINUX_SYSTEM: &[&str] = &["/usr/bin", "/usr/local/bin", "/bin", "/usr/sbin", "/sbin"];
    const LINUXBREW: &str = "/home/linuxbrew/.linuxbrew/bin";
    let linux_with_brew = || {
        let mut c = at(LINUX_SYSTEM);
        c.push(Path::new(LINUXBREW).join(name));
        c
    };
    let mac = cfg!(target_os = "macos");
    // A relative HOME would name these against the working directory,
    // which is whatever checkout swamp was started in.
    let in_home = |c: &mut Vec<PathBuf>, rels: &[&str]| {
        if home.is_absolute() {
            c.extend(rels.iter().map(|r| home.join(r)));
        }
    };
    match program {
        Program::Brew if mac => at(&["/opt/homebrew/bin", "/usr/local/bin"]),
        Program::Brew => {
            let mut c = vec![Path::new(LINUXBREW).join(name)];
            c.extend(at(&["/usr/local/bin", "/usr/bin"]));
            c
        }
        Program::Mise => {
            let mut c = if mac {
                at(&["/opt/homebrew/bin", "/usr/local/bin"])
            } else {
                let mut c = at(&["/usr/local/bin", "/usr/bin"]);
                c.push(Path::new(LINUXBREW).join(name));
                c
            };
            in_home(&mut c, &[".local/bin/mise", ".cargo/bin/mise"]);
            c
        }
        // macOS-only programs: Apple's own locations.
        Program::Xcrun | Program::Plutil | Program::Defaults | Program::Tmutil => at(&["/usr/bin"]),
        Program::Diskutil => at(&["/usr/sbin"]),
        Program::Launchctl => at(&["/bin"]),
        Program::Lsof if mac => at(&["/usr/sbin"]),
        Program::Du | Program::Id if mac => at(&["/usr/bin"]),
        Program::Df if mac => at(&["/bin"]),
        // Linux-only programs, and the system tools on Linux.
        Program::Systemctl
        | Program::Loginctl
        | Program::Lsof
        | Program::Du
        | Program::Df
        | Program::Id => at(LINUX_SYSTEM),
        // Homebrew's first on macOS: it is the git/gh the user installed
        // and runs; `/usr/bin/git` is Apple's.
        Program::Git | Program::Gh if mac => {
            at(&["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"])
        }
        Program::Git | Program::Gh => linux_with_brew(),
        // Docker Desktop and OrbStack both link `/usr/local/bin/docker`;
        // OrbStack also installs `~/.orbstack/bin`, Docker Desktop
        // `~/.docker/bin`. Linux packages install `/usr/bin/docker`.
        Program::Docker => {
            let mut c = if mac {
                at(&["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"])
            } else {
                linux_with_brew()
            };
            in_home(&mut c, &[".docker/bin/docker", ".orbstack/bin/docker"]);
            c
        }
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

/// Who may be trusted to own or write a program directory: the current
/// user, root, and, on macOS only, the `admin` group.
struct Trust {
    uid: u32,
    /// The macOS `admin` group's id, looked up by name; `None` on Linux
    /// (there is no such group, and Linuxbrew directories are user-owned
    /// and not group-writable).
    admin_gid: Option<u32>,
}

impl Trust {
    fn current() -> Self {
        Self {
            uid: super::current_uid(),
            admin_gid: if cfg!(target_os = "macos") {
                super::sys::group_id("admin")
            } else {
                None
            },
        }
    }
}

/// The directory rule, on plain metadata so tests can state any owner and
/// group (#205 follow-up to the #190 resolver). A directory is trusted when
/// it is owned by the current user or root, is not world-writable, and is
/// group-writable only for the macOS `admin` group: admin members can
/// already use sudo, so letting them write where Homebrew keeps its
/// programs grants no power they lack. Any other group, any other owner,
/// or world-write is refused, and so is any group-write on Linux.
fn check_dir(
    path: &Path,
    is_dir: bool,
    mode: u32,
    uid: u32,
    gid: u32,
    trust: &Trust,
    names: &dyn Fn(u32) -> String,
) -> Result<(), String> {
    if !is_dir {
        return Err(format!("{} is not a directory", path.display()));
    }
    if uid != 0 && uid != trust.uid {
        return Err(format!(
            "{} is owned by another user; swamp runs a program only you or root can change",
            path.display()
        ));
    }
    if mode & 0o002 != 0 {
        return Err(format!("{} is writable by every user", path.display()));
    }
    if mode & 0o020 != 0 && trust.admin_gid != Some(gid) {
        return Err(format!(
            "{} is writable by group {}",
            path.display(),
            names(gid)
        ));
    }
    Ok(())
}

/// A directory the rule in [`check_dir`] trusts.
pub(super) fn trusted_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    check_dir(
        path,
        m.is_dir(),
        m.mode(),
        m.uid(),
        m.gid(),
        &Trust::current(),
        &super::sys::group_name,
    )
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
/// only when that is trusted too; otherwise with the real file's
/// directory.
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
    // Every directory the path passes through on its way to the file is
    // checked: the candidate's own, each link's in a chain, the real
    // file's. A link planted in a writable directory, pointing at a
    // perfectly trusted binary, is still a link anyone could repoint.
    let mut cur = found.clone();
    for _ in 0..40 {
        let dir = cur.parent().map(Path::to_path_buf).unwrap_or_default();
        trusted_dir(&dir)?;
        match std::fs::read_link(&cur) {
            Ok(next) => {
                cur = if next.is_absolute() {
                    next
                } else {
                    dir.join(next)
                }
            }
            Err(_) => break,
        }
    }
    let real_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    trusted_dir(&real_dir)?;
    let path_head = found.parent().map(Path::to_path_buf).unwrap_or(real_dir);
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

/// Inherited variables a `Plan::Fixed` child never sees (#199): loader
/// injection, and every variable through which git, gh, ssh or a shell
/// runs a program or reads configuration the user did not write to their
/// own config files (a checkout's direnv or mise `[env]` can set any of
/// them). Exact names and prefixes; everything else (credentials and
/// contexts: `HOME`, `DOCKER_*`, `GH_TOKEN`, `XDG_*`, `SSH_AUTH_SOCK`,
/// `TMPDIR`, `LANG`/`LC_*`) passes.
const FIXED_REMOVED_PREFIXES: &[&str] = &["DYLD_", "LD_", "GIT_CONFIG", "GIT_SSH"];
const FIXED_REMOVED: &[&str] = &[
    "GIT_EXEC_PATH",
    "GIT_EXTERNAL_DIFF",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_PROXY_COMMAND",
    "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR",
    "GIT_TEMPLATE_DIR",
    "EDITOR",
    "VISUAL",
    "GH_EDITOR",
    "BROWSER",
    "GH_BROWSER",
    "BASH_ENV",
    "ENV",
    "PERL5LIB",
    "PERL5OPT",
];

/// Whether an inherited variable is removed for a `Plan::Fixed` child.
pub(super) fn removed_for_fixed(key: &str) -> bool {
    FIXED_REMOVED.contains(&key) || FIXED_REMOVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// A `Plan::Fixed` child's `PATH`: `head` (the program's own, checked
/// directory), the system directories, and Homebrew's and
/// `/usr/local/bin` only when they pass [`trusted_dir`] (gh finds git
/// there, docker its credential helpers). Never the inherited `PATH`.
fn fixed_path(head: &Path) -> String {
    let mut dirs: Vec<String> = vec![head.display().to_string()];
    for d in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(d.to_string());
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if std::fs::metadata(d).is_ok() && trusted_dir(Path::new(d)).is_ok() {
            dirs.push(d.to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs.join(":")
}

/// Whether `program` is present at one of its fixed locations (or, in a
/// test build with `SWAMP_TEST_PROGRAM_DIR`, as a fake): present, not
/// necessarily trusted or runnable.
pub fn present(program: Program) -> bool {
    if let Some(found) = test_override(program) {
        return found.is_some();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    candidates(program, Path::new(&home))
        .iter()
        .any(|c| std::fs::symlink_metadata(c).is_ok())
}

/// How to start `program`: its first fixed location, checked. `NotFound`
/// ("not available") when none exists; `PermissionDenied`, naming what
/// failed, when the first that exists is not trusted.
pub fn plan(program: Program) -> io::Result<Plan> {
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
            format!(
                "{} is not available: not found in its known locations (swamp does not search PATH)",
                program.binary()
            ),
        ));
    };
    if !migrated(program) {
        let head = exe.parent().unwrap_or(Path::new("/usr/bin")).to_path_buf();
        return Ok(Plan::Fixed {
            path: fixed_path(&head),
            exe,
            arg0: program.binary(),
        });
    }
    Ok(Plan::Scrubbed(Scrubbed {
        env: scrubbed_env(program, &exe, &home),
        exe,
        cwd: PathBuf::from("/"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 501;
    const ADMIN: u32 = 80;
    const STAFF: u32 = 20;
    fn names(gid: u32) -> String {
        match gid {
            ADMIN => "admin".into(),
            STAFF => "staff".into(),
            g => format!("gid {g}"),
        }
    }
    fn mac() -> Trust {
        Trust {
            uid: ME,
            admin_gid: Some(ADMIN),
        }
    }
    fn linux() -> Trust {
        Trust {
            uid: ME,
            admin_gid: None,
        }
    }
    fn dir(mode: u32, uid: u32, gid: u32, t: &Trust) -> Result<(), String> {
        check_dir(
            Path::new("/opt/homebrew/bin"),
            true,
            mode,
            uid,
            gid,
            t,
            &names,
        )
    }

    /// The standard Apple Silicon Homebrew layout: user-owned, admin
    /// group-writable. Tempting wrong patch (the one this replaces):
    /// refusing every group-writable directory, which refuses brew on
    /// most Macs.
    #[test]
    fn an_admin_group_writable_dir_you_own_is_trusted_on_macos() {
        assert_eq!(dir(0o40775, ME, ADMIN, &mac()), Ok(()));
        assert_eq!(dir(0o40775, 0, ADMIN, &mac()), Ok(()));
        assert_eq!(dir(0o40755, ME, STAFF, &mac()), Ok(()));
    }

    /// Tempting wrong patch: accepting any group-write, or matching the
    /// group by "is the user a member".
    #[test]
    fn another_group_world_write_or_another_owner_is_refused() {
        assert_eq!(
            dir(0o40775, ME, STAFF, &mac()),
            Err("/opt/homebrew/bin is writable by group staff".into())
        );
        assert!(
            dir(0o40777, ME, ADMIN, &mac())
                .unwrap_err()
                .contains("every user")
        );
        assert!(
            dir(0o40757, ME, STAFF, &mac())
                .unwrap_err()
                .contains("every user")
        );
        assert!(
            dir(0o40755, 502, ADMIN, &mac())
                .unwrap_err()
                .contains("another user")
        );
        assert!(
            check_dir(Path::new("/x"), false, 0o100755, ME, STAFF, &mac(), &names).is_err(),
            "a non-directory is never a trusted directory"
        );
    }

    /// Linux has no admin group: any group-write is refused, even with a
    /// group numbered like macOS's admin.
    #[test]
    fn linux_refuses_any_group_writable_dir() {
        assert!(dir(0o40775, ME, ADMIN, &linux()).is_err());
        assert_eq!(dir(0o40755, ME, ADMIN, &linux()), Ok(()));
    }

    /// The binary itself is still held to the strict rule: owned by you or
    /// root and not group- or world-writable, whatever its directory.
    #[test]
    fn the_binary_checks_still_apply_under_a_trusted_dir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("brew");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(
            trusted_file(&exe).is_err(),
            "a group-writable binary is refused"
        );
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            trusted_file(&exe).is_err(),
            "a non-executable file is refused"
        );
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(trusted_file(&exe).is_ok());
    }

    /// The tools that are not package managers run from a fixed location
    /// with the inherited environment (their credentials and contexts),
    /// named by their own arg0, or are not available; never scrubbed, never
    /// a bare name.
    #[test]
    fn other_programs_run_from_a_fixed_location_with_their_own_name() {
        for p in Program::ALL
            .iter()
            .filter(|p| !matches!(p, Program::Brew | Program::Mise))
        {
            match plan(*p) {
                Ok(Plan::Fixed { exe, arg0, .. }) => {
                    assert!(exe.is_absolute(), "{p:?}");
                    assert_eq!(arg0, p.binary());
                }
                Ok(other) => panic!("{p:?} planned as {other:?}"),
                Err(e) => assert!(
                    e.kind() == io::ErrorKind::NotFound
                        || e.kind() == io::ErrorKind::PermissionDenied,
                    "{p:?}: {e}"
                ),
            }
        }
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

    /// #199: every program has a fixed, absolute candidate list, so no
    /// program is ever looked up by bare name. Tempting wrong patch:
    /// migrating the programs the auditor named and leaving the rest on
    /// `PATH` (an empty list fell back to the bare name before).
    #[test]
    fn every_program_has_absolute_candidates() {
        for p in Program::ALL {
            let c = candidates(*p, Path::new("/Users/x"));
            assert!(!c.is_empty(), "{p:?} has no fixed location");
            assert!(c.iter().all(|c| c.is_absolute()), "{p:?}: {c:?}");
        }
    }

    /// Linux: the system tools are looked for in every system directory,
    /// and git, gh and docker in Linuxbrew too, so a fleet runner's
    /// `/usr/bin/gh` or `/home/linuxbrew/.linuxbrew/bin/git` is found.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_lists_cover_the_system_dirs_and_linuxbrew() {
        let home = Path::new("/home/x");
        for p in [
            Program::Lsof,
            Program::Du,
            Program::Df,
            Program::Id,
            Program::Systemctl,
            Program::Loginctl,
            Program::Git,
            Program::Gh,
            Program::Docker,
        ] {
            let c = candidates(p, home);
            for d in ["/usr/bin", "/usr/local/bin", "/bin", "/usr/sbin", "/sbin"] {
                assert!(
                    c.contains(&Path::new(d).join(p.binary())),
                    "{p:?} misses {d}"
                );
            }
        }
        for p in [
            Program::Git,
            Program::Gh,
            Program::Docker,
            Program::Brew,
            Program::Mise,
        ] {
            let c = candidates(p, home);
            assert!(
                c.contains(&Path::new("/home/linuxbrew/.linuxbrew/bin").join(p.binary())),
                "{p:?} misses Linuxbrew"
            );
        }
    }

    /// docs/usage.md's table names every candidate directory of this
    /// platform's lists, in the program's row. Tempting wrong patch:
    /// adding a location to the code and not to the table users read.
    #[test]
    fn the_usage_table_matches_the_candidate_lists() {
        let doc =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/usage.md"))
                .unwrap();
        let col = if cfg!(target_os = "macos") { 1 } else { 2 };
        let home = Path::new("/home/x");
        for p in Program::ALL {
            let row = doc
                .lines()
                .find(|l| {
                    l.starts_with("| `")
                        && l.split('|')
                            .nth(1)
                            .is_some_and(|c| c.contains(&format!("`{}`", p.binary())))
                })
                .unwrap_or_else(|| panic!("no usage row for {}", p.binary()));
            let cell = row.split('|').nth(col + 1).unwrap();
            // A program this platform never runs (systemctl on macOS,
            // diskutil on Linux) is "(not used)" there.
            if cell.contains("(not used)") {
                continue;
            }
            for c in candidates(*p, home) {
                let dir = c.parent().unwrap();
                let shown = match dir.strip_prefix(home) {
                    Ok(rel) => format!("`~/{}`", rel.display()),
                    Err(_) => format!("`{}`", dir.display()),
                };
                assert!(
                    cell.contains(&shown),
                    "{}: {shown} missing from {cell}",
                    p.binary()
                );
            }
        }
    }

    /// The variables git, gh and docker need still reach a `Plan::Fixed`
    /// child; only the loader and exec-hook ones are removed. Tempting
    /// wrong patch: `env_clear`, which drops `GH_TOKEN`, `DOCKER_HOST`,
    /// `SSH_AUTH_SOCK` and makes the tools answer for nobody.
    #[test]
    fn fixed_children_keep_credentials_and_contexts() {
        for k in [
            "HOME",
            "DOCKER_CONFIG",
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "GH_TOKEN",
            "GH_HOST",
            "GH_CONFIG_DIR",
            "GITHUB_TOKEN",
            "XDG_CONFIG_HOME",
            "SSH_AUTH_SOCK",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "USER",
        ] {
            assert!(!removed_for_fixed(k), "{k} must pass");
        }
        for k in [
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_GLOBAL",
            "GIT_EXEC_PATH",
            "GIT_SSH_COMMAND",
            "GIT_SSH",
            "GIT_EXTERNAL_DIFF",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
            "GIT_PROXY_COMMAND",
            "BASH_ENV",
        ] {
            assert!(removed_for_fixed(k), "{k} must be removed");
        }
        let p = fixed_path(Path::new("/opt/x/bin"));
        assert!(
            p.starts_with("/opt/x/bin:/usr/bin:/bin:/usr/sbin:/sbin"),
            "{p}"
        );
    }

    /// A world-writable directory holding the program, or a directory
    /// owned by another user, refuses; a symlinked binary is followed to
    /// the real file, which is checked too. Tempting wrong patch:
    /// checking only the file, so a writable directory lets anyone swap
    /// it.
    #[test]
    fn a_world_writable_dir_refuses_and_a_link_is_checked_at_its_target() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let open = tmp.path().join("open");
        std::fs::create_dir(&open).unwrap();
        let exe = open.join("git");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = resolve_strict(std::slice::from_ref(&exe)).unwrap_err();
        assert!(err.contains("writable by every user"), "{err}");
        // A trusted link to a binary inside the world-writable directory
        // is refused too: the target's directory is what is checked.
        let trusted = tmp.path().join("trusted");
        std::fs::create_dir(&trusted).unwrap();
        std::fs::set_permissions(&trusted, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = trusted.join("git");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        assert!(resolve_strict(&[link]).is_err());
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Auditor: the directory the candidate lives in must be trusted too,
    /// not only the link target's. A world-writable `/usr/local/bin` lets
    /// another user point `docker` at any trusted binary (an interpreter
    /// that then reads a file named by the first argument from the
    /// inherited working directory). Tempting wrong patch (the head):
    /// checking the candidate directory only to pick the PATH head.
    #[test]
    fn audit_a_link_in_a_world_writable_candidate_dir_refuses() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exe = real.join("sh");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let open = tmp.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let link = open.join("docker");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let got = resolve_strict(&[link]);
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            got.is_err(),
            "accepted a link planted in a world-writable dir: {got:?}"
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
