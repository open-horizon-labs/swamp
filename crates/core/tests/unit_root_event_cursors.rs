//! Per-unit-root FSEvents cursors: the external caches and agent tool
//! homes get their own replay anchors, independent of the folded walk's.
//!
//! One thing to know before reading these, because it contradicts the
//! premise the previous chunk recorded: a detector-resolved home **is**
//! a `Present` scan root and **is** walked
//! (`scope::resolve_effective_scope`, verified in
//! `a_detector_home_is_a_present_scan_root` below), so its own walk does
//! produce a window once it has an anchor. What the walk's window cannot
//! do is stay aligned with the *unit rows*: any pass that walks without
//! measuring units -- `ObservationParts::WALK_ONLY`, which is what the
//! TUI's background refresh uses -- advances that anchor past rows it
//! never refreshed, and `EventCoverage::unchanged_since` then correctly
//! refuses them. A cursor owned by the unit families does not move on
//! such a pass, which is the difference these tests measure.
//!
//! Everything here drives an injected `FsEventsSource`. A real replay
//! cannot be made deterministic (`fseventsd`'s own log lags a write by
//! longer than a whole second, which is what `TooSoon` exists for), and
//! the property under test is the *cursor lifecycle*, not CoreServices.
//! The lag-tolerant real-source coverage stays where it already is, in
//! `fsevents_incremental.rs`.
//!
//! Disposable `tempfile` fixtures throughout; every "session transcript"
//! is a one-line JSON object this file wrote itself. No real tool home,
//! transcript or credential is read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{ObservationParts, ScopeObservation, observe_scope};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

/// The work counters are process-global, so two tests measuring at once
/// would measure each other.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// A source that behaves the way the macOS one does for the cases these
/// tests need, and no more: it refuses a root with no stored anchor,
/// and otherwise answers incrementally with exactly the changes at or
/// under that root -- the same per-root split
/// `fs_events::partition_changes` performs on a shared stream, so a
/// change under one root can never appear in another's window.
struct ScriptedSource {
    /// The id every replay reports as "current"; the next pass stores it.
    current_event_id: u64,
    /// The device every root is reported on, unless `device_override`
    /// says otherwise for the root being asked about.
    device: u64,
    /// Every path this pass should report as changed, in canonical form.
    changed: Vec<PathBuf>,
}

impl ScriptedSource {
    fn quiet(current_event_id: u64) -> Self {
        Self {
            current_event_id,
            device: 1,
            changed: Vec::new(),
        }
    }
    fn with_changes(current_event_id: u64, changed: Vec<PathBuf>) -> Self {
        Self {
            current_event_id,
            device: 1,
            changed,
        }
    }
}

impl FsEventsSource for ScriptedSource {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        if request.since.event_id.is_none() {
            return FsEventsPlan {
                incremental: false,
                refusal: Some(swamp_core::fs_events::RefreshRefusal::NoStoredEventId),
                changed_dirs: Vec::new(),
                current_event_id: self.current_event_id,
                device: Some(self.device),
                live: false,
                consume: None,
            };
        }
        FsEventsPlan {
            incremental: true,
            refusal: None,
            changed_dirs: self
                .changed
                .iter()
                .filter(|c| c.starts_with(&request.root))
                .cloned()
                .collect(),
            current_event_id: self.current_event_id,
            device: Some(self.device),
            live: false,
            consume: None,
        }
    }
}

/// `~/.claude` with `containers` project directories, `per` transcripts
/// in each; `~/.cargo` with `cache_files` crate files; and a project
/// directory `src/` -- the detector homes being siblings of the project
/// root rather than under it, which is the default-install shape and the
/// one the reviewer's cost fixture uses.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    claude: PathBuf,
    cargo_home: PathBuf,
    src: PathBuf,
    store: tempfile::TempDir,
}

fn fixture(containers: usize, per: usize, cache_files: usize) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();

    let claude = root.join("claude");
    for p in 0..containers {
        let proj = claude.join(format!("projects/-Users-dev-proj{p}"));
        fs::create_dir_all(&proj).unwrap();
        for s in 0..per {
            fs::write(
                proj.join(format!("sess-{p}-{s}.jsonl")),
                format!("{{\"cwd\":\"/Users/dev/proj{p}\",\"type\":\"user\"}}\n"),
            )
            .unwrap();
        }
    }

    let cargo_home = root.join("cargo-home");
    let registry = cargo_home.join("registry/cache/index.crates.io-abc");
    fs::create_dir_all(&registry).unwrap();
    for i in 0..cache_files {
        fs::write(registry.join(format!("crate-{i}.crate")), b"x").unwrap();
    }

    let src = root.join("src");
    fs::create_dir_all(src.join("p/target/debug")).unwrap();
    fs::write(src.join("p/target/debug/blob.bin"), vec![b'x'; 65536]).unwrap();
    fs::write(src.join("p/Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();

    Fixture {
        _tmp: tmp,
        root,
        claude,
        cargo_home,
        src,
        store: tempfile::tempdir().unwrap(),
    }
}

impl Fixture {
    fn scope(&self) -> EffectiveScope {
        let registry = Registry::with_builtins();
        let keep = ["claude-code", "cargo-home"];
        let cfg = ScanConfig {
            defaults: false,
            include: vec![self.src.display().to_string()],
            disabled_detectors: registry
                .detectors()
                .iter()
                .map(|d| d.id().to_string())
                .filter(|id| !keep.contains(&id.as_str()))
                .collect(),
            ..Default::default()
        };
        let env = Environment::fixture(
            self.root.clone(),
            HashMap::from([
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    self.claude.display().to_string(),
                ),
                (
                    "CARGO_HOME".to_string(),
                    self.cargo_home.display().to_string(),
                ),
            ]),
            Platform::MacOS,
        );
        resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
    }

    fn observe(&self, source: &dyn FsEventsSource) -> ScopeObservation {
        self.observe_parts(source, ObservationParts::ALL, true)
    }

    fn observe_parts(
        &self,
        source: &dyn FsEventsSource,
        want: ObservationParts,
        observe: bool,
    ) -> ScopeObservation {
        observe_scope(
            &self.scope(),
            want,
            None,
            None,
            false,
            Some(self.store.path()),
            None,
            observe,
            false,
            false,
            false,
            source,
            30,
            24 * 3600,
        )
        .expect("scope observation")
    }

    fn agent_bytes(&self, o: &ScopeObservation) -> u64 {
        o.agent_units.iter().map(|u| u.bytes).sum()
    }
    fn external_bytes(&self, o: &ScopeObservation) -> u64 {
        o.external_units.iter().map(|u| u.bytes).sum()
    }
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, WorkCounters) {
    let before = work_counters::snapshot();
    let out = f();
    (out, work_counters::since(before))
}

/// No real sleeps: the `TooSoon` floor is a property of the design, and
/// the tests that are not *about* it set it to zero rather than waiting
/// three seconds each.
struct NoFloor(String);

impl NoFloor {
    fn set() -> Self {
        let prev = std::env::var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS").unwrap_or_default();
        unsafe { std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0") };
        Self(prev)
    }
}

impl Drop for NoFloor {
    fn drop(&mut self) {
        if self.0.is_empty() {
            unsafe { std::env::remove_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS") };
        } else {
            unsafe { std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", &self.0) };
        }
    }
}

/// The unit-root half of the agent home's `cursors.parquet`. The walk
/// writes the other row of the same table whenever that path is also a
/// scan root, so the two must be compared separately.
fn unit_cursor(fx: &Fixture) -> Option<swamp_core::fs_events::UnitRootCursor> {
    swamp_core::growth::read_fsevents_anchor(&swamp_core::growth::volume_store_dir(
        fx.store.path(),
        &fx.claude,
    ))
    .unit_root
}

fn reason_for<'a>(o: &'a ScopeObservation, root: &Path) -> &'a str {
    o.unit_root_coverage
        .iter()
        .find(|c| c.path == root)
        .map(|c| c.reason.as_str())
        .unwrap_or("<no coverage row>")
}

/// **The point of the chunk.** A 5,000-session agent home and a
/// 20,000-file cache that are detector roots *outside* the scan root
/// cost nothing on an unchanged pass once their cursors exist.
///
/// Three passes, because two cannot say it: pass 1 has no anchor
/// (`no_stored_event_id`) and pays for everything; pass 2 anchors the
/// cursors but its own window opened at pass 1's anchor, which did not
/// exist, so it still pays; pass 3 is the first one with a window to
/// vouch for rows that are no older than it. That is the "third pass
/// after the FSEvents lag" the brief names, and it is also exactly the
/// shape a real install has.
#[test]
fn an_unchanged_pass_over_out_of_scope_detector_roots_costs_nothing() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(5, 1_000, 20_000);

    let first = fx.observe(&ScriptedSource::quiet(10));
    let agent_bytes = fx.agent_bytes(&first);
    let external_bytes = fx.external_bytes(&first);
    assert!(agent_bytes > 0, "precondition: the agent home has units");
    assert!(external_bytes > 0, "precondition: the cache has units");
    assert_eq!(
        reason_for(&first, &fx.claude),
        "no_stored_event_id",
        "a first pass has no anchor to replay from"
    );

    // Pass 2: the cursors now exist, but each one's window opens at the
    // instant the *previous* pass recorded -- and the previous pass
    // recorded none. Still full cost, honestly.
    let second = fx.observe(&ScriptedSource::quiet(20));
    assert_eq!(
        reason_for(&second, &fx.claude),
        "incremental",
        "the second pass replays, even though the rows it finds are not yet reusable"
    );

    // Pass 3: a window that opens before the rows were written.
    let (third, cost) = measure(|| fx.observe(&ScriptedSource::quiet(30)));
    assert_eq!(
        fx.agent_bytes(&third),
        agent_bytes,
        "a replayed agent home reports what it was measured at"
    );
    assert_eq!(
        fx.external_bytes(&third),
        external_bytes,
        "a replayed cache reports what it was measured at"
    );
    assert!(
        third.unit_root_coverage.iter().all(|c| c.event_covered),
        "every authorized unit root must be event-covered: {:?}",
        third.unit_root_coverage
    );
    // And the report says so, so a surface can explain the cost.
    assert!(
        third
            .merged
            .notes
            .iter()
            .any(|n| n.contains("unit roots:") && n.contains("event-covered")),
        "the report must carry the coverage fact: {:?}",
        third.merged.notes
    );
    assert_eq!(
        cost.header_bytes_read, 0,
        "a replayed container reads no session headers"
    );
    assert_eq!(
        cost.containers_reused, 5,
        "every container replayed: {} reused, {} identified",
        cost.containers_reused, cost.containers_identified
    );
    // The scan root (`src/`) is still walked on every pass -- that is
    // the folded walk, not unit work -- so the residual is bounded, not
    // zero. What must be zero is per-file work in the two unit roots:
    // 25,000 files between them.
    assert!(
        cost.files_statted < 200,
        "an unchanged pass must not re-stat 25,000 unit files: {} stats, {} listings",
        cost.files_statted,
        cost.dirs_listed
    );
}

/// A transcript appended in place is seen, cursors or no cursors: the
/// window names the file, so the container it belongs to is
/// re-identified and its new bytes reported. This is the property the
/// whole event gate exists for, re-asserted through the unit-root
/// cursors rather than through a hand-built `EventCoverage`.
#[test]
fn an_appended_transcript_is_seen_through_the_unit_root_cursor() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(3, 5, 10);

    let first = fx.observe(&ScriptedSource::quiet(10));
    let before = fx.agent_bytes(&first);
    let _ = fx.observe(&ScriptedSource::quiet(20));

    let victim = fx.claude.join("projects/-Users-dev-proj1/sess-1-0.jsonl");
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(&victim).unwrap();
        f.write_all(&vec![b'x'; 4096]).unwrap();
    }

    let source = ScriptedSource::with_changes(
        30,
        vec![victim.clone(), victim.parent().unwrap().to_path_buf()],
    );
    let (after, cost) = measure(|| fx.observe(&source));
    assert!(
        fx.agent_bytes(&after) >= before + 4096,
        "the appended bytes must be reported: {} vs {before}",
        fx.agent_bytes(&after)
    );
    assert_eq!(
        cost.containers_identified, 1,
        "only the container the window named is re-identified: {} identified, {} reused",
        cost.containers_identified, cost.containers_reused
    );
    assert_eq!(
        cost.containers_reused, 2,
        "the two quiet containers are still replayed"
    );
}

/// Two authorized roots on one device are replayed through one stream,
/// and a change under one must never read as a change under the other.
///
/// The split itself is unit-tested in `fs_events`; what this asserts is
/// the consequence at the pipeline level, which is the thing a wrong
/// split would actually break: the cache re-measures, the agent home
/// still replays, on the same pass.
#[test]
fn two_roots_on_one_device_do_not_cross_talk() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(3, 5, 200);

    let first = fx.observe(&ScriptedSource::quiet(10));
    let cache_bytes = fx.external_bytes(&first);
    let _ = fx.observe(&ScriptedSource::quiet(20));

    // Grow the Cargo cache only. The agent home is untouched.
    let registry = fx.cargo_home.join("registry/cache/index.crates.io-abc");
    fs::write(registry.join("crate-new.crate"), vec![b'x'; 8192]).unwrap();
    let source =
        ScriptedSource::with_changes(30, vec![registry.join("crate-new.crate"), registry.clone()]);
    let (third, cost) = measure(|| fx.observe(&source));

    assert!(
        fx.external_bytes(&third) > cache_bytes,
        "the root that changed must be re-measured: {} vs {cache_bytes}",
        fx.external_bytes(&third)
    );
    assert_eq!(
        cost.containers_reused, 3,
        "the agent home shares the stream but saw none of the cache's events: \
         {} reused, {} identified",
        cost.containers_reused, cost.containers_identified
    );
    assert_eq!(
        cost.header_bytes_read, 0,
        "and therefore read no session headers"
    );
    // Both roots are still event-covered: coverage says the window was
    // usable, not that nothing changed.
    assert!(
        third.unit_root_coverage.iter().all(|c| c.event_covered),
        "{:?}",
        third.unit_root_coverage
    );
}

/// A root whose device differs from the one its cursor was recorded
/// against gets no window and says so, and its units are re-measured.
///
/// Asserted by rewriting the stored cursor's device, which is the only
/// way to reach this state without a second volume. The check lives in
/// `growth::replay_unit_roots` rather than in the platform source
/// precisely so it holds for every source -- an injected one included.
#[test]
fn a_device_mismatch_re_measures_and_names_the_reason() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(3, 5, 10);

    let _ = fx.observe(&ScriptedSource::quiet(10));
    let second = fx.observe(&ScriptedSource::quiet(20));
    assert_eq!(reason_for(&second, &fx.claude), "incremental");

    // The agent home's cursor now claims a different volume.
    let dir = swamp_core::growth::volume_store_dir(fx.store.path(), &fx.claude);
    let mut state = swamp_core::growth::read_fsevents_anchor(&dir);
    let unit = state
        .unit_root
        .as_mut()
        .expect("the cursor must have been written");
    unit.device = Some(999_999);
    swamp_core::growth::write_fsevents_anchor(&dir, &state).unwrap();

    // The window itself, not just its label: a mismatched cursor vouches
    // for nothing under its root, while its neighbour still does. (The
    // end-to-end reuse count cannot say this on its own, because a tool
    // home that is also a `Present` scan root has a second, legitimate
    // window from its own walk -- see the session note. This asserts the
    // cursor layer's own answer.) Staged and dropped, so the assertion
    // below still sees the corrupted cursor.
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &fx.scope().authorized_unit_roots(),
        3_000,
        false,
        &ScriptedSource::quiet(30),
    );
    assert!(!replay.covered(&fx.claude), "{:?}", replay.outcomes);
    assert!(
        !replay
            .coverage
            .unchanged_since(&fx.claude.join("projects/-Users-dev-proj0"), 1_000),
        "a cursor that addresses another filesystem may not vouch for anything"
    );
    assert!(replay.covered(&fx.cargo_home), "{:?}", replay.outcomes);
    drop(replay);

    let third = fx.observe(&ScriptedSource::quiet(40));
    assert_eq!(
        reason_for(&third, &fx.claude),
        "root_mismatch",
        "the reason must name the device, not go quiet: {:?}",
        third.unit_root_coverage
    );
    // The *other* root is unaffected: one root's mismatch is not a
    // scope-wide refusal.
    assert_eq!(reason_for(&third, &fx.cargo_home), "incremental");
    // ...and the pass re-anchors it, so a mismatch costs one
    // re-measurement rather than condemning the root forever.
    let fourth = fx.observe(&ScriptedSource::quiet(50));
    assert_eq!(reason_for(&fourth, &fx.claude), "incremental");
}

/// **The case the walk's own window cannot cover.**
///
/// A `WALK_ONLY` pass -- what the TUI's background refresh does -- moves
/// the walked root's FSEvents anchor forward without refreshing any unit
/// row. The next full pass's *walk* window therefore opens after the
/// stored unit rows were written, and `EventCoverage::unchanged_since`
/// correctly refuses to reuse them: a change in that gap is invisible to
/// both. Left there, a TUI that refreshes between full passes would
/// permanently prevent unit reuse.
///
/// The unit-root cursor does not advance on a pass that measured no
/// units, so its window still opens where the rows were written and the
/// reuse is honest. Asserted at the family seam, where the observation
/// times are the test's to choose rather than the wall clock's.
#[test]
fn a_walk_only_pass_between_full_passes_keeps_the_unit_window_aligned() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(3, 5, 10);
    let scope = fx.scope();
    let roots = scope.authorized_unit_roots();

    // T=1000: the rows are written, and the cursors anchor here.
    let units = swamp_core::agents::discover_and_measure(
        &scope,
        &[],
        Some(fx.store.path()),
        true,
        1_000,
        30,
        24 * 3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .expect("first agent measurement");
    assert!(!units.is_empty(), "precondition: the home has units");
    swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        1_000,
        false,
        &ScriptedSource::quiet(10),
    )
    .commit()
    .unwrap();

    // T=2000: a walk-only pass. Its window opens at 2000; no unit row
    // was refreshed, and the unit cursors were not committed.
    let walk_window =
        swamp_core::fs_events::EventCoverage::trusted(fx.claude.clone(), Vec::new(), 2_000);

    // T=3000, walk window alone: the rows (1000) predate it, so nothing
    // may be replayed. This is the control -- it is what the pipeline
    // did before the cursors existed. It does not observe, so the rows
    // it refuses to reuse are still the ones the next step is about.
    let (_, cost) = measure(|| {
        swamp_core::agents::discover_and_measure(
            &scope,
            &[],
            Some(fx.store.path()),
            false,
            3_000,
            30,
            24 * 3600,
            &walk_window,
        )
        .expect("agent measurement")
    });
    assert_eq!(
        cost.containers_reused, 0,
        "control: a window that opened after the rows were written vouches for nothing"
    );

    // T=4000, the same walk window plus the unit-root cursors, which
    // still open at 1000 because no pass since then measured a unit.
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        4_000,
        false,
        &ScriptedSource::quiet(20),
    );
    let mut both = walk_window.clone();
    both.merge(replay.coverage.clone());
    let (_, cost) = measure(|| {
        swamp_core::agents::discover_and_measure(
            &scope,
            &[],
            Some(fx.store.path()),
            true,
            4_000,
            30,
            24 * 3600,
            &both,
        )
        .expect("agent measurement")
    });
    assert_eq!(
        cost.containers_reused, 3,
        "the unit cursor's window still covers the rows it was anchored with: \
         {} reused, {} identified",
        cost.containers_reused, cost.containers_identified
    );
    assert_eq!(cost.header_bytes_read, 0, "and reads no headers");
}

/// Two passes back to back get no window, whatever the source says. The
/// floor exists because FSEvents' own persisted log can lag a write by
/// longer than a whole second, so a replay this soon cannot yet tell
/// "nothing changed" from "the change has not been logged yet".
///
/// This is the one test that does **not** disable the floor, and it is
/// why the reviewer's back-to-back cost fixture still re-measures.
#[test]
fn a_back_to_back_pass_refuses_reuse() {
    let _serial = serial();
    let fx = fixture(2, 3, 10);
    let _ = fx.observe(&ScriptedSource::quiet(10));
    let second = fx.observe(&ScriptedSource::quiet(20));
    assert_eq!(
        reason_for(&second, &fx.claude),
        "too_soon",
        "{:?}",
        second.unit_root_coverage
    );
    let (third, cost) = measure(|| fx.observe(&ScriptedSource::quiet(30)));
    assert_eq!(reason_for(&third, &fx.claude), "too_soon");
    assert_eq!(
        cost.containers_reused, 0,
        "a refused window reuses nothing at all"
    );
}

/// A pass that does not observe, or that covers only one unit family,
/// must not advance the cursors.
///
/// Advancing would move the next pass's window past changes the
/// unrefreshed family never measured. The `observed_at` guard would then
/// refuse to reuse those rows anyway, so advancing costs a guaranteed
/// re-measurement and buys nothing; not advancing costs a slightly wider
/// replay. This asserts the choice rather than leaving it to be
/// rediscovered.
#[test]
fn a_partial_pass_does_not_advance_the_cursors() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(2, 3, 10);
    let _ = fx.observe(&ScriptedSource::quiet(10));
    let after_first = unit_cursor(&fx);
    assert!(
        after_first.is_some(),
        "precondition: the first pass anchored a cursor"
    );

    // Walk only: no unit family observed at all. The walk rewrites the
    // same control file for its own anchor, so this compares the
    // unit-root half rather than the file.
    let _ = fx.observe_parts(
        &ScriptedSource::quiet(20),
        ObservationParts::WALK_ONLY,
        true,
    );
    assert_eq!(
        unit_cursor(&fx),
        after_first,
        "a walk-only pass measured no units, so it may not vouch for any"
    );

    // One family only.
    let _ = fx.observe_parts(&ScriptedSource::quiet(30), ObservationParts::AGENTS, true);
    assert_eq!(
        unit_cursor(&fx),
        after_first,
        "a pass that skipped the external family may not advance either root's cursor"
    );

    // Both families, but not observing: nothing was persisted.
    let _ = fx.observe_parts(&ScriptedSource::quiet(40), ObservationParts::ALL, false);
    assert_eq!(
        unit_cursor(&fx),
        after_first,
        "a non-observing pass wrote no rows, so the anchor must not move"
    );

    // A full observing pass does advance it.
    let _ = fx.observe(&ScriptedSource::quiet(50));
    assert_ne!(
        unit_cursor(&fx),
        after_first,
        "a complete pass must re-anchor, or the window would grow without bound"
    );
}

/// A pass that fails after measuring leaves the cursor where it was, so
/// the next pass re-measures instead of reusing rows the failed pass
/// never persisted.
///
/// Staged-then-dropped is exactly what the failure path does:
/// `observe_scope` calls `commit` only on the success branch, and the
/// `UnitRootReplay` is dropped otherwise. Asserted at that seam because
/// it is the seam -- and then confirmed end to end by the next pass
/// refusing to replay.
#[test]
fn a_failed_pass_does_not_advance_the_cursor() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(2, 3, 10);
    let _ = fx.observe(&ScriptedSource::quiet(10));
    let anchored = unit_cursor(&fx);

    let roots = fx.scope().authorized_unit_roots();
    assert!(!roots.is_empty(), "precondition: the scope resolved roots");

    // A pass that replays, measures, and then fails: the staged cursors
    // are dropped, never committed.
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        2_000,
        false,
        &ScriptedSource::quiet(20),
    );
    assert!(
        replay.covered(&fx.claude),
        "precondition: this pass did earn a window: {:?}",
        replay.outcomes
    );
    drop(replay);
    assert_eq!(
        unit_cursor(&fx),
        anchored,
        "a dropped checkpoint must leave the previous anchor in place"
    );

    // And committing publishes it, so the assertion above is about the
    // gate rather than about a writer that never runs.
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        3_000,
        false,
        &ScriptedSource::quiet(20),
    );
    replay.commit().unwrap();
    assert_ne!(unit_cursor(&fx), anchored);
}

/// `--full` never replays history. A source without pre-walk baseline
/// support cannot manufacture a persistent anchor.
#[test]
fn a_forced_full_pass_skips_replay_and_keeps_no_cursor_without_anchor_support() {
    let _serial = serial();
    let _floor = NoFloor::set();
    struct Exploding;
    impl FsEventsSource for Exploding {
        fn replay(&self, _request: &FsEventsRequest) -> FsEventsPlan {
            panic!("a forced full pass must not reach the FSEvents source");
        }
    }
    let fx = fixture(2, 3, 10);
    let roots = fx.scope().authorized_unit_roots();
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        1_000,
        true,
        &Exploding,
    );
    assert!(
        replay.outcomes.iter().all(|(_, r)| r == "full_forced"),
        "{:?}",
        replay.outcomes
    );
    replay.commit().unwrap();
    assert!(
        unit_cursor(&fx).is_none(),
        "a forced full pass anchors nothing"
    );
}

/// A root with no FSEvents at all (the non-macOS contract) gets no
/// window, a `unsupported_platform` reason, and no stored cursor --
/// "continuity unavailable, full re-measure", stated rather than
/// silently degraded.
#[test]
fn a_platform_without_fsevents_re_measures_and_says_so() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(2, 3, 10);
    let roots = fx.scope().authorized_unit_roots();
    let replay = swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        1_000,
        false,
        &swamp_core::fs_events::UnsupportedPlatformSource,
    );
    #[cfg(target_os = "linux")]
    let expected_reason = "no_persisted_change_history";
    #[cfg(not(target_os = "linux"))]
    let expected_reason = "unsupported_platform";
    assert!(
        replay.outcomes.iter().all(|(_, r)| r == expected_reason),
        "{:?}",
        replay.outcomes
    );
    assert!(replay.coverage.is_empty(), "no window is earned");
    replay.commit().unwrap();
    assert!(
        unit_cursor(&fx).is_none(),
        "an anchor that cannot be replayed from is not stored"
    );
}

/// The walk's anchor and the unit-root anchor share one `fsevents.json`
/// when a tool home is also a scan root. Neither writer may erase the
/// other's half: the visible symptom would be a unit that re-measures
/// every pass with no stated reason.
#[test]
fn the_walk_and_the_unit_cursor_share_one_file_without_erasing_each_other() {
    let _serial = serial();
    let _floor = NoFloor::set();
    let fx = fixture(2, 3, 10);
    let roots = vec![fx.claude.clone()];

    // The unit cursor first...
    swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        1_000,
        false,
        &ScriptedSource::quiet(10),
    )
    .commit()
    .unwrap();

    // ...then a walk of the same path, which writes the same file.
    // Twice: the first walk of a store the unit cursor created finds
    // `rules_version` unset and takes the full-walk branch, which
    // deliberately re-stamps nothing but the rules version.
    for (at, id) in [(2_000u64, 20u64), (3_000, 30)] {
        swamp_core::growth::observe_tracked_with_source(
            &swamp_core::bus::Stage::for_tests(),
            fx.store.path(),
            &fx.claude,
            at,
            1 << 30,
            false,
            true,
            &ScriptedSource::quiet(id),
            &[],
        )
        .expect("walk");
    }

    let volume = swamp_core::growth::volume_store_dir(fx.store.path(), &fx.claude);
    let state = swamp_core::growth::read_fsevents_anchor(&volume);
    assert!(
        state
            .unit_root
            .as_ref()
            .is_some_and(|u| u.event_id.is_some()),
        "the walk must carry the unit anchor through: {state:?}"
    );
    assert!(state.event_id.is_some(), "and keep its own: {state:?}");

    // And the other way round.
    swamp_core::growth::replay_unit_roots(
        Some(fx.store.path()),
        &roots,
        4_000,
        false,
        &ScriptedSource::quiet(40),
    )
    .commit()
    .unwrap();
    let state = swamp_core::growth::read_fsevents_anchor(&volume);
    assert!(
        state.last_observed_at.is_some(),
        "the unit cursor must carry the walk's anchor through: {state:?}"
    );
}

/// The premise correction, asserted so nobody has to rediscover it: a
/// detector-resolved home is a `Present` root of the scope and is walked
/// like any other, and the nested locations it contributes are
/// `SkippedAsNested` (folded into its walk, measured as their own
/// units).
///
/// The previous chunk's note reasoned from "`~/.claude` is under no scan
/// root", which is not what the scope resolves. The cursors are still
/// worth having -- see
/// `a_walk_only_pass_between_full_passes_keeps_the_unit_window_aligned`
/// -- but for a different reason than the one recorded, and a wrong
/// premise left standing is how the next chunk gets planned wrong.
#[test]
fn a_detector_home_is_a_present_scan_root() {
    let fx = fixture(1, 1, 1);
    let scope = fx.scope();
    let present: Vec<&PathBuf> = scope
        .roots
        .iter()
        .filter(|r| matches!(r.status, swamp_core::scope::RootStatus::Present))
        .map(|r| &r.path)
        .collect();
    assert!(
        present.contains(&&fx.claude),
        "the agent home is walked, not only measured: {present:?}"
    );
    assert!(
        present.contains(&&fx.cargo_home),
        "so is the Cargo home: {present:?}"
    );
    assert!(
        present.contains(&&fx.src),
        "alongside the configured project root: {present:?}"
    );
    // And the nested detector locations are folded into the Cargo home's
    // walk rather than walked again.
    assert!(
        scope
            .roots
            .iter()
            .any(|r| r.path == fx.cargo_home.join("registry/cache")
                && matches!(
                    r.status,
                    swamp_core::scope::RootStatus::SkippedAsNested { .. }
                )),
        "{:?}",
        scope.roots.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}
