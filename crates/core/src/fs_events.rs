//! In-process FSEvents replay: "which directories under a root changed
//! since event id N?", answered without a helper process.
//!
//! Ports the design from mole `integrate` (`cmd/fswatch`,
//! `internal/fswatch`, `docs/fsevents-helper.md`), collapsed into one
//! crate instead of a separate cgo binary: this crate is already macOS
//! product code (unlike Mole's pure-Go release binaries), so there is no
//! `CGO_ENABLED=0` constraint to protect by keeping FSEvents in a
//! sidecar. `fsevent-sys` gives raw CoreServices bindings; [`macos::replay`]
//! is the small safe wrapper around them.
//!
//! The client never treats a failed replay as an error: [`FsEventsSource::replay`]
//! always returns a [`FsEventsPlan`], and every way a replay can fail to
//! earn an incremental refresh is a named [`RefreshRefusal`] instead of an
//! `Err`. A refusal is not a bug; it is this design refusing to guess.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Every way a replay can fail to earn an incremental refresh. Two rules
/// hold across all of them: a refusal never carries a usable change list,
/// and the caller always falls back to a full walk.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RefreshRefusal {
    /// First observation of this root, or a stored id this build has never
    /// written: nothing to replay from.
    NoStoredEventId,
    /// The stored id is ahead of the stream's current id -- the volume's
    /// FSEvents history was reset and everything in between is
    /// unaccounted for.
    EventIdFromFuture,
    /// The root does not canonicalize to itself, or the root's device
    /// differs from the device the stored id was recorded against: the
    /// stored id addresses a different filesystem than the one being
    /// asked about.
    RootMismatch,
    /// No FSEvents history at all is available for this volume (no
    /// current event id, stream creation failed, or the volume is not
    /// journaled).
    FseventsdUnavailable,
    /// The replay could not be completed as a confident, complete account
    /// of what changed: `MustScanSubDirs`, `UserDropped`/`KernelDropped`,
    /// `EventIdsWrapped`, or the run loop budget expired before FSEvents
    /// reported `HistoryDone`. A prefix of the changes is not an answer.
    HelperInconclusive,
    /// The changed-directory count exceeds the configured fraction of
    /// previously known directories: re-walking that much of the tree
    /// piecemeal costs more than a full walk would.
    TooManyChanges,
    /// The previous observation of this root is too recent (within the
    /// same coarse timestamp granularity the growth store uses) for a
    /// replay to be meaningful: FSEvents' own persisted log can lag a
    /// write by longer than that gap, so a replay this soon after the
    /// baseline cannot yet distinguish "nothing changed" from "the
    /// change has not been logged yet". Decided by the caller
    /// (`growth::observe_tracked`), not by [`FsEventsSource::replay`]
    /// itself.
    TooSoon,
    /// This build has no FSEvents implementation for the current
    /// platform (non-macOS). Always refuses; there is no fallback stream
    /// to try.
    UnsupportedPlatform,
    /// This platform's kernel keeps no persisted change history to
    /// replay from. Linux: inotify reports only what happens while a
    /// watch is open, so a period with no running watch is a gap, not a
    /// quiet period. Distinct from [`RefreshRefusal::UnsupportedPlatform`]
    /// (which says a backend is missing) because this one says the
    /// backend cannot exist: no amount of implementation work makes a
    /// Linux kernel able to answer "what changed while you were not
    /// running". #81 adds the live watch, which narrows the gap to the
    /// time before the watch opened; it does not remove it.
    NoPersistedChangeHistory,
    /// Linux live watch: the kernel's inotify queue overflowed
    /// (`IN_Q_OVERFLOW`) and dropped events. Nothing since the watch's
    /// epoch opened can be vouched for.
    WatchQueueOverflow,
    /// Linux live watch: `inotify_add_watch` ran out of watches
    /// (`fs.inotify.max_user_watches`), so part of the root is unwatched.
    WatchLimitReached,
    /// Linux live watch: a directory under the root could not be watched
    /// or listed.
    WatchPermissionGap,
    /// Linux live watch: a watched filesystem was unmounted.
    WatchedFilesystemUnmounted,
    /// Linux live watch: the kernel removed a watch swamp did not ask it
    /// to remove.
    WatchRemoved,
    /// Linux: a live watch (TUI or `swamp collect`) is running, but the
    /// stored observation of this root predates its epoch -- the gap
    /// between the two is not covered by anything.
    LiveWatchGap,
    /// Linux: a collector checkpoint exists for this root, but its
    /// collector is not running (it exited, was killed, or the machine
    /// rebooted), so the checkpoint says nothing about what happened
    /// since.
    CollectorStopped,
    /// Linux: the collector did not confirm it had drained its event
    /// queue in time, so its checkpoint might miss a change that has
    /// already happened.
    CollectorUnresponsive,
    /// Linux: the running collector's scope (root identity or
    /// exclusions) is not the scope this observation walks.
    ScopeChanged,
}

impl RefreshRefusal {
    /// Stable, lowercase snake_case reason code used in report coverage
    /// notes and the `observe` log line (`mode=full reason=<this>`).
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshRefusal::NoStoredEventId => "no_stored_event_id",
            RefreshRefusal::EventIdFromFuture => "event_id_from_future",
            RefreshRefusal::RootMismatch => "root_mismatch",
            RefreshRefusal::FseventsdUnavailable => "fseventsd_unavailable",
            RefreshRefusal::HelperInconclusive => "helper_inconclusive",
            RefreshRefusal::TooManyChanges => "too_many_changes",
            RefreshRefusal::TooSoon => "too_soon",
            RefreshRefusal::UnsupportedPlatform => "unsupported_platform",
            RefreshRefusal::NoPersistedChangeHistory => "no_persisted_change_history",
            RefreshRefusal::WatchQueueOverflow => "watch_queue_overflow",
            RefreshRefusal::WatchLimitReached => "watch_limit_reached",
            RefreshRefusal::WatchPermissionGap => "watch_permission_gap",
            RefreshRefusal::WatchedFilesystemUnmounted => "watched_filesystem_unmounted",
            RefreshRefusal::WatchRemoved => "watch_removed",
            RefreshRefusal::LiveWatchGap => "live_watch_gap",
            RefreshRefusal::CollectorStopped => "collector_stopped",
            RefreshRefusal::CollectorUnresponsive => "collector_unresponsive",
            RefreshRefusal::ScopeChanged => "scope_changed",
        }
    }

    /// Every variant, for the tests that hold the vocabulary to the
    /// docs.
    pub const ALL: &'static [RefreshRefusal] = &[
        RefreshRefusal::NoStoredEventId,
        RefreshRefusal::EventIdFromFuture,
        RefreshRefusal::RootMismatch,
        RefreshRefusal::FseventsdUnavailable,
        RefreshRefusal::HelperInconclusive,
        RefreshRefusal::TooManyChanges,
        RefreshRefusal::TooSoon,
        RefreshRefusal::UnsupportedPlatform,
        RefreshRefusal::NoPersistedChangeHistory,
        RefreshRefusal::WatchQueueOverflow,
        RefreshRefusal::WatchLimitReached,
        RefreshRefusal::WatchPermissionGap,
        RefreshRefusal::WatchedFilesystemUnmounted,
        RefreshRefusal::WatchRemoved,
        RefreshRefusal::LiveWatchGap,
        RefreshRefusal::CollectorStopped,
        RefreshRefusal::CollectorUnresponsive,
        RefreshRefusal::ScopeChanged,
    ];

    /// The sentence a coverage note or `--json` explanation carries
    /// beside the code. A reason code tells a script what happened; this
    /// tells a person why a Linux run walks fully every time and that it
    /// is the platform, not a misconfiguration.
    pub fn explanation(self) -> Option<&'static str> {
        match self {
            RefreshRefusal::NoPersistedChangeHistory => {
                crate::platform::ContinuitySource::LiveWatchEpochOnly.no_history_reason()
            }
            RefreshRefusal::WatchQueueOverflow => Some(
                "the kernel's inotify queue overflowed and dropped events, so the watch cannot \
                 say what changed; a full walk re-establishes the baseline and a new watch epoch \
                 starts from it",
            ),
            RefreshRefusal::WatchLimitReached => Some(
                "part of the root has no inotify watch because fs.inotify.max_user_watches was \
                 reached; raise it (sysctl, as root) or narrow the scope. Until then every \
                 observation of this root walks fully",
            ),
            RefreshRefusal::WatchPermissionGap => Some(
                "a directory under the root could not be watched or listed, so changes there \
                 are invisible to the watch; every observation of this root walks fully",
            ),
            RefreshRefusal::WatchedFilesystemUnmounted => {
                Some("a watched filesystem was unmounted; the watch no longer covers it")
            }
            RefreshRefusal::WatchRemoved => Some(
                "the kernel removed a watch swamp did not ask it to remove, so part of the root \
                 went unwatched",
            ),
            RefreshRefusal::LiveWatchGap => Some(
                "a live watch is running, but the last observation of this root happened before \
                 it started; the time in between is covered by nothing, so this walk is full \
                 and the next one can use the watch",
            ),
            RefreshRefusal::CollectorStopped => Some(
                "the collector that kept this root's change list is not running (it exited, was \
                 stopped, or the machine restarted), so its list says nothing about what changed \
                 since; `swamp collect` (or `swamp schedule --collector`) starts one",
            ),
            RefreshRefusal::CollectorUnresponsive => Some(
                "the collector did not confirm it had read every pending event in time, so its \
                 change list might be missing one that already happened",
            ),
            RefreshRefusal::ScopeChanged => Some(
                "the running collector watches a different scope (root identity or exclusions) \
                 than this observation walks; restart it with the current scope",
            ),
            _ => None,
        }
    }
}

/// The two scalars a caller persists alongside whatever it publishes, and
/// passes back in on the next call. Mirrors mole's `fswatch.Request`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FsEventsState {
    /// The `current_event_id` from the plan that accompanied the last
    /// observation. `None` means "nothing stored", always a full refresh.
    pub event_id: Option<u64>,
    /// The device (`st_dev`) the root lived on when `event_id` was
    /// recorded. A root that now resolves to a different device makes the
    /// stored id meaningless.
    pub device: Option<u64>,
    /// `now()` (whole seconds) as of the observation that recorded
    /// `event_id`. See [`RefreshRefusal::TooSoon`].
    pub last_observed_at: Option<u64>,
    /// `ecosystem::RULES_VERSION` the stored rows were classified under.
    /// A different current version forces a full walk (missing = 0).
    #[serde(default)]
    pub rules_version: u32,
    /// This path's anchor as a *measured unit root* -- an authorized
    /// detector-resolved external cache or agent tool home
    /// (`crate::growth::replay_unit_roots`).
    ///
    /// It is a separate anchor from the three scalars above, in the same
    /// control file, because the two are advanced by different things at
    /// different times: the scalars by the folded walk of a scan root,
    /// this by the pass that measured the unit family. A path can be
    /// both (a tool home inside the configured scope), which is exactly
    /// why neither writer may rewrite the whole file -- both do a
    /// read-modify-write of their own half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_root: Option<UnitRootCursor>,
}

/// One authorized unit root's replay anchor.
///
/// Deliberately not just a second [`FsEventsState`]: a unit root has no
/// `rules_version` of its own (nothing about it is classified by the
/// ecosystem rules) and reusing the same struct would invite a writer to
/// copy the walk's scalars into it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnitRootCursor {
    /// The `current_event_id` from the replay that accompanied the last
    /// measurement of this root.
    pub event_id: Option<u64>,
    /// The device the root lived on then. A root that now resolves to a
    /// different device makes the stored id meaningless.
    pub device: Option<u64>,
    /// `now()` (whole seconds) as of the pass that recorded `event_id`.
    /// This is the instant the next window opens from, and the floor
    /// [`EventCoverage::unchanged_since`] compares stored rows against.
    pub observed_at: Option<u64>,
}

/// One replay request: a canonical root and the state persisted from the
/// last observation of it.
#[derive(Debug, Clone)]
pub struct FsEventsRequest {
    /// Must already be canonical (absolute, symlinks resolved). FSEvents
    /// answers in canonical paths; a non-canonical root would make every
    /// path in the reply appear to sit outside the tree the caller thinks
    /// it asked about.
    pub root: PathBuf,
    pub since: FsEventsState,
    /// The store this observation writes to. A live-watch source finds a
    /// collector's checkpoint there; FSEvents ignores it.
    pub swamp_dir: Option<PathBuf>,
    /// What this observation excludes under `root`. A live-watch source
    /// refuses when its own watch excludes something this walks.
    pub excluded: Vec<PathBuf>,
}

/// The verdict: either a complete, incremental account of what changed,
/// or a refusal naming why a full walk is required instead. There is no
/// third "error" case -- a failed replay is a refusal, never an `Err`.
#[derive(Debug, Clone)]
pub struct FsEventsPlan {
    pub incremental: bool,
    pub refusal: Option<RefreshRefusal>,
    /// Meaningful only when `incremental` is true. Every reported path
    /// contributes itself and its parent directory (mirrors mole's
    /// `changeSet`), deduplicated, so a creation (visible only from the
    /// parent's listing) and a deletion (nothing left to walk) are both
    /// covered.
    pub changed_dirs: Vec<PathBuf>,
    /// The id to persist for the next call. Meaningful even on a
    /// refusal: one forced full refresh re-anchors the stream rather than
    /// condemning every later run to be full as well. `0` when the
    /// platform could not tell us one at all.
    pub current_event_id: u64,
    /// The device the root lived on for this replay, persisted alongside
    /// `current_event_id` so the next call can detect a root that moved
    /// to a different volume.
    pub device: Option<u64>,
    /// The changes came from a live stream (`watch`), not a replay of the
    /// persisted log, so the replay-lag floor (`TooSoon`) does not apply:
    /// a live event is the change, not a query that might predate it.
    pub live: bool,
    /// Linux collector: which dirty-list entries this plan's walk covers.
    /// Applied only after the observation's history is written
    /// (`growth::ObservationCheckpoint::commit`). `None` everywhere else.
    pub consume: Option<crate::continuity::Consumption>,
}

impl FsEventsPlan {
    /// An incremental plan built from live stream batches.
    pub fn from_live(
        changed_dirs: Vec<PathBuf>,
        current_event_id: u64,
        device: Option<u64>,
    ) -> Self {
        Self {
            incremental: true,
            refusal: None,
            changed_dirs,
            current_event_id,
            device,
            live: true,
            consume: None,
        }
    }

    /// A refusal a live consumer builds itself: the watch lost coverage,
    /// the collector stopped, or the stored observation predates the
    /// watch's epoch. The walk is full, and `reason` is what the coverage
    /// note names.
    ///
    /// Marked `live`: the replay-lag floor ([`RefreshRefusal::TooSoon`])
    /// protects a replay of a *persisted* log that may lag a write, and
    /// no persisted log was consulted here, so it must not replace the
    /// actual reason with `too_soon`.
    pub fn refused(reason: RefreshRefusal, device: Option<u64>) -> Self {
        Self {
            live: true,
            ..Self::refuse(reason, 0, device)
        }
    }

    fn refuse(reason: RefreshRefusal, current_event_id: u64, device: Option<u64>) -> Self {
        Self {
            incremental: false,
            refusal: Some(reason),
            changed_dirs: Vec::new(),
            current_event_id,
            device,
            live: false,
            consume: None,
        }
    }

    /// The successful counterpart of [`FsEventsPlan::refuse`], built
    /// only by a backend that actually replayed something. Target-gated
    /// because on a platform with no replay backend there is no caller
    /// and no way to reach it -- and an unreachable constructor for
    /// "incremental: true" is exactly the thing a Linux build should
    /// not contain.
    #[cfg(target_os = "macos")]
    fn ok(changed_dirs: Vec<PathBuf>, current_event_id: u64, device: Option<u64>) -> Self {
        Self {
            incremental: true,
            refusal: None,
            changed_dirs,
            current_event_id,
            device,
            live: false,
            consume: None,
        }
    }

    /// The reason string for coverage notes / log lines, on either branch.
    pub fn reason_str(&self) -> &'static str {
        self.refusal.map(|r| r.as_str()).unwrap_or("incremental")
    }
}

/// The seam a real observation drives, and tests replace with canned
/// event batches so no test depends on the live `fseventsd`.
pub trait FsEventsSource: Send + Sync {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan;

    /// Cheap pre-walk baseline, without replaying history. Returns
    /// (event ID, device). Persist only after the full measurement succeeds.
    /// Unsupported/live-only sources have no persistent baseline to offer.
    fn anchor_before_full(&self, _root: &Path) -> Option<(u64, u64)> {
        None
    }

    /// Replays several roots in one go, returning one plan per request
    /// in the same order.
    ///
    /// An implementation is free to answer them with a single stream --
    /// the macOS one opens one stream per *device*, since that is the
    /// granularity FSEvents' retained log actually has, and splits the
    /// result with [`partition_changes`] so no root ever sees another
    /// root's events. The default implementation replays them one at a
    /// time, which is what every canned test source wants and what the
    /// non-macOS stub needs.
    fn replay_roots(&self, requests: &[FsEventsRequest]) -> Vec<FsEventsPlan> {
        requests.iter().map(|r| self.replay(r)).collect()
    }
}

/// One root's trusted replay window, as `growth` hands it up to the
/// report pipeline: the replay's own (unfiltered) change list, and the
/// observation time the window opens from.
pub type TrustedWindow = (Vec<PathBuf>, u64);

/// Where `consumers::walk` leaves one root's [`TrustedWindow`] for the
/// caller that built the bus context.
pub type EventWindowSlot = std::sync::Arc<std::sync::Mutex<Option<TrustedWindow>>>;

/// One root this observation replayed successfully, and everything the
/// replay said changed underneath it.
#[derive(Debug, Clone)]
struct EventWindow {
    /// The spelling the *units* under this root are addressed in -- the
    /// one a caller passes to [`EventCoverage::unchanged_since`].
    root: PathBuf,
    /// The same root canonicalized, which is the namespace `changed` is
    /// expressed in because that is the only namespace FSEvents answers
    /// in. Equal to `root` for every root that was already canonical.
    canonical_root: PathBuf,
    /// Every path the replay implicated (each reported path plus its
    /// parent), unfiltered, in the canonical namespace.
    changed: Vec<PathBuf>,
    /// The observation time the window replays *from*. Stored rows older
    /// than this were written before the window opened, so the window
    /// cannot vouch for them.
    since_observed_at: u64,
}

/// What this observation's FSEvents replays can vouch for -- the
/// evidence that makes reusing a previous pass's measurement honest
/// rather than hopeful.
///
/// # Why this exists
///
/// Both unit families cache a previous pass's answer: the external
/// family its folded byte totals
/// (`crate::folded_measurement::reuse_folded_measurement`), the agent
/// family a whole container's identified units
/// (`crate::agents::IdentifyCtx::container`). Until 2026-09-22 both
/// decided "unchanged" from the recorded directories' own
/// `mtime`/`ctime` stamps. A directory stamp cannot see a file rewritten
/// **in place**, and for agent storage that is not a corner case: a tool
/// appends to an open session transcript in place, which moves the
/// file's own size and mtime and not its parent's. A growth tool that
/// cannot see the file that is growing is not doing its job, so
/// stamp-only reuse was removed as a sufficient condition.
///
/// Trusted event coverage replaces it, and it is the same rule the Cargo
/// adapter has always followed (`crate::consumers::cargo`): when this
/// pass has a successful FSEvents replay (or live window) over a root,
/// and that window reports no event at or under a path, then nothing
/// under that path changed since the window opened -- appends included,
/// because FSEvents reports writes, not just directory-shape changes.
/// When there is no such window (a full walk, a refusal, an overflow, a
/// first observation), there is no evidence and there is no reuse: the
/// caller re-identifies, where the per-file caches still keep header
/// reads at zero for the files that did not move.
///
/// # What it deliberately does not do
///
/// It does not fall back to directory stamps when a window is missing. A
/// stamp is strictly weaker evidence than the window -- it cannot see
/// the append -- and costs one `stat` per recorded directory per pass,
/// so keeping it as a second opinion would buy nothing and charge for
/// it.
#[derive(Debug, Clone, Default)]
pub struct EventCoverage {
    windows: Vec<EventWindow>,
}

impl EventCoverage {
    /// No evidence at all: every reuse decision must re-derive. This is
    /// what a store-less caller, a forced full walk, an execution-time
    /// recheck and every refusal reason get.
    pub fn untrusted() -> Self {
        Self::default()
    }

    /// Records one root whose replay this pass trusted.
    ///
    /// `changed` must be the replay's own list (each reported path and
    /// its parent), *not* one filtered for exclusions or pruning: a path
    /// this walk chose not to descend into is still a path the window
    /// has to be able to say "changed" about.
    pub fn trust(&mut self, root: PathBuf, changed: Vec<PathBuf>, since_observed_at: u64) {
        let canonical_root = root.clone();
        self.trust_alias(root, canonical_root, changed, since_observed_at);
    }

    /// [`Self::trust`] for a root whose units are addressed by a
    /// non-canonical spelling (a symlinked tool home, a `/var` alias of
    /// `/private/var`).
    ///
    /// Both spellings are needed and neither substitutes for the other.
    /// Matching a queried path against the canonical root alone would
    /// never cover a unit reached through the alias; matching the
    /// alias-form path against `changed` (which FSEvents always answers
    /// in canonical form) would find no event under it and report the
    /// unit quiet *because* the spellings differ -- silently wrong in
    /// the direction that matters. So the query is matched against
    /// `root` and then rewritten into the canonical namespace before it
    /// is tested against `changed`.
    pub fn trust_alias(
        &mut self,
        root: PathBuf,
        canonical_root: PathBuf,
        changed: Vec<PathBuf>,
        since_observed_at: u64,
    ) {
        self.windows.push(EventWindow {
            root,
            canonical_root,
            changed,
            since_observed_at,
        });
    }

    /// Folds another pass-scoped coverage into this one. Windows are
    /// independent evidence, so this is a concatenation: the unit-root
    /// cursors' windows and the walk's windows both vouch for whatever
    /// they each cover.
    pub fn merge(&mut self, other: EventCoverage) {
        self.windows.extend(other.windows);
    }

    /// A single-window coverage, for tests and for callers that replay
    /// exactly one root.
    pub fn trusted(root: PathBuf, changed: Vec<PathBuf>, since_observed_at: u64) -> Self {
        let mut c = Self::untrusted();
        c.trust(root, changed, since_observed_at);
        c
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Whether this pass can show that **nothing under `path` changed**
    /// since `stored_at`, the observation time of the rows a caller
    /// wants to reuse.
    ///
    /// Three things must all hold, and each one is a way reuse goes
    /// wrong when it is missing:
    ///
    /// * some trusted window's root is `path` or an ancestor of it --
    ///   otherwise nothing was watching this path at all;
    /// * the rows are no older than that window's start
    ///   (`stored_at >= since_observed_at`) -- otherwise a change in the
    ///   gap between when the rows were written and when the window
    ///   opened is invisible to both. That gap is exactly what a pass
    ///   which skipped this unit family (`report::ObservationParts`)
    ///   leaves behind, and it is why a window on its own is not enough;
    /// * that window reports no event at `path` or under it. An event
    ///   *above* `path` (a sibling created next door) is not a change to
    ///   `path`, and is not treated as one.
    pub fn unchanged_since(&self, path: &Path, stored_at: u64) -> bool {
        self.windows.iter().any(|w| {
            if stored_at < w.since_observed_at {
                return false;
            }
            let Ok(rel) = path.strip_prefix(&w.root) else {
                return false;
            };
            let in_window = w.canonical_root.join(rel);
            !w.changed.iter().any(|c| c.starts_with(&in_window))
        })
    }
}

/// Splits one shared stream's change list into one list per root: each
/// root gets exactly the reported paths at or under it.
///
/// Several roots on one device are replayed through a single FSEvents
/// stream (see [`FsEventsSource::replay_roots`]), which means one
/// callback sees every root's events. Handing that combined list to
/// every root would make a write under `~/.cargo` read as a change under
/// `~/.claude` -- the cross-talk this function exists to prevent. Roots
/// are compared as canonical paths, the namespace FSEvents reports in.
///
/// Its only production caller is `fs_events::macos`
/// (`target_os = "macos"`-gated); on Linux only the portable unit test
/// below calls it, which the lib target (built without `cfg(test)`)
/// does not see.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn partition_changes(roots: &[PathBuf], changes: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    roots
        .iter()
        .map(|root| {
            changes
                .iter()
                .filter(|c| c.starts_with(root))
                .cloned()
                .collect()
        })
        .collect()
}

/// Returns the platform's real source on macOS, and the always-refusing
/// stub everywhere else.
pub fn platform_source() -> Box<dyn FsEventsSource> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOsFsEventsSource)
    }
    // Linux: a running collector's checkpoint when there is one and it
    // can vouch for the gap; the platform's own refusal otherwise
    // (`crate::continuity`).
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(crate::continuity::CollectorSource)
    }
}

/// One delivery from a live [`watch`]: the directories the platform's
/// watcher reported (each with its parent, within the root) and the
/// newest event id (FSEvents) or sequence number (inotify) seen.
#[derive(Debug, Clone, Default)]
pub struct WatchBatch {
    /// The root the watch is on (the key a consumer with several roots
    /// needs for a batch that names no path: an epoch opening, a loss).
    pub root: PathBuf,
    pub changed_dirs: Vec<PathBuf>,
    pub last_event_id: u64,
    /// The watch stopped being able to vouch for its root (Linux: queue
    /// overflow, watch limit, permissions, unmount, a removed watch).
    /// The consumer must walk the root fully, with this reason, and must
    /// not treat `changed_dirs` as the whole change. Always `None` from
    /// FSEvents, whose own "rescan" flags are reported as the root.
    pub coverage_lost: Option<(RefreshRefusal, String)>,
    /// Linux: when this watch's epoch opened. A root whose stored
    /// observation predates it is not covered by the watch and needs one
    /// full walk before live batches can be applied incrementally.
    /// `None` from FSEvents, whose replay covers the gap itself.
    pub epoch_opened_at: Option<u64>,
}

/// A running live stream. Dropping it, or calling `stop`, ends the
/// thread and releases the stream.
pub struct Watcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A stream whose thread has been spawned but whose
/// `FSEventStreamStart` has not been confirmed yet.
///
/// The split exists because starting an FSEvents stream is a synchronous
/// round-trip to `fseventsd` that is **serialized per process** and
/// measurably costs seconds: on the development machine two streams over
/// two fresh temp directories reported ready at 1.4 s / 2.9 s on a quiet
/// run and at 4.7 s / 6.6 s on a loaded one. A caller that opens one
/// stream per root and waits for each one before spawning the next pays
/// the sum of those; with a fixed per-call budget the later roots are
/// the ones that lose, and the stream they abandon had usually started
/// successfully a moment later. Spawning every root first and only then
/// collecting readiness bounds the wait by the slowest stream instead of
/// their sum.
pub struct PendingWatch {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    ready: std::sync::mpsc::Receiver<bool>,
}

impl PendingWatch {
    /// Waits up to `budget` for this stream to report that
    /// `FSEventStreamStart` succeeded. A stream that reports failure, or
    /// that has not reported at all within the budget, is stopped and
    /// joined: an abandoned stream must not outlive the decision to
    /// abandon it.
    pub fn ready(mut self, budget: std::time::Duration) -> Option<Watcher> {
        match self.ready.recv_timeout(budget) {
            Ok(true) => Some(Watcher {
                stop: self.stop.clone(),
                thread: self.thread.take(),
            }),
            _ => {
                self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
                None
            }
        }
    }
}

impl Drop for PendingWatch {
    fn drop(&mut self) {
        // Only a stream still owned by this pending handle is stopped.
        // `ready` hands the join handle to the `Watcher` it returns and
        // leaves `thread` empty; setting the shared stop flag here
        // regardless would end the stream the caller just took
        // ownership of, on the same line it took it.
        if let Some(t) = self.thread.take() {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = t.join();
        }
    }
}

/// How long a caller waits for one stream to confirm it started.
///
/// Was five seconds, which is smaller than the observed cost of the
/// *second* `FSEventStreamStart` in a process and is exactly why
/// `tui::app::start_watch` intermittently ended up with one watcher for
/// two roots. The budget's job is to stop a caller hanging forever on an
/// `fseventsd` that never answers, not to second-guess how long a call
/// that is known to take seconds is allowed to take. Overridable via
/// `SWAMP_FSEVENTS_WATCH_START_TIMEOUT_SEC`.
pub fn watch_start_budget() -> std::time::Duration {
    std::env::var("SWAMP_FSEVENTS_WATCH_START_TIMEOUT_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(30))
}

impl Watcher {
    /// A watcher whose thread was started elsewhere in this crate (the
    /// Linux inotify watch, `live_watch::spawn_watch`).
    #[cfg(target_os = "linux")]
    pub(crate) fn from_parts(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            stop,
            thread: Some(thread),
        }
    }

    pub fn stop(mut self) {
        self.signal_stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
    fn signal_stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.signal_stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts a live FSEvents stream on `root` from now, delivering a
/// [`WatchBatch`] on `tx` each time FSEvents flushes (latency 0.5 s). The
/// stream runs on its own thread with its own run loop. `None` where the
/// platform has no FSEvents.
///
/// Blocks until the stream confirms it started (see
/// [`watch_start_budget`]). A caller opening several streams should use
/// [`watch_pending`] and collect readiness afterwards instead, so the
/// per-stream `fseventsd` latencies overlap rather than add up.
pub fn watch(root: &Path, tx: std::sync::mpsc::Sender<WatchBatch>) -> Option<Watcher> {
    watch_excluding(root, Vec::new(), tx)
}

/// [`watch`], never reporting a change under `exclude` -- swamp's own
/// store, so writing an observation's results cannot trigger the next
/// live refresh. On macOS the exclusion is not needed (the stream is
/// unchanged: `watch_pending(..).ready(..)`); on Linux the watcher
/// registers directories itself and simply does not register those.
pub fn watch_excluding(
    root: &Path,
    exclude: Vec<PathBuf>,
    tx: std::sync::mpsc::Sender<WatchBatch>,
) -> Option<Watcher> {
    #[cfg(target_os = "macos")]
    {
        let _ = exclude;
        watch_pending(root, tx)?.ready(watch_start_budget())
    }
    #[cfg(target_os = "linux")]
    {
        crate::live_watch::spawn_watch(root, exclude, tx)
    }
}

/// Spawns `root`'s stream thread and returns immediately; the caller
/// decides when (and for how long) to wait for it to report ready.
/// `None` where the platform has no FSEvents, or where the thread itself
/// could not be spawned.
pub fn watch_pending(root: &Path, tx: std::sync::mpsc::Sender<WatchBatch>) -> Option<PendingWatch> {
    #[cfg(target_os = "macos")]
    {
        macos::watch_pending(root, tx)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, tx);
        None
    }
}

/// The seam `tui::app::start_watch` opens its streams through, so a test
/// can assert "one stream per root" without depending on `fseventsd`
/// answering within any particular time.
pub type WatchFactory = fn(&Path, std::sync::mpsc::Sender<WatchBatch>) -> Option<PendingWatch>;

/// The non-macOS fallback: always refuses, naming the platform as the
/// cause, never a bug in the replay itself.
///
/// The refusal it gives is the one the platform's continuity source
/// justifies, not a generic "unsupported": on Linux there is no
/// persisted kernel change history to replay from
/// ([`RefreshRefusal::NoPersistedChangeHistory`]), which is a different
/// statement from "nobody has written the backend yet" and leads to a
/// different answer for the user.
pub struct UnsupportedPlatformSource;

impl FsEventsSource for UnsupportedPlatformSource {
    fn replay(&self, _request: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::refuse(platform_refusal(), 0, None)
    }
}

/// Why *this* build cannot replay history. Derived from the platform's
/// continuity source rather than from the target triple, so a future
/// backend changes one table instead of every refusal site.
pub fn platform_refusal() -> RefreshRefusal {
    if crate::platform::ContinuitySource::for_os(crate::platform::Os::current()).replays_history() {
        RefreshRefusal::UnsupportedPlatform
    } else {
        RefreshRefusal::NoPersistedChangeHistory
    }
}

/// Adds `path` and its parent directory to `changes`, provided each is
/// within `root`. FSEvents reports a path whose meaning (the changed item
/// itself, or the directory containing it) depends on flags this design
/// deliberately does not trust for that judgement (see
/// `docs/fsevents-helper.md`'s "every path contributes itself and its
/// parent"); adding both costs one extra listing on re-walk and closes
/// both a creation and a deletion.
#[cfg(target_os = "macos")]
fn add_with_parent(changes: &mut std::collections::HashSet<PathBuf>, root: &Path, path: &Path) {
    for candidate in [
        Some(path.to_path_buf()),
        path.parent().map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    {
        if candidate == root || candidate.starts_with(root) {
            changes.insert(candidate);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // fsevent-sys itself is marked deprecated in favour of
// objc2-core-services, but it is a maintained, direct binding to the same
// stable CoreServices C API this module needs and ships today; swapping
// bindings crates is out of scope for this change.
mod macos;

/// A source that answers with a plan the caller already has: the TUI's
/// live refresh, which turns what its own FSEvents watcher reported into
/// a replay plan for one root (`tui::app::observe_live`).
///
/// A production type, not a test double. It used to be
/// `testing::CannedSource`, whose documentation called it "test-only
/// support … inert in a release binary" while the live refresh ran
/// through it; and the doc comment's own words hid it from the old
/// program-model audit (re-review 4, C3/U1).
///
/// It cannot manufacture an incremental walk on its own: the plan comes
/// from [`FsEventsPlan::from_live`], and `growth::stage_tracked_with_source`
/// refuses an incremental plan for a root whose stored state has no
/// anchor or carries an older rules version (re-review 4, C2).
pub struct LivePlanSource(FsEventsPlan);

impl LivePlanSource {
    pub fn new(plan: FsEventsPlan) -> Self {
        Self(plan)
    }
}

impl FsEventsSource for LivePlanSource {
    fn replay(&self, _request: &FsEventsRequest) -> FsEventsPlan {
        self.0.clone()
    }
}

/// Test fixtures: a canned source and an inert watch factory, for this
/// crate's integration tests and the TUI's tests. Compiled only with the
/// `testing` feature, which only `[dev-dependencies]` enable (and which
/// `lib.rs` refuses to build in a release profile), so no production
/// build contains it.
#[cfg(feature = "testing")]
pub mod testing {
    use super::{FsEventsPlan, FsEventsRequest, FsEventsSource, PendingWatch, WatchBatch};
    use std::path::Path;

    /// A canned source: returns whatever plan it was built with,
    /// regardless of the request. Lets every refusal reason and the
    /// incremental path be exercised without depending on the live
    /// `fseventsd`.
    pub struct CannedSource(pub FsEventsPlan);

    impl FsEventsSource for CannedSource {
        fn replay(&self, _request: &FsEventsRequest) -> FsEventsPlan {
            self.0.clone()
        }
    }

    /// A [`super::WatchFactory`] that opens no FSEvents stream and
    /// reports ready immediately.
    ///
    /// It exists so "one stream per root" can be asserted as a property
    /// of the caller's loop rather than of how quickly `fseventsd`
    /// answers. Starting a real stream is a multi-second, per-process
    /// serialized call (see [`super::PendingWatch`]), which makes any
    /// test that opens two real streams a race against its own budget.
    pub fn inert_watch_factory(
        _root: &Path,
        _tx: std::sync::mpsc::Sender<WatchBatch>,
    ) -> Option<PendingWatch> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let _ = ready_tx.send(true);
        Some(PendingWatch {
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            thread: None,
            ready: ready_rx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    #[test]
    fn live_watch_reports_a_write_under_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = watch(&root, tx).expect("fseventsd available on macOS");
        std::thread::sleep(Duration::from_millis(300));
        let dir = root.join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.extend(b.changed_dirs);
                if seen.iter().any(|p| p == &dir) {
                    break;
                }
            }
        }
        watcher.stop();
        assert!(
            seen.iter().any(|p| p == &dir),
            "expected {} among live changes, got {seen:?}",
            dir.display()
        );
    }

    #[test]
    fn a_platform_without_a_backend_always_refuses_and_names_which_kind() {
        let source = UnsupportedPlatformSource;
        let plan = source.replay(&FsEventsRequest {
            root: PathBuf::from("/tmp"),
            since: FsEventsState {
                event_id: Some(1),
                device: Some(1),
                last_observed_at: Some(1),
                rules_version: 0,
                unit_root: None,
            },
            swamp_dir: None,
            excluded: Vec::new(),
        });
        assert!(!plan.incremental);
        assert!(
            plan.changed_dirs.is_empty(),
            "a refusal never carries a change list"
        );
        // Which refusal depends on *why* there is no replay. A build on
        // a kernel that keeps no change history says so; only a target
        // with no backend written for it says "unsupported".
        assert_eq!(plan.refusal, Some(platform_refusal()));
        assert_eq!(plan.reason_str(), platform_refusal().as_str());
    }

    /// The distinction that must not collapse: "no backend yet" invites
    /// someone to write one; "the kernel keeps no history" is a fact
    /// about Linux that #81's watcher narrows but does not remove. A
    /// build that reported the first where the second is true would
    /// promise a Linux user an incremental refresh that can never come.
    #[test]
    fn a_kernel_without_persisted_history_says_so_rather_than_unsupported() {
        assert_eq!(
            RefreshRefusal::NoPersistedChangeHistory.as_str(),
            "no_persisted_change_history"
        );
        let why = RefreshRefusal::NoPersistedChangeHistory
            .explanation()
            .expect("this refusal must explain itself in words");
        assert!(why.contains("inotify"), "{why}");
        assert!(
            RefreshRefusal::UnsupportedPlatform.explanation().is_none(),
            "only the platform-history refusal carries that explanation"
        );

        // And the mapping is derived from the continuity source, not
        // from a hand-written per-target list.
        let expected = if crate::platform::ContinuitySource::for_os(crate::platform::Os::current())
            .replays_history()
        {
            RefreshRefusal::UnsupportedPlatform
        } else {
            RefreshRefusal::NoPersistedChangeHistory
        };
        assert_eq!(platform_refusal(), expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn add_with_parent_stays_within_root() {
        let root = Path::new("/root");
        let mut changes = std::collections::HashSet::new();
        add_with_parent(&mut changes, root, Path::new("/root/a/b"));
        assert!(changes.contains(Path::new("/root/a/b")));
        assert!(changes.contains(Path::new("/root/a")));

        let mut changes = std::collections::HashSet::new();
        add_with_parent(&mut changes, root, Path::new("/elsewhere/x"));
        assert!(changes.is_empty(), "a path outside root must not be added");
    }

    /// Two roots on one device share one FSEvents stream, so the
    /// callback sees both roots' events. Splitting that union is the
    /// only thing standing between "a write under the Cargo home" and
    /// "the Claude Code home changed"; assert the split directly, since
    /// the stream itself cannot be reproduced deterministically.
    #[test]
    fn a_shared_stream_gives_each_root_only_its_own_changes() {
        let cargo = PathBuf::from("/Users/x/.cargo");
        let claude = PathBuf::from("/Users/x/.claude");
        let changes = vec![
            cargo.join("registry/cache"),
            cargo.join("registry"),
            PathBuf::from("/Users/x/elsewhere"),
        ];
        let split = partition_changes(&[cargo.clone(), claude.clone()], &changes);
        assert_eq!(split[0].len(), 2, "the Cargo home keeps its own two paths");
        assert!(
            split[1].is_empty(),
            "the Claude Code home saw nothing: {:?}",
            split[1]
        );
        // And the consequence the split exists for.
        let mut coverage = EventCoverage::untrusted();
        coverage.trust(cargo.clone(), split[0].clone(), 1_000);
        coverage.trust(claude.clone(), split[1].clone(), 1_000);
        assert!(
            !coverage.unchanged_since(&cargo, 1_000),
            "the root that changed must not be reported unchanged"
        );
        assert!(
            coverage.unchanged_since(&claude, 1_000),
            "the quiet root on the same stream is still reusable"
        );
    }

    /// A window whose root is reached through an alias spelling
    /// (`/var/folders/...` for `/private/var/folders/...`) must translate
    /// the queried path into the canonical namespace before testing it
    /// against the change list. Skipping the translation reports the unit
    /// quiet because the spellings differ, which is the wrong answer in
    /// the only direction that matters.
    #[test]
    fn an_aliased_root_still_sees_its_own_changes() {
        let alias = PathBuf::from("/var/home/.claude");
        let canonical = PathBuf::from("/private/var/home/.claude");
        let mut coverage = EventCoverage::untrusted();
        coverage.trust_alias(
            alias.clone(),
            canonical.clone(),
            vec![canonical.join("projects/a")],
            1_000,
        );
        assert!(
            !coverage.unchanged_since(&alias.join("projects/a"), 1_000),
            "the change is under the queried path, however it is spelled"
        );
        assert!(
            coverage.unchanged_since(&alias.join("projects/b"), 1_000),
            "a sibling that did not change is still reusable"
        );
        assert!(
            !coverage.unchanged_since(&PathBuf::from("/somewhere/else"), 1_000),
            "a path outside the root is not covered at all"
        );
    }

    /// Merging is concatenation: the unit-root cursors' windows and the
    /// walk's windows are independent evidence and neither overrides the
    /// other.
    #[test]
    fn merging_coverage_keeps_both_sets_of_windows() {
        let mut walk = EventCoverage::trusted(PathBuf::from("/src"), Vec::new(), 1_000);
        let units = EventCoverage::trusted(PathBuf::from("/home/.cargo"), Vec::new(), 1_000);
        walk.merge(units);
        assert!(walk.unchanged_since(Path::new("/src/proj"), 1_000));
        assert!(walk.unchanged_since(Path::new("/home/.cargo/registry"), 1_000));
        assert!(!walk.unchanged_since(Path::new("/home/.claude"), 1_000));
    }

    #[test]
    fn reason_str_covers_every_refusal() {
        for (reason, expected) in [
            (RefreshRefusal::NoStoredEventId, "no_stored_event_id"),
            (RefreshRefusal::EventIdFromFuture, "event_id_from_future"),
            (RefreshRefusal::RootMismatch, "root_mismatch"),
            (
                RefreshRefusal::FseventsdUnavailable,
                "fseventsd_unavailable",
            ),
            (RefreshRefusal::HelperInconclusive, "helper_inconclusive"),
            (RefreshRefusal::TooManyChanges, "too_many_changes"),
            (RefreshRefusal::TooSoon, "too_soon"),
            (RefreshRefusal::UnsupportedPlatform, "unsupported_platform"),
            (
                RefreshRefusal::NoPersistedChangeHistory,
                "no_persisted_change_history",
            ),
        ] {
            assert_eq!(reason.as_str(), expected);
        }
    }
}
