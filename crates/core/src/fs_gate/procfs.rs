//! Linux occupancy from procfs (#86), unprivileged: for every process
//! running as this user, its `cwd`, `root` and `exe` links, every open
//! file descriptor, and every file it has mapped (`maps`) are compared
//! with the anchors an [`crate::occupancy`] probe was asked about.
//! Anything at or under an anchor is `Occupied`.
//!
//! Every read here is `std::fs`/`libc`, so the whole prober lives inside
//! the gate rather than only its primitive calls: unlike `fs_gate::sys`'s
//! one-shot `statfs`/`flock` probes, this is one cohesive capability
//! (walk `/proc`, compare credentials, follow links, read `maps`) that
//! [`crate::occupancy::probe_paths`] calls as a unit and never partially
//! reimplements outside the gate.
//!
//! **What it can see, and what it answers when it cannot.** An
//! unprivileged process can read the fd table of the processes that run
//! as its own user and no others; that is the same boundary `lsof` has
//! without root, on either platform. So the question answered is "does
//! any process running as you hold this" -- the evidence says so in its
//! coverage note -- and every way *that* question can go unanswered is
//! `Unknown`, never `Free`.

#![allow(unsafe_code)]

use crate::occupancy::OccupancyState;
use std::path::Path;
use std::time::{Duration, Instant};

/// A process's credentials as procfs states them: real, effective,
/// saved and filesystem uid and gid, and its permitted capability set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Creds {
    pub uids: [u32; 4],
    pub gids: [u32; 4],
    pub cap_prm: u64,
}

impl Creds {
    /// This process's own credentials.
    pub fn of_self() -> Creds {
        let text = std::fs::read_to_string("/proc/self/status");
        match text.as_deref().map(parse_status) {
            Ok(Some(c)) => c,
            // Without procfs there is nothing to compare against; the
            // probe itself then fails on `/proc/self` and says Unknown.
            _ => {
                // SAFETY: plain getters, no memory read.
                let (u, g) = unsafe { (libc::getuid(), libc::getgid()) };
                Creds {
                    uids: [u; 4],
                    gids: [g; 4],
                    cap_prm: 0,
                }
            }
        }
    }

    /// The kernel's own test for "this user may read that process"
    /// (`ptrace_may_access` with `PTRACE_MODE_READ`): every uid and gid
    /// equal, and no capability this process lacks. A process that
    /// fails it is another user's, or runs with more privilege than
    /// this user has -- outside what an unprivileged probe can see, on
    /// either platform, and so outside the question it answers. A
    /// process that passes it and still cannot be read is the gap that
    /// is `Unknown`.
    pub fn same_privilege_as(&self, me: &Creds) -> bool {
        self.uids.iter().all(|u| *u == me.uids[0])
            && self.gids.iter().all(|g| *g == me.gids[0])
            && self.cap_prm & !me.cap_prm == 0
    }
}

/// This process's pid, for the `/proc/self` PID-namespace check.
pub fn self_pid() -> u32 {
    // SAFETY: getpid cannot fail and reads no memory.
    unsafe { libc::getpid() as u32 }
}

/// `status`'s `Uid:`, `Gid:` and `CapPrm:` lines.
pub(crate) fn parse_status(text: &str) -> Option<Creds> {
    let four = |key: &str| -> Option<[u32; 4]> {
        let line = text.lines().find_map(|l| l.strip_prefix(key))?;
        let v: Vec<u32> = line
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
    };
    let cap_prm = text
        .lines()
        .find_map(|l| l.strip_prefix("CapPrm:"))
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
        .unwrap_or(0);
    Some(Creds {
        uids: four("Uid:")?,
        gids: four("Gid:")?,
        cap_prm,
    })
}

fn gone(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
}

/// One process's credentials, from `status`. `status` stays readable
/// for a non-dumpable process, whose other entries become root-owned.
/// `Ok(None)` when the process has exited. Under `hidepid=1` another
/// user's `status` is unreadable; its directory's owner still says it
/// is not ours.
fn process_creds(dir: &Path, my_uid: u32) -> Result<Option<Creds>, String> {
    use std::io::ErrorKind;
    match std::fs::read_to_string(dir.join("status")) {
        Ok(text) => match parse_status(&text) {
            Some(c) => Ok(Some(c)),
            None => Err("its status has no readable Uid/Gid lines".into()),
        },
        Err(e) if gone(&e) => Ok(None),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            use std::os::unix::fs::MetadataExt;
            match std::fs::symlink_metadata(dir) {
                Ok(m) if m.uid() != my_uid => Ok(Some(Creds {
                    uids: [m.uid(); 4],
                    gids: [m.gid(); 4],
                    cap_prm: 0,
                })),
                Ok(_) => Err(format!("its status cannot be read ({e})")),
                Err(e) if gone(&e) => Ok(None),
                Err(e) => Err(format!("cannot stat it ({e})")),
            }
        }
        Err(e) => Err(format!("cannot read its status ({e})")),
    }
}

fn process_name(dir: &Path) -> String {
    match std::fs::read_to_string(dir.join("comm")) {
        Ok(c) => c.trim().to_string(),
        Err(e) => format!("name unreadable: {e}"),
    }
}

/// Whether the kernel itself withholds a same-credential process from
/// this user: a process whose memory is marked non-dumpable (one that
/// changed credentials without an `exec`, like a PAM session holder, or
/// that asked for it with `prctl(PR_SET_DUMPABLE, 0)`) has its procfs
/// entries owned by root, and no unprivileged reader, `lsof` included,
/// can see its open files. That is the same boundary as a process with
/// more privilege than this user, and is treated the same way: outside
/// the question this probe answers, stated in the evidence's coverage.
///
/// A same-user process whose entries are still *this user's* and yet
/// cannot be read (an LSM denial, anything unexplained) is not that
/// boundary, and stays `Unknown`.
fn kernel_withholds(dir: &Path, my_uid: u32) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(dir.join("fd"))?;
    Ok(withheld_owner(m.uid(), my_uid))
}

/// The ownership rule `kernel_withholds` reads, pure so both platforms
/// test it.
pub(crate) fn withheld_owner(fd_dir_owner: u32, my_uid: u32) -> bool {
    fd_dir_owner != my_uid
}

/// Whether one process holds anything under an anchor: its `cwd`,
/// `root` and `exe` links, each fd, each mapped file. `Ok(None)` also
/// when the process exited while it was being read.
fn process_holds(
    dir: &Path,
    held: &dyn Fn(&Path) -> Option<std::path::PathBuf>,
) -> Result<Option<std::path::PathBuf>, String> {
    for link in ["cwd", "root", "exe"] {
        match std::fs::read_link(dir.join(link)) {
            Ok(t) => {
                if let Some(m) = held(&t) {
                    return Ok(Some(m));
                }
            }
            Err(e) if gone(&e) => return Ok(None),
            Err(e) => return Err(format!("its {link} link cannot be read ({e})")),
        }
    }
    let fds = match std::fs::read_dir(dir.join("fd")) {
        Ok(rd) => rd,
        Err(e) if gone(&e) => return Ok(None),
        Err(e) => return Err(format!("its open files cannot be listed ({e})")),
    };
    for fd in fds {
        let fd = match fd {
            Ok(f) => f,
            Err(e) if gone(&e) => return Ok(None),
            Err(e) => return Err(format!("its open files cannot be listed ({e})")),
        };
        match std::fs::read_link(fd.path()) {
            Ok(t) => {
                if let Some(m) = held(&t) {
                    return Ok(Some(m));
                }
            }
            // That descriptor closed since the listing.
            Err(e) if gone(&e) => {}
            Err(e) => {
                return Err(format!(
                    "its descriptor {:?} cannot be read ({e})",
                    fd.file_name()
                ));
            }
        }
    }
    match std::fs::read_to_string(dir.join("maps")) {
        Ok(maps) => {
            for line in maps.lines() {
                // address perms offset dev inode pathname
                let Some(path) = line.split_whitespace().nth(5) else {
                    continue;
                };
                if path.starts_with('/')
                    && let Some(m) = held(Path::new(path))
                {
                    return Ok(Some(m));
                }
            }
            Ok(None)
        }
        Err(e) if gone(&e) => Ok(None),
        Err(e) => Err(format!("its memory maps cannot be read ({e})")),
    }
}

/// Pure over `proc_root` so the fail-closed rules are testable against
/// a fixture tree on either platform; the real call passes `/proc`.
///
/// A process that exits mid-scan (`ENOENT`/`ESRCH` on its entries) is
/// skipped: it holds nothing any more. Processes the kernel does not
/// let this user read -- another user's, one with more privilege (a
/// capability this process lacks, a saved uid of root), or one marked
/// non-dumpable -- are outside the question, as they are for `lsof` run
/// without root; the evidence's coverage note says so.
pub fn probe(
    proc_root: &Path,
    anchors: &[&Path],
    me: &Creds,
    self_pid: u32,
    budget: Duration,
) -> OccupancyState {
    use std::io::ErrorKind;
    let started = Instant::now();

    let mut targets: Vec<std::path::PathBuf> = Vec::new();
    for a in anchors {
        match std::fs::canonicalize(a) {
            Ok(c) => targets.push(c),
            // Nothing there to hold open; the caller's identity recheck
            // is what refuses a vanished path.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return OccupancyState::Unknown(format!("cannot resolve {}: {e}", a.display()));
            }
        }
    }
    if targets.is_empty() {
        return OccupancyState::Free;
    }

    // The procfs must be this PID namespace's.
    match std::fs::read_link(proc_root.join("self")) {
        Ok(link) if link.to_str() == Some(self_pid.to_string().as_str()) => {}
        Ok(link) => {
            return OccupancyState::Unknown(format!(
                "{}/self is {} rather than this process ({self_pid}): the process table belongs \
                 to another PID namespace, so the processes sharing this filesystem are not the \
                 ones listed",
                proc_root.display(),
                link.display()
            ));
        }
        Err(e) => {
            return OccupancyState::Unknown(format!(
                "cannot read {}/self ({e}); procfs is not usable here",
                proc_root.display()
            ));
        }
    }

    let entries = match std::fs::read_dir(proc_root) {
        Ok(rd) => rd,
        Err(e) => {
            return OccupancyState::Unknown(format!("cannot list {} ({e})", proc_root.display()));
        }
    };
    let held = |p: &Path| -> Option<std::path::PathBuf> {
        let text = p.to_string_lossy();
        let text = text.strip_suffix(" (deleted)").unwrap_or(&text);
        let p = Path::new(text);
        targets
            .iter()
            .find(|t| p.starts_with(t))
            .map(|_| p.to_path_buf())
    };

    for entry in entries {
        if started.elapsed() > budget {
            return OccupancyState::Unknown(format!(
                "the procfs scan did not finish within {}s",
                budget.as_secs()
            ));
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                return OccupancyState::Unknown(format!(
                    "listing {} failed part-way ({e})",
                    proc_root.display()
                ));
            }
        };
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let dir = entry.path();
        let creds = match process_creds(&dir, me.uids[0]) {
            Ok(Some(c)) => c,
            // Exited between the listing and now.
            Ok(None) => continue,
            Err(why) => return OccupancyState::Unknown(format!("process {pid}: {why}")),
        };
        if !creds.same_privilege_as(me) {
            // Another user's process, or one running with more
            // privilege than this user (a setuid program, a process
            // holding capabilities): the kernel does not let this user
            // read it, and it is outside the question (see the doc
            // comment).
            continue;
        }
        match process_holds(&dir, &held) {
            Ok(Some(member)) => return OccupancyState::Occupied(member),
            Ok(None) => {}
            Err(why) => match kernel_withholds(&dir, me.uids[0]) {
                // The kernel marks the process non-dumpable (its
                // procfs entries turn root-owned): no unprivileged tool
                // may read it, the same boundary as a more privileged
                // process.
                Ok(true) => continue,
                Ok(false) => {
                    return OccupancyState::Unknown(format!(
                        "process {pid} ({}) runs as this user but {why}, so whether it holds \
                         anything under the selection is not known",
                        process_name(&dir)
                    ));
                }
                Err(e) if gone(&e) => continue,
                Err(e) => {
                    return OccupancyState::Unknown(format!(
                        "process {pid}: {why}, and its procfs entry cannot be examined ({e})"
                    ));
                }
            },
        }
    }
    OccupancyState::Free
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withheld_owner_is_pure() {
        assert!(withheld_owner(0, 501));
        assert!(!withheld_owner(501, 501));
    }

    #[test]
    fn parse_status_reads_uid_gid_and_caps() {
        let text = "Name:\tfoo\nUid:\t501\t501\t501\t501\nGid:\t20\t20\t20\t20\nCapPrm:\t0000000000000000\n";
        let c = parse_status(text).expect("parses");
        assert_eq!(c.uids, [501; 4]);
        assert_eq!(c.gids, [20; 4]);
        assert_eq!(c.cap_prm, 0);
    }
}
