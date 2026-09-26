//! What each operating system can and cannot do for swamp, as data.
//!
//! Before this module, "macOS-only" was a property you discovered by
//! reading `#[cfg]` attributes scattered across `fs_events.rs`,
//! `schedule.rs`, `actions.rs` and `occupancy.rs`, and a Linux build
//! answered several questions by quietly doing the macOS thing badly --
//! writing a LaunchAgent plist into a `~/Library/LaunchAgents` that does
//! not exist, parsing `df` output whose columns differ, resolving a
//! `~/.Trash` no desktop environment looks in.
//!
//! The contracts here are **data, not `cfg`**, for the same reason
//! [`crate::locations::Platform`] is: a test on either target can ask
//! "what does a Linux build promise about scheduling?" and get a real
//! answer, so a macOS regression in the Linux table (or the reverse) is
//! caught by `cargo test` on one machine rather than by a user on the
//! other. Only [`Os::current`] is `cfg`-gated, and only the *backends*
//! -- FSEvents replay, launchd -- are compiled conditionally.
//!
//! Three rules hold throughout:
//!
//! 1. **A capability swamp does not have is named, never approximated.**
//!    [`Scheduling::Unavailable`] carries the reason and the issue that
//!    would add it; it never degrades into "write the file anyway".
//! 2. **Continuity is source-specific.** An FSEvents event id is a cursor
//!    into a log the kernel kept while swamp was not running. An inotify
//!    watch descriptor is a handle on a watch that is running *now*.
//!    [`ContinuitySource`] keeps them apart, because storing the second
//!    where the first belongs would turn "swamp was not watching" into
//!    "nothing changed" -- the exact falsehood the growth store must
//!    never record.
//! 3. **Shared Unix stays shared.** Allocated bytes (`st_blocks * 512`),
//!    device/inode identity, and the free-space contract are POSIX and
//!    live in [`fs_space`] and `crate::walk` for both targets (only the
//!    free-space syscall differs: `statfs` on macOS, `statvfs` on Linux). Target
//!    gating is for genuinely different kernels, not for filing code by
//!    operating system.
//!
//! The capability table in [`CAPABILITIES`] is checked against
//! `docs/platform.md` by `crates/core/tests/platform_matrix_matches_docs.rs`,
//! so the documented answer and the compiled one cannot drift.

/// Free-space measurement: the syscalls themselves live in
/// [`crate::fs_gate::fs_space`] (the gate names `libc`, not this
/// module); re-exported here under its original path since this is
/// where every caller and the module doc above already point.
pub use crate::fs_gate::fs_space;

use crate::locations::Platform;
use std::path::PathBuf;

// ---------------------------------------------------------------------
// Where swamp keeps its own state
// ---------------------------------------------------------------------

/// `${SWAMP_DIR}`, or the platform's per-user data directory.
///
/// `~/.local/share/swamp` on macOS -- the path swamp has always used
/// there, kept unchanged deliberately: an existing install's growth
/// history lives at that path, and this work moves no user data. On
/// Linux the same path, except that `$XDG_DATA_HOME` is honoured when it
/// is set to an absolute path, because on Linux that variable is the
/// documented way to say where per-user data goes and a user who has set
/// it means it.
///
/// **`Err`, never the current directory.** The old resolution fell back
/// to `.` when `HOME` was unset, which silently scattered a growth store
/// into whatever directory swamp was run from -- and made the *next*
/// run, from a different directory, look like every project had
/// vanished. A missing home is a question for the user, not a default.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("SWAMP_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        anyhow::anyhow!(
            "HOME is not set, so there is no per-user directory to keep the growth store in. \
             Set SWAMP_DIR to an explicit path; swamp will not write it into the current \
             directory, where the next run from somewhere else would not find it."
        )
    })?;
    Ok(match Os::current() {
        Os::MacOs => home.join(".local/share/swamp"),
        Os::Linux => match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
            // The XDG base directory spec: a relative value is invalid
            // and must be ignored.
            Some(p) if p.is_absolute() => p.join("swamp"),
            _ => home.join(".local/share/swamp"),
        },
    })
}

/// The operating system a build's backends were compiled for.
///
/// Distinct from [`crate::locations::Platform`], which is a *detection*
/// input a fixture may set to anything. This is what the binary can
/// actually do, and only [`Os::current`] may produce it from the real
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Linux,
}

impl Os {
    /// The OS this build was compiled for. `cfg`-gated so a Linux build
    /// does not even contain the macOS answer.
    #[cfg(target_os = "macos")]
    pub fn current() -> Self {
        Os::MacOs
    }

    #[cfg(target_os = "linux")]
    pub fn current() -> Self {
        Os::Linux
    }

    /// No backend set exists for any other target. Returning an answer
    /// here would be a claim this build cannot keep, so the supported
    /// targets are the only ones that compile.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn current() -> Self {
        compile_error!(
            "swamp supports x86_64-unknown-linux-gnu and aarch64-apple-darwin; \
             see docs/platform.md before adding a target"
        )
    }

    /// The detection-side platform this OS corresponds to. The one
    /// bridge between "what this build can do" and "which conventional
    /// paths a detector should propose".
    pub fn platform(self) -> Platform {
        match self {
            Os::MacOs => Platform::MacOS,
            Os::Linux => Platform::Linux,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Os::MacOs => "macos",
            Os::Linux => "linux",
        }
    }
}

impl From<Platform> for Os {
    fn from(p: Platform) -> Self {
        match p {
            Platform::MacOS => Os::MacOs,
            Platform::Linux => Os::Linux,
        }
    }
}

// ---------------------------------------------------------------------
// Change observation and continuity
// ---------------------------------------------------------------------

/// Where a build gets "what changed since last time" from, and -- more
/// importantly -- what that source can and cannot answer *about a period
/// when swamp was not running*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuitySource {
    /// macOS. `fseventsd` keeps a per-volume log on disk. A stored event
    /// id is a cursor *into history*: a replay from it accounts for
    /// changes that happened while no swamp process existed.
    FsEventsPersistedLog,
    /// Linux. inotify delivers events to a *running* watch and keeps no
    /// history; `IN_Q_OVERFLOW` is the kernel telling you it dropped
    /// some of even those. So the only honest cursor is the instant a
    /// watch started, and anything before it is unaccounted for.
    LiveWatchEpochOnly,
}

impl ContinuitySource {
    pub fn for_os(os: Os) -> Self {
        match os {
            Os::MacOs => ContinuitySource::FsEventsPersistedLog,
            Os::Linux => ContinuitySource::LiveWatchEpochOnly,
        }
    }

    /// True only where a stored cursor can account for time the process
    /// was not running. Every incremental-refresh decision keys off this,
    /// not off the target triple.
    pub fn replays_history(self) -> bool {
        matches!(self, ContinuitySource::FsEventsPersistedLog)
    }

    /// Why an incremental refresh is unavailable on this source, in the
    /// words that go into `mode=full reason=...` and the coverage note.
    /// `None` where history *is* available and a refusal would have some
    /// other cause.
    pub fn no_history_reason(self) -> Option<&'static str> {
        match self {
            ContinuitySource::FsEventsPersistedLog => None,
            ContinuitySource::LiveWatchEpochOnly => Some(
                "no persisted kernel change history on this platform: inotify reports only what \
                 happens while a watch is open, so a period with no running watch is a gap. A \
                 full walk covers it; a live-watch epoch covers the time since the watch opened.",
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ContinuitySource::FsEventsPersistedLog => "fsevents_persisted_log",
            ContinuitySource::LiveWatchEpochOnly => "live_watch_epoch_only",
        }
    }
}

/// A persisted continuity cursor, tagged with the source that produced
/// it. The tag is the point: a cursor from one source is meaningless to
/// the other, and storing an inotify watch descriptor in the slot an
/// FSEvents event id occupies would silently turn "I was not watching"
/// into "nothing changed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuityCursor {
    /// An `fseventsd` event id plus the device it was recorded against.
    /// Valid for time *before* it was recorded.
    FsEventsEventId { event_id: u64, device: u64 },
    /// The wall-clock second a live watch opened, plus the device. Valid
    /// only for time *after* it: nothing before `opened_at` is covered.
    ///
    /// Deliberately not constructible from a watch descriptor. A watch
    /// descriptor identifies a watch; it says nothing about when the
    /// watch began or what happened before it.
    LiveWatchEpoch { opened_at: u64, device: u64 },
}

impl ContinuityCursor {
    pub fn source(self) -> ContinuitySource {
        match self {
            ContinuityCursor::FsEventsEventId { .. } => ContinuitySource::FsEventsPersistedLog,
            ContinuityCursor::LiveWatchEpoch { .. } => ContinuitySource::LiveWatchEpochOnly,
        }
    }

    pub fn device(self) -> u64 {
        match self {
            ContinuityCursor::FsEventsEventId { device, .. }
            | ContinuityCursor::LiveWatchEpoch {
                opened_at: _,
                device,
            } => device,
        }
    }

    /// Does this cursor account for everything that happened since
    /// `since` (a wall-clock second)?
    ///
    /// An FSEvents id does, by construction: the log it points into was
    /// being written whether or not swamp was running. A live-watch
    /// epoch does **only** when the watch was already open at `since`;
    /// otherwise there is a gap and the answer is no. This is the one
    /// method that must never be "optimized" into `true`.
    pub fn covers_since(self, since: u64) -> bool {
        match self {
            ContinuityCursor::FsEventsEventId { .. } => true,
            ContinuityCursor::LiveWatchEpoch { opened_at, .. } => opened_at <= since,
        }
    }
}

// ---------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------

/// How (or whether) a build can install an unattended periodic
/// observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduling {
    /// macOS: a per-user LaunchAgent plist driven by `launchctl`. Opt-in,
    /// no resident process.
    LaunchdUserAgent,
    /// Linux (#83): a `systemd --user` timer + oneshot service, opt-in,
    /// and an optional collector service. Whether a user manager is
    /// actually reachable is a *runtime* question, answered by
    /// `systemd_user::user_manager` before anything is written; this
    /// variant only says which backend the build has.
    SystemdUser,
    /// No backend. Naming the reason and the issue is the whole contract
    /// -- an unavailable scheduler must refuse, not write a job file into
    /// a directory no daemon reads. No supported build constructs it
    /// today; it stays so a new target starts from a refusal.
    Unavailable {
        reason: &'static str,
        planned: &'static str,
    },
}

impl Scheduling {
    pub fn for_os(os: Os) -> Self {
        match os {
            Os::MacOs => Scheduling::LaunchdUserAgent,
            Os::Linux => Scheduling::SystemdUser,
        }
    }

    /// The message a scheduling command prints when it cannot proceed.
    /// `None` where the build has a backend (which may still refuse at
    /// runtime, with its own diagnostics).
    pub fn refusal(self) -> Option<String> {
        match self {
            Scheduling::LaunchdUserAgent | Scheduling::SystemdUser => None,
            Scheduling::Unavailable { reason, planned } => Some(format!(
                "scheduled observation is not available on this platform: {reason}. Planned in \
                 {planned}. Run `swamp observe` from your own timer until then; nothing has been \
                 installed."
            )),
        }
    }
}

// ---------------------------------------------------------------------
// Occupancy
// ---------------------------------------------------------------------

/// How a build answers "does anything have this path open right now".
///
/// The distinction that matters is not which mechanism but that an
/// unanswerable question is [`crate::occupancy::OccupancyState::Unknown`],
/// never "free" -- which is enforced at the sinks, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccupancyProbe {
    /// macOS: `lsof +D` for directories, `lsof --` for files, bounded.
    Lsof,
    /// Linux: procfs, read directly and unprivileged (#86) -- `cwd`,
    /// `root`, `exe`, every fd and every mapped file of each process
    /// running as this user. No `lsof` dependency, which a minimal
    /// Ubuntu install does not have; where one is installed it is a
    /// second reader that can only make the answer stricter.
    Procfs,
}

impl OccupancyProbe {
    pub fn for_os(os: Os) -> Self {
        match os {
            Os::MacOs => OccupancyProbe::Lsof,
            Os::Linux => OccupancyProbe::Procfs,
        }
    }
}

// ---------------------------------------------------------------------
// The capability table
// ---------------------------------------------------------------------

/// What a capability is on one OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Implemented and exercised by tests on that OS.
    Supported,
    /// Deliberately absent. The build refuses with a named reason rather
    /// than approximating it.
    Unavailable,
    /// Absent today, with an issue that adds it. Still refuses now.
    Planned,
}

impl Support {
    pub fn as_str(self) -> &'static str {
        match self {
            Support::Supported => "supported",
            Support::Unavailable => "unavailable",
            Support::Planned => "planned",
        }
    }
}

/// One row of the platform capability matrix.
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// Stable id. Matches the first column of the table in
    /// `docs/platform.md`.
    pub id: &'static str,
    pub macos: Support,
    pub linux: Support,
    /// One sentence a user can act on, not an implementation note.
    pub note: &'static str,
}

impl Capability {
    pub fn support(&self, os: Os) -> Support {
        match os {
            Os::MacOs => self.macos,
            Os::Linux => self.linux,
        }
    }
}

/// The single source of truth for "what works where".
///
/// `docs/platform.md` renders this table; a test asserts the two agree,
/// so a capability cannot be claimed in prose without being claimed here
/// (and vice versa).
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        id: "walk-and-accounting",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "Allocated bytes from st_blocks*512, hardlink dedup by (dev, ino), \
               same-filesystem boundary by st_dev, symlinks never followed. Shared POSIX code.",
    },
    Capability {
        id: "free-space",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "A syscall through libc, replacing df output parsing whose columns differ \
               between the two: statfs(2) on macOS, whose statvfs has 32-bit block counts, and \
               statvfs(3) on Linux.",
    },
    Capability {
        id: "history-replay",
        macos: Support::Supported,
        linux: Support::Unavailable,
        note: "macOS replays the fseventsd log from a stored event id. Linux has no persisted \
               kernel change history; an observation there walks fully and says so, unless a \
               running collector can vouch for the gap.",
    },
    Capability {
        id: "live-watch",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "macOS opens an FSEvents stream for the TUI. Linux registers unprivileged inotify \
               watches per directory and names every loss (queue overflow, watch limit, \
               permissions, unmount); a loss makes the next refresh a full walk.",
    },
    Capability {
        id: "background-collection",
        macos: Support::Unavailable,
        linux: Support::Supported,
        note: "Linux: opt-in swamp collect keeps a bounded change list that later observations \
               reuse only while it runs, in the same boot, with coverage intact. macOS needs \
               none: FSEvents keeps the history.",
    },
    Capability {
        id: "scheduled-observation",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "macOS installs an opt-in per-user LaunchAgent. Linux installs systemd --user \
               units (timer, optional collector); where no user manager is reachable it \
               refuses and writes nothing, and it never enables lingering.",
    },
    Capability {
        id: "trash",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "macOS renames into ~/.Trash. Linux follows the freedesktop Trash spec (home \
               trash, or the mount's own .Trash-$uid) with a .trashinfo record; a rename or a \
               refusal, never a copy or a permanent fallback.",
    },
    Capability {
        id: "occupancy",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "macOS runs a bounded lsof; Linux reads procfs (fds, cwd, exe, maps) of this \
               user's processes. Anything that cannot be read is Unknown, which every \
               destructive sink refuses on -- never 'nothing is open'.",
    },
    Capability {
        id: "atime-reliability",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "macOS reads statfs mount flags; Linux reads /proc/mounts. Either failing is \
               Undetermined, not 'atime is fine'.",
    },
    Capability {
        id: "release-artifact",
        macos: Support::Supported,
        linux: Support::Supported,
        note: "Separate archives with checksums per target (binary, README, skill), each built \
               and tested on its own runner; a release publishes only after both targets and a \
               newer-Ubuntu test pass.",
    },
];

/// Look one capability up by id.
pub fn capability(id: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_continuity_never_claims_history() {
        let linux = ContinuitySource::for_os(Os::Linux);
        assert!(!linux.replays_history());
        assert!(
            linux.no_history_reason().is_some(),
            "a source that cannot replay must name why, not refuse anonymously"
        );
        assert!(ContinuitySource::for_os(Os::MacOs).replays_history());
        assert!(
            ContinuitySource::for_os(Os::MacOs)
                .no_history_reason()
                .is_none()
        );
    }

    #[test]
    fn a_live_watch_epoch_does_not_cover_time_before_the_watch_opened() {
        let cursor = ContinuityCursor::LiveWatchEpoch {
            opened_at: 1_000,
            device: 7,
        };
        assert!(
            !cursor.covers_since(999),
            "a watch opened at t=1000 cannot account for what happened at t=999"
        );
        assert!(cursor.covers_since(1_000));
        assert!(cursor.covers_since(1_001));
        assert_eq!(cursor.source(), ContinuitySource::LiveWatchEpochOnly);
    }

    #[test]
    fn an_fsevents_cursor_covers_the_time_the_process_was_not_running() {
        let cursor = ContinuityCursor::FsEventsEventId {
            event_id: 42,
            device: 7,
        };
        assert!(cursor.covers_since(0));
        assert_eq!(cursor.source(), ContinuitySource::FsEventsPersistedLog);
    }

    #[test]
    fn each_platform_has_its_own_scheduler_and_neither_the_others() {
        assert_eq!(Scheduling::for_os(Os::Linux), Scheduling::SystemdUser);
        assert_eq!(Scheduling::for_os(Os::MacOs), Scheduling::LaunchdUserAgent);
        assert!(Scheduling::for_os(Os::Linux).refusal().is_none());
        let none = Scheduling::Unavailable {
            reason: "no backend",
            planned: "#0",
        };
        let refusal = none
            .refusal()
            .expect("an unavailable scheduler explains itself");
        assert!(refusal.contains("nothing has been installed"), "{refusal}");
    }

    #[test]
    fn every_capability_id_is_unique_and_documented() {
        let mut ids: Vec<&str> = CAPABILITIES.iter().map(|c| c.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate capability id");
        for c in CAPABILITIES {
            assert!(!c.note.is_empty(), "{} has no note", c.id);
            assert!(capability(c.id).is_some());
        }
    }

    /// Env vars are process-global, so the resolution tests take the
    /// crate-wide lock -- the same one `schedule`'s tests take, because
    /// one of the tests below unsets `HOME` and `schedule::home()` reads
    /// it. Every test restores what it changed.
    use crate::TEST_ENV_LOCK as ENV_LOCK;

    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn new(vars: &[&'static str]) -> Self {
            let saved = vars
                .iter()
                .map(|v| (*v, std::env::var_os(v)))
                .collect::<Vec<_>>();
            for v in vars {
                // SAFETY: serialized by ENV_LOCK; restored on drop.
                unsafe { std::env::remove_var(v) };
            }
            Self { saved }
        }
        fn set(&self, key: &str, value: &str) {
            // SAFETY: as above.
            unsafe { std::env::set_var(key, value) };
        }
        fn unset(&self, key: &str) {
            // SAFETY: as above.
            unsafe { std::env::remove_var(key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                // SAFETY: as above.
                unsafe {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    #[test]
    fn swamp_dir_overrides_every_convention() {
        let _guard = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(&["SWAMP_DIR", "HOME", "XDG_DATA_HOME"]);
        env.set("SWAMP_DIR", "/explicit/store");
        env.set("HOME", "/home/dev");
        env.set("XDG_DATA_HOME", "/data/dev");
        assert_eq!(data_dir().unwrap(), PathBuf::from("/explicit/store"));
    }

    /// macOS keeps `~/.local/share/swamp` unchanged -- an existing
    /// install's history lives there and this work moves no user data --
    /// and does not start honouring `$XDG_DATA_HOME`, which is not a
    /// macOS convention. Linux does honour it, because there it is the
    /// documented way to say where per-user data goes.
    #[test]
    fn the_store_directory_follows_this_platforms_convention() {
        let _guard = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(&["SWAMP_DIR", "HOME", "XDG_DATA_HOME"]);
        env.set("HOME", "/home/dev");

        assert_eq!(
            data_dir().unwrap(),
            PathBuf::from("/home/dev/.local/share/swamp"),
            "both platforms default to the same path"
        );

        env.set("XDG_DATA_HOME", "/data/dev");
        match Os::current() {
            Os::Linux => assert_eq!(data_dir().unwrap(), PathBuf::from("/data/dev/swamp")),
            Os::MacOs => assert_eq!(
                data_dir().unwrap(),
                PathBuf::from("/home/dev/.local/share/swamp"),
                "XDG_DATA_HOME is not a macOS convention and must not move an existing store"
            ),
        }
    }

    /// The XDG base directory spec: a relative value is invalid and must
    /// be ignored. Honouring one would put the growth store somewhere
    /// relative to the process's working directory.
    #[test]
    fn a_relative_xdg_data_home_is_ignored() {
        let _guard = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(&["SWAMP_DIR", "HOME", "XDG_DATA_HOME"]);
        env.set("HOME", "/home/dev");
        env.set("XDG_DATA_HOME", "relative/share");
        let dir = data_dir().unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert_eq!(dir, PathBuf::from("/home/dev/.local/share/swamp"));
    }

    /// The old resolution fell back to `.`, which scattered a growth
    /// store into whatever directory swamp was run from -- and made the
    /// next run, from somewhere else, look like every project had
    /// vanished. A missing home is a question for the user.
    #[test]
    fn no_home_and_no_override_is_an_error_not_the_current_directory() {
        let _guard = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(&["SWAMP_DIR", "HOME", "XDG_DATA_HOME"]);
        env.unset("HOME");
        let err = data_dir().expect_err("a missing HOME must not resolve to the cwd");
        let message = format!("{err}");
        assert!(message.contains("SWAMP_DIR"), "{message}");
        assert!(
            message.contains("current") && message.contains("directory"),
            "the error must say what it refused to do: {message}"
        );
    }

    #[test]
    fn os_and_detection_platform_agree() {
        assert_eq!(Os::MacOs.platform(), Platform::MacOS);
        assert_eq!(Os::Linux.platform(), Platform::Linux);
        assert_eq!(Os::from(Platform::Linux), Os::Linux);
        assert_eq!(Os::current().platform(), Platform::current());
    }
}
