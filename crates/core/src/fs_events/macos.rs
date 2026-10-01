//! The macOS FSEvents FFI: the only code in this crate outside
//! `fs_gate` that the gate audit allows to use `unsafe`, `extern "C"`
//! and the CoreFoundation/`fsevent-sys` bindings. It is a child of
//! `fs_events` because it builds that module's private plan types; it
//! does no filesystem I/O beyond `stat`/`canonicalize` of the roots it
//! is asked to watch.

#![allow(unsafe_code, clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
use core_foundation::string::CFString;
use core_foundation::uuid::{CFUUID, CFUUIDGetUUIDBytes, CFUUIDRef};
use fsevent_sys as fs;
use std::ffi::CStr;
use std::os::raw::c_void;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventsCopyUUIDForDevice(dev: std::os::unix::raw::dev_t) -> CFUUIDRef;
}

/// Returns the identity of the device's persisted FSEvents stream. Apple
/// documents this UUID as changing when history is purged, the device is
/// erased, or the event counter wraps; NULL means no historical stream.
fn device_uuid(dev: u64) -> Option<String> {
    // SAFETY: `dev` came from stat and the API returns a retained CFUUID or NULL.
    let raw = unsafe { FSEventsCopyUUIDForDevice(dev as std::os::unix::raw::dev_t) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: FSEventsCopyUUIDForDevice follows the Copy ownership rule.
    let uuid = unsafe { CFUUID::wrap_under_create_rule(raw) };
    // SAFETY: `uuid` owns a live CFUUID reference.
    let b = unsafe { CFUUIDGetUUIDBytes(uuid.as_concrete_TypeRef()) };
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b.byte0,
        b.byte1,
        b.byte2,
        b.byte3,
        b.byte4,
        b.byte5,
        b.byte6,
        b.byte7,
        b.byte8,
        b.byte9,
        b.byte10,
        b.byte11,
        b.byte12,
        b.byte13,
        b.byte14,
        b.byte15,
    ))
}

/// Bounds one replay's run loop. FSEvents replays retained history
/// from local, in-kernel state, so a replay that has not reached
/// `HistoryDone` in this long is not going to; refusing with
/// `HelperInconclusive` beats blocking an observation indefinitely.
/// Overridable for slow CI via `SWAMP_FSEVENTS_TIMEOUT_SEC`.
fn replay_budget() -> Duration {
    std::env::var("SWAMP_FSEVENTS_TIMEOUT_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(6))
}

struct Collector {
    /// Every root this stream is watching. One stream can carry
    /// several roots on the same device; the collector keeps their
    /// union and `replay_many` splits it per root afterwards.
    roots: Vec<PathBuf>,
    changes: std::collections::HashSet<PathBuf>,
    /// Directories FSEvents could not itemize (`MustScanSubDirs`): only
    /// the roots they touch are refused (`partition_replay`).
    rescans: Vec<PathBuf>,
    history_done: bool,
    hard_fail: Option<RefreshRefusal>,
}

extern "C" fn stream_callback(
    _stream: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const fs::FSEventStreamEventFlags,
    _event_ids: *const fs::FSEventStreamEventId,
) {
    if info.is_null() || num_events == 0 {
        return;
    }
    // SAFETY: `info` is the `Collector` this replay's caller created
    // and kept alive on the stack for the stream's entire lifetime;
    // FSEvents only invokes this callback on the run loop that same
    // stack frame is pumping.
    let collector = unsafe { &mut *(info as *mut Collector) };
    let paths = event_paths as *const *const std::os::raw::c_char;
    // SAFETY: FSEvents guarantees `num_events` valid entries in both
    // the flags array and the paths array it hands the callback.
    let flags = unsafe { std::slice::from_raw_parts(event_flags, num_events) };

    for (i, &f) in flags.iter().enumerate() {
        // Dropped events (user or kernel queue overflow) cannot be
        // located with confidence: the whole stream stays inconclusive.
        if (f & fs::kFSEventStreamEventFlagEventIdsWrapped) != 0
            || (f & fs::kFSEventStreamEventFlagUserDropped) != 0
            || (f & fs::kFSEventStreamEventFlagKernelDropped) != 0
        {
            collector
                .hard_fail
                .get_or_insert(RefreshRefusal::HelperInconclusive);
            continue;
        }
        if (f & fs::kFSEventStreamEventFlagMustScanSubDirs) != 0 {
            // SAFETY: as below, `i` is in range of `paths`.
            let cpath = unsafe { *paths.add(i) };
            if cpath.is_null() {
                collector
                    .hard_fail
                    .get_or_insert(RefreshRefusal::HelperInconclusive);
            } else {
                // SAFETY: FSEvents paths are NUL-terminated C strings.
                let cstr = unsafe { CStr::from_ptr(cpath) };
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(cstr.to_bytes()));
                collector.rescans.push(path);
            }
            continue;
        }
        if (f & fs::kFSEventStreamEventFlagRootChanged) != 0
            || (f & fs::kFSEventStreamEventFlagUnmount) != 0
        {
            collector
                .hard_fail
                .get_or_insert(RefreshRefusal::RootMismatch);
            continue;
        }
        if (f & fs::kFSEventStreamEventFlagHistoryDone) != 0 {
            collector.history_done = true;
            CFRunLoop::get_current().stop();
            continue;
        }

        // SAFETY: `paths` was validated non-null by FSEvents for
        // every one of `num_events` entries; `i` is in range.
        let cpath = unsafe { *paths.add(i) };
        if cpath.is_null() {
            // A path we cannot read is a change we cannot locate:
            // refusing is the difference between a full refresh and a
            // generation that silently omits this event.
            collector
                .hard_fail
                .get_or_insert(RefreshRefusal::HelperInconclusive);
            continue;
        }
        // SAFETY: FSEvents paths are NUL-terminated C strings.
        let cstr = unsafe { CStr::from_ptr(cpath) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(cstr.to_bytes()));
        for i in 0..collector.roots.len() {
            let root = collector.roots[i].clone();
            add_with_parent(&mut collector.changes, &root, &path);
        }
    }
}

/// State behind the live stream's callback: where batches go.
struct WatchState {
    root: PathBuf,
    tx: std::sync::mpsc::Sender<super::WatchBatch>,
}

extern "C" fn watch_callback(
    _stream: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const fs::FSEventStreamEventFlags,
    event_ids: *const fs::FSEventStreamEventId,
) {
    if info.is_null() || num_events == 0 {
        return;
    }
    // SAFETY: `info` is the `WatchState` the watch thread boxed and
    // keeps alive until after the stream is invalidated; FSEvents
    // invokes this callback only on that thread's run loop.
    let state = unsafe { &*(info as *const WatchState) };
    let paths = event_paths as *const *const std::os::raw::c_char;
    // SAFETY: FSEvents guarantees `num_events` valid entries in the
    // flags, paths and ids arrays.
    let flags = unsafe { std::slice::from_raw_parts(event_flags, num_events) };
    let ids = unsafe { std::slice::from_raw_parts(event_ids, num_events) };
    let mut changes = std::collections::HashSet::new();
    let mut last_id = 0u64;
    for (i, &f) in flags.iter().enumerate() {
        last_id = last_id.max(ids[i]);
        if (f & fs::kFSEventStreamEventFlagMustScanSubDirs) != 0
            || (f & fs::kFSEventStreamEventFlagRootChanged) != 0
            || (f & fs::kFSEventStreamEventFlagUnmount) != 0
        {
            // Everything under the root may have changed: report the
            // root itself; the consumer decides between an incremental
            // re-walk of it and a full walk.
            changes.insert(state.root.clone());
            continue;
        }
        // SAFETY: validated non-null by FSEvents; `i` in range.
        let cpath = unsafe { *paths.add(i) };
        if cpath.is_null() {
            continue;
        }
        // SAFETY: FSEvents paths are NUL-terminated C strings.
        let cstr = unsafe { CStr::from_ptr(cpath) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(cstr.to_bytes()));
        add_with_parent(&mut changes, &state.root, &path);
    }
    if !changes.is_empty() {
        let _ = state.tx.send(super::WatchBatch {
            root: state.root.clone(),
            changed_dirs: changes.into_iter().collect(),
            last_event_id: last_id,
            coverage_lost: None,
            epoch_opened_at: None,
        });
    }
}

pub fn watch_pending(
    root: &Path,
    tx: std::sync::mpsc::Sender<super::WatchBatch>,
) -> Option<super::PendingWatch> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
    let thread = std::thread::Builder::new()
        .name("fsevents-watch".into())
        .spawn(move || {
            let state = Box::new(WatchState {
                root: root.clone(),
                tx,
            });
            let info_ptr = state.as_ref() as *const WatchState as *mut c_void;
            let cf_path = CFString::new(&root.to_string_lossy());
            let paths_array: CFArray<CFString> = CFArray::from_CFTypes(&[cf_path]);
            let context = fs::FSEventStreamContext {
                version: 0,
                info: info_ptr,
                retain: None,
                release: None,
                copy_description: None,
            };
            let create_flags =
                fs::kFSEventStreamCreateFlagNoDefer | fs::kFSEventStreamCreateFlagWatchRoot;
            // SAFETY: `paths_array`, `context` and `state` outlive the
            // stream, which is released below before they drop.
            let stream = unsafe {
                fs::FSEventStreamCreate(
                    core_foundation_sys::base::kCFAllocatorDefault,
                    watch_callback,
                    &context,
                    paths_array.as_concrete_TypeRef() as CFArrayRef,
                    fs::kFSEventStreamEventIdSinceNow,
                    0.5,
                    create_flags,
                )
            };
            if stream.is_null() {
                let _ = ready_tx.send(false);
                return;
            }
            // SAFETY: stream just created; released on every exit below.
            let started = unsafe {
                fs::FSEventStreamScheduleWithRunLoop(
                    stream,
                    CFRunLoop::get_current().as_concrete_TypeRef(),
                    kCFRunLoopDefaultMode,
                );
                fs::FSEventStreamStart(stream) != 0
            };
            let _ = ready_tx.send(started);
            if started {
                while !stop_thread.load(Ordering::Relaxed) {
                    CFRunLoop::run_in_mode(
                        unsafe { kCFRunLoopDefaultMode },
                        Duration::from_millis(250),
                        true,
                    );
                }
                // SAFETY: matches the successful start above.
                unsafe {
                    fs::FSEventStreamStop(stream);
                }
            }
            // SAFETY: matches `FSEventStreamCreate` above.
            unsafe {
                fs::FSEventStreamInvalidate(stream);
                fs::FSEventStreamRelease(stream);
            }
            drop(state);
        })
        .ok()?;
    Some(super::PendingWatch {
        stop,
        thread: Some(thread),
        ready: ready_rx,
    })
}

pub struct MacOsFsEventsSource;

impl FsEventsSource for MacOsFsEventsSource {
    fn anchor_before_full(&self, root: &Path) -> Option<(u64, u64, Option<String>)> {
        // Capture before walking, never after: later events must remain replayable.
        let device = std::fs::metadata(root).ok()?.dev();
        let uuid_before = device_uuid(device)?;
        // SAFETY: a read-only query with no arguments.
        let event_id = unsafe { fs::FSEventsGetCurrentEventId() };
        let uuid_after = device_uuid(device);
        full_anchor(event_id, device, Some(uuid_before), uuid_after)
    }

    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        self.replay_roots(std::slice::from_ref(request))
            .pop()
            .unwrap_or_else(|| FsEventsPlan::refuse(RefreshRefusal::FseventsdUnavailable, 0, None))
    }

    fn replay_roots(&self, requests: &[FsEventsRequest]) -> Vec<FsEventsPlan> {
        replay_many(requests)
    }
}

fn full_anchor(
    event_id: u64,
    device: u64,
    uuid_before: Option<String>,
    uuid_after: Option<String>,
) -> Option<(u64, u64, Option<String>)> {
    match (event_id, uuid_before, uuid_after) {
        (id, Some(before), Some(after)) if id != 0 && before == after => {
            Some((id, device, Some(after)))
        }
        _ => None,
    }
}

/// Every root whose pre-checks passed, grouped by the device its
/// FSEvents history lives on.
struct Group {
    dev: u64,
    device_uuid: String,
    /// Index into the caller's request slice, so each plan goes back
    /// to the root that asked for it.
    members: Vec<usize>,
    /// The earliest anchor among the members. A member whose own
    /// anchor is later simply sees some events it already knew
    /// about, which can only make it re-measure something that did
    /// not need it -- never the other way round.
    since_id: u64,
}

fn replay_cursor(
    since: &FsEventsState,
    dev: u64,
    current: u64,
    current_uuid: Option<&str>,
) -> Result<(u64, String), Box<FsEventsPlan>> {
    if let Some(stored_device) = since.device
        && stored_device != dev
    {
        let mut plan = FsEventsPlan::refuse(RefreshRefusal::RootMismatch, current, Some(dev));
        plan.device_uuid = current_uuid.map(str::to_owned);
        return Err(Box::new(plan));
    }
    let Some(device_uuid) = current_uuid else {
        return Err(Box::new(FsEventsPlan::refuse(
            RefreshRefusal::FseventsdUnavailable,
            current,
            Some(dev),
        )));
    };
    let Some(since_id) = since.event_id else {
        let mut plan = FsEventsPlan::refuse(RefreshRefusal::NoStoredEventId, current, Some(dev));
        plan.device_uuid = Some(device_uuid.to_owned());
        return Err(Box::new(plan));
    };
    if since.device_uuid.as_deref() != Some(device_uuid) {
        let mut plan = FsEventsPlan::refuse(RefreshRefusal::HelperInconclusive, current, Some(dev));
        plan.device_uuid = Some(device_uuid.to_owned());
        return Err(Box::new(plan));
    }
    if since_id > current {
        let mut plan = FsEventsPlan::refuse(RefreshRefusal::EventIdFromFuture, current, Some(dev));
        plan.device_uuid = Some(device_uuid.to_owned());
        return Err(Box::new(plan));
    }
    Ok((since_id, device_uuid.to_owned()))
}

fn replay_many(requests: &[FsEventsRequest]) -> Vec<FsEventsPlan> {
    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let prechecks = Instant::now();
    // SAFETY: no arguments; a pure query of the FSEvents subsystem.
    let current = unsafe { fs::FSEventsGetCurrentEventId() };
    if current == 0 {
        return requests
            .iter()
            .map(|_| FsEventsPlan::refuse(RefreshRefusal::FseventsdUnavailable, 0, None))
            .collect();
    }

    let mut plans: Vec<Option<FsEventsPlan>> = vec![None; requests.len()];
    let mut groups: Vec<Group> = Vec::new();
    // Ask once per device for the stream identity, not once per root.
    // The BeforeTime API returns the newest event before a timestamp,
    // not the oldest retained event, so it cannot establish a history floor.
    let mut uuid_by_dev: std::collections::HashMap<u64, Option<String>> =
        std::collections::HashMap::new();

    for (i, request) in requests.iter().enumerate() {
        let root = &request.root;
        let since = &request.since;
        let dev = match std::fs::metadata(root) {
            Ok(meta) => meta.dev(),
            Err(_) => {
                plans[i] = Some(FsEventsPlan::refuse(
                    RefreshRefusal::FseventsdUnavailable,
                    current,
                    None,
                ));
                continue;
            }
        };

        let current_uuid = uuid_by_dev.entry(dev).or_insert_with(|| device_uuid(dev));
        let (since_id, device_uuid) =
            match replay_cursor(since, dev, current, current_uuid.as_deref()) {
                Ok(cursor) => cursor,
                Err(plan) => {
                    plans[i] = Some(*plan);
                    continue;
                }
            };
        match groups.iter_mut().find(|g| g.dev == dev) {
            Some(g) => {
                g.members.push(i);
                g.since_id = g.since_id.min(since_id);
            }
            None => groups.push(Group {
                dev,
                device_uuid,
                members: vec![i],
                since_id,
            }),
        }
    }

    if trace {
        eprintln!(
            "[trace] fsevents prechecks: roots={} devices={} groups={} elapsed={:?}",
            requests.len(),
            uuid_by_dev.len(),
            groups.len(),
            prechecks.elapsed()
        );
    }

    // One stream per device, all at once: each waits on fseventsd for
    // its own history to drain (up to `replay_budget`), and the waits
    // are independent -- serially, four devices cost four waits (R19:
    // 2.4 s of an unchanged pass). Every stream runs its own CFRunLoop
    // on its own thread, as the live watch already does.
    let group_roots: Vec<Vec<PathBuf>> = groups
        .iter()
        .map(|group| {
            group
                .members
                .iter()
                .map(|&i| requests[i].root.clone())
                .collect()
        })
        .collect();
    type Replayed = Result<(Vec<PathBuf>, Vec<PathBuf>), RefreshRefusal>;
    let results: Vec<(Replayed, bool)> = std::thread::scope(|scope| {
        let handles: Vec<_> = groups
            .iter()
            .zip(group_roots.iter())
            .map(|(group, roots)| {
                let since_id = group.since_id;
                let dev = group.dev;
                let expected_uuid = group.device_uuid.clone();
                scope.spawn(move || {
                    let started = Instant::now();
                    let mut result = run_stream(roots, since_id);
                    let stream_changed = device_uuid(dev).as_deref() != Some(&expected_uuid);
                    if stream_changed {
                        result = Err(RefreshRefusal::HelperInconclusive);
                    }
                    if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                        eprintln!(
                            "[xtrace] fsevents group dev={dev} roots={} since={since_id} current={current} ok={} rescans={:?} elapsed={:?}",
                            roots.len(),
                            result.is_ok(),
                            result.as_ref().map(|(_, r)| r.len()).map_err(|e| e.as_str()),
                            started.elapsed()
                        );
                    }
                    (result, stream_changed)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or((Err(RefreshRefusal::FseventsdUnavailable), false))
            })
            .collect()
    });
    for ((group, roots), (result, stream_changed)) in
        groups.iter().zip(group_roots.iter()).zip(results)
    {
        if stream_changed {
            for &i in &group.members {
                // The pre-replay ID and UUID no longer name the same stream.
                // Publish neither; the full walk must obtain a fresh,
                // UUID-bracketed anchor before traversing the filesystem.
                plans[i] = Some(FsEventsPlan::refuse(
                    RefreshRefusal::HelperInconclusive,
                    0,
                    Some(group.dev),
                ));
            }
            continue;
        }
        match result {
            Err(reason) => {
                for &i in &group.members {
                    let mut plan = FsEventsPlan::refuse(reason, current, Some(group.dev));
                    plan.device_uuid = Some(group.device_uuid.clone());
                    plans[i] = Some(plan);
                }
            }
            Ok((changes, rescans)) => {
                let per_root = super::partition_replay(roots, &changes, &rescans);
                for (&i, outcome) in group.members.iter().zip(per_root) {
                    plans[i] = Some(match outcome {
                        Ok(changed) => FsEventsPlan::ok(changed, current, Some(group.dev)),
                        Err(reason) => FsEventsPlan::refuse(reason, current, Some(group.dev)),
                    });
                    if let Some(plan) = plans[i].as_mut() {
                        plan.device_uuid = Some(group.device_uuid.clone());
                    }
                }
            }
        }
    }

    plans
        .into_iter()
        .map(|p| {
            p.unwrap_or_else(|| {
                FsEventsPlan::refuse(RefreshRefusal::FseventsdUnavailable, current, None)
            })
        })
        .collect()
}

/// One FSEvents stream over `roots` (all on one device), replayed
/// from `since_id`. `Ok` is the complete union of implicated paths;
/// `Err` is the one refusal every root in the group shares, because
/// an inconclusive replay is inconclusive for all of them.
fn run_stream(
    roots: &[PathBuf],
    since_id: u64,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RefreshRefusal> {
    let mut collector = Box::new(Collector {
        roots: roots.to_vec(),
        changes: std::collections::HashSet::new(),
        rescans: Vec::new(),
        history_done: false,
        hard_fail: None,
    });
    let info_ptr = collector.as_mut() as *mut Collector as *mut c_void;

    let cf_paths: Vec<CFString> = roots
        .iter()
        .map(|r| CFString::new(&r.to_string_lossy()))
        .collect();
    let paths_array: CFArray<CFString> = CFArray::from_CFTypes(&cf_paths);
    let context = fs::FSEventStreamContext {
        version: 0,
        info: info_ptr,
        retain: None,
        release: None,
        copy_description: None,
    };
    // NoDefer delivers the first batch immediately instead of after
    // the latency window; WatchRoot reports the root itself being
    // moved or replaced. Directory-level events (no FileEvents flag)
    // suffice: rows here are per artifact/worktree/dir, never
    // per-file.
    let create_flags = fs::kFSEventStreamCreateFlagNoDefer | fs::kFSEventStreamCreateFlagWatchRoot;

    // SAFETY: `paths_array` and `context` outlive the call; the
    // callback pointer has the exact signature FSEvents expects.
    let stream = unsafe {
        fs::FSEventStreamCreate(
            core_foundation_sys::base::kCFAllocatorDefault,
            stream_callback,
            &context,
            paths_array.as_concrete_TypeRef() as CFArrayRef,
            since_id,
            0.0,
            create_flags,
        )
    };
    if stream.is_null() {
        return Err(RefreshRefusal::FseventsdUnavailable);
    }

    // SAFETY: `stream` was just created and is released below on
    // every path out of this function.
    unsafe {
        fs::FSEventStreamScheduleWithRunLoop(
            stream,
            CFRunLoop::get_current().as_concrete_TypeRef(),
            kCFRunLoopDefaultMode,
        );
        if fs::FSEventStreamStart(stream) == 0 {
            fs::FSEventStreamInvalidate(stream);
            fs::FSEventStreamRelease(stream);
            return Err(RefreshRefusal::FseventsdUnavailable);
        }
    }

    let deadline = Instant::now() + replay_budget();
    while !collector.history_done && collector.hard_fail.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Run in short slices so the loop condition (history_done /
        // hard_fail, both set from inside the callback which runs on
        // this same thread while this call is blocked in it) is
        // re-checked promptly rather than only after the whole
        // remaining budget elapses.
        let slice = remaining.min(Duration::from_millis(200));
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, slice, true);
    }

    // SAFETY: matches the successful `FSEventStreamCreate` above.
    unsafe {
        fs::FSEventStreamStop(stream);
        fs::FSEventStreamInvalidate(stream);
        fs::FSEventStreamRelease(stream);
    }

    if let Some(reason) = collector.hard_fail {
        return Err(reason);
    }
    if !collector.history_done {
        // The run loop stopped (budget exhausted) without FSEvents
        // ever reporting the replay complete. Whatever was collected
        // may be a prefix of what changed, and a prefix is exactly
        // the silent partial this design refuses to report.
        return Err(RefreshRefusal::HelperInconclusive);
    }

    let rescans = std::mem::take(&mut collector.rescans);
    Ok((collector.changes.drain().collect(), rescans))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(event_id: Option<u64>, device: Option<u64>, uuid: Option<&str>) -> FsEventsState {
        FsEventsState {
            event_id,
            device,
            device_uuid: uuid.map(str::to_owned),
            ..FsEventsState::default()
        }
    }

    #[test]
    fn stream_uuid_allows_matching_cursor_and_rejects_missing_or_changed_identity() {
        let stored = cursor(Some(41), Some(7), Some("stream-a"));
        // The current event ID is later than the stored one; a matching
        // stream UUID is the evidence that makes this replay valid.
        assert_eq!(
            replay_cursor(&stored, 7, 99, Some("stream-a")).unwrap(),
            (41, "stream-a".into())
        );

        for stale in [
            cursor(Some(41), Some(7), None),
            cursor(Some(41), Some(7), Some("stream-old")),
        ] {
            let refused = replay_cursor(&stale, 7, 99, Some("stream-new")).unwrap_err();
            assert_eq!(refused.refusal, Some(RefreshRefusal::HelperInconclusive));
            assert_eq!(refused.device_uuid.as_deref(), Some("stream-new"));
            assert!(refused.changed_dirs.is_empty());
        }

        let unavailable = replay_cursor(&stored, 7, 99, None).unwrap_err();
        assert_eq!(
            unavailable.refusal,
            Some(RefreshRefusal::FseventsdUnavailable)
        );
    }

    #[test]
    fn cursor_planner_keeps_device_and_future_id_refusals() {
        let stored = cursor(Some(41), Some(7), Some("stream-a"));
        assert_eq!(
            replay_cursor(&stored, 8, 99, Some("stream-b"))
                .unwrap_err()
                .refusal,
            Some(RefreshRefusal::RootMismatch)
        );
        assert_eq!(
            replay_cursor(&stored, 7, 40, Some("stream-a"))
                .unwrap_err()
                .refusal,
            Some(RefreshRefusal::EventIdFromFuture)
        );
        assert_eq!(
            replay_cursor(
                &cursor(None, Some(7), Some("stream-a")),
                7,
                99,
                Some("stream-a")
            )
            .unwrap_err()
            .refusal,
            Some(RefreshRefusal::NoStoredEventId)
        );
    }

    #[test]
    fn full_anchor_requires_one_stream_identity_on_both_sides_of_event_id() {
        assert_eq!(
            full_anchor(88, 7, Some("stream-a".into()), Some("stream-a".into())),
            Some((88, 7, Some("stream-a".into())))
        );
        assert_eq!(
            full_anchor(
                88,
                7,
                Some("before-reset".into()),
                Some("after-reset".into())
            ),
            None
        );
        assert_eq!(full_anchor(88, 7, Some("stream-a".into()), None), None);
        assert_eq!(
            full_anchor(0, 7, Some("stream-a".into()), Some("stream-a".into())),
            None
        );
    }

    fn callback_collector() -> Collector {
        Collector {
            roots: vec![PathBuf::from("/tmp/swamp-root")],
            changes: Default::default(),
            rescans: Vec::new(),
            history_done: false,
            hard_fail: None,
        }
    }

    #[test]
    fn stream_callback_refuses_every_dropped_or_wrapped_history_flag() {
        let loss_flags = [
            fs::kFSEventStreamEventFlagEventIdsWrapped,
            fs::kFSEventStreamEventFlagUserDropped,
            fs::kFSEventStreamEventFlagKernelDropped,
        ];
        for loss in loss_flags {
            let mut collector = callback_collector();
            let path = std::ffi::CString::new("/tmp/swamp-root/changed").unwrap();
            let path_ptr = path.as_ptr();
            let paths = &path_ptr as *const *const std::os::raw::c_char;
            let flags = [loss];
            stream_callback(
                std::ptr::null_mut(),
                &mut collector as *mut Collector as *mut c_void,
                1,
                paths as *mut c_void,
                flags.as_ptr(),
                std::ptr::null(),
            );
            assert_eq!(
                collector.hard_fail,
                Some(RefreshRefusal::HelperInconclusive)
            );
            assert!(collector.changes.is_empty());
            assert!(collector.rescans.is_empty());
        }
    }

    #[test]
    fn matching_stream_cursor_still_collects_ordinary_changes() {
        let (since_id, _) = replay_cursor(
            &cursor(Some(41), Some(7), Some("stream-a")),
            7,
            99,
            Some("stream-a"),
        )
        .unwrap();
        assert_eq!(since_id, 41);

        let path = std::ffi::CString::new("/tmp/swamp-root/changed").unwrap();
        let path_ptr = path.as_ptr();
        let paths = &path_ptr as *const *const std::os::raw::c_char;
        let flags = [fs::kFSEventStreamEventFlagNone];
        let mut collector = callback_collector();
        stream_callback(
            std::ptr::null_mut(),
            &mut collector as *mut Collector as *mut c_void,
            1,
            paths as *mut c_void,
            flags.as_ptr(),
            std::ptr::null(),
        );
        assert!(
            collector
                .changes
                .contains(&PathBuf::from("/tmp/swamp-root/changed"))
        );
        assert!(collector.hard_fail.is_none());
    }

    #[test]
    fn stream_callback_keeps_scan_subdirs_and_root_change_refusals() {
        let path = std::ffi::CString::new("/tmp/swamp-root/changed").unwrap();
        let path_ptr = path.as_ptr();
        let paths = &path_ptr as *const *const std::os::raw::c_char;

        let mut collector = callback_collector();
        let flags = [fs::kFSEventStreamEventFlagMustScanSubDirs];
        stream_callback(
            std::ptr::null_mut(),
            &mut collector as *mut Collector as *mut c_void,
            1,
            paths as *mut c_void,
            flags.as_ptr(),
            std::ptr::null(),
        );
        assert_eq!(
            collector.rescans,
            vec![PathBuf::from("/tmp/swamp-root/changed")]
        );

        collector = callback_collector();
        let flags = [fs::kFSEventStreamEventFlagRootChanged];
        stream_callback(
            std::ptr::null_mut(),
            &mut collector as *mut Collector as *mut c_void,
            1,
            paths as *mut c_void,
            flags.as_ptr(),
            std::ptr::null(),
        );
        assert_eq!(collector.hard_fail, Some(RefreshRefusal::RootMismatch));
        assert!(collector.changes.is_empty());
    }
}
