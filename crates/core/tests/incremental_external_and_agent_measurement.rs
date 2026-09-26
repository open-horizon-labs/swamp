//! What an *unchanged* observation actually costs
//! (`.oh/guardrails/no-second-traversal-on-report-path.md`).
//!
//! The handoff requires unchanged work to scale with roots and changed
//! containers rather than with all files, and the 2026-09-21 review's P1
//! was that it does not: external roots were re-sized recursively on
//! every call, and every adapter re-read every session header. "Bounded
//! reads are enough" is not a measurement, so this file measures --
//! through `swamp_core::work_counters`, which counts directory listings,
//! stats and header bytes.
//!
//! These tests run single-threaded: the counters are process-global, and
//! two tests measuring at once would measure each other.
//!
//! Fixtures are disposable `tempfile` trees with synthetic content. No
//! real tool home, transcript or credential is read; the "session
//! headers" here are one-line JSON objects this test wrote itself.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::fs_events::EventCoverage;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

/// A trusted replay window over `root` that reports nothing changed --
/// what an ordinary unchanged pass has after its FSEvents replay
/// succeeded. `since` is the previous observation's time, which is what
/// `growth::stage_tracked_with_source` passes through.
fn quiet_window(root: &Path, since: u64) -> EventCoverage {
    EventCoverage::trusted(root.to_path_buf(), Vec::new(), since)
}

/// A trusted replay window that names exactly the paths the pass touched
/// (each reported path and its parent, as FSEvents answers).
fn window(root: &Path, changed: &[PathBuf], since: u64) -> EventCoverage {
    EventCoverage::trusted(root.to_path_buf(), changed.to_vec(), since)
}

/// Enough sessions that a per-session cost is unmistakable in the
/// counters. The review's objection to the existing evidence was
/// precisely that 300 sessions is not a large tool home.
const SESSIONS: usize = 5_000;

/// Enough files that a recursive re-measurement of an unchanged external
/// root would be obvious.
const EXTERNAL_FILES: usize = 20_000;

/// How many `projects/<encoded-cwd>/` container directories the
/// synthetic home has. The unit of unchanged work after the 2026-09-22
/// container-level reuse: every cost assertion below is a multiple of
/// this, never of `SESSIONS`.
const CONTAINERS: usize = 5;

/// The work counters are process-global, so two tests measuring at once
/// would measure each other. Serializing the file on one lock makes the
/// numbers trustworthy under the default harness rather than relying on
/// every caller passing `--test-threads=1`.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn only(keep: &[&str]) -> ScanConfig {
    let registry = Registry::with_builtins();
    ScanConfig {
        defaults: false,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !keep.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    }
}

fn scope_with(env_vars: HashMap<String, String>, home: &Path, keep: &[&str]) -> EffectiveScope {
    let env = Environment::fixture(home.to_path_buf(), env_vars, Platform::MacOS);
    resolve_effective_scope(&env, &only(keep), &[], &Registry::with_builtins(), 1_000)
}

/// A synthetic Claude Code home: `SESSIONS` transcripts across a handful
/// of project directories, each a single JSON header line.
fn synthetic_claude_home(root: &Path) -> PathBuf {
    claude_home_with(root, CONTAINERS, SESSIONS / CONTAINERS)
}

/// [`synthetic_claude_home`] with the shape chosen by the caller, so a
/// test can vary the container count while holding everything else --
/// including the home's own structure -- constant.
fn claude_home_with(root: &Path, containers: usize, per_container: usize) -> PathBuf {
    let home = root.join("claude");
    let projects = home.join("projects");
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    // A few containers, so "re-list only the changed container" is
    // measurable rather than vacuous.
    for bucket in 0..containers {
        let dir = projects.join(format!("-bucket-{bucket}"));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..per_container {
            let id = format!("{bucket:04}-{i:08}-4000-8000-000000000000");
            fs::write(
                dir.join(format!("{id}.jsonl")),
                format!(
                    "{{\"type\":\"user\",\"sessionId\":\"{id}\",\"cwd\":\"{}\"}}\n",
                    repo.display()
                ),
            )
            .unwrap();
        }
    }
    home
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, WorkCounters) {
    let before = work_counters::snapshot();
    let out = f();
    (out, work_counters::since(before))
}

fn observe_agents(
    scope: &EffectiveScope,
    store: &Path,
    at: u64,
    coverage: &EventCoverage,
) -> usize {
    swamp_core::agents::discover_and_measure(scope, &[], Some(store), true, at, 30, 3600, coverage)
        .expect("agent discovery")
        .len()
}

fn agent_bytes(scope: &EffectiveScope, store: &Path, at: u64, coverage: &EventCoverage) -> u64 {
    swamp_core::agents::discover_and_measure(scope, &[], Some(store), true, at, 30, 3600, coverage)
        .expect("agent discovery")
        .iter()
        .map(|u| u.bytes)
        .sum()
}

#[test]
fn a_large_agent_home_reads_headers_once_and_not_again() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = synthetic_claude_home(&root);
    let scope = scope_with(
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        &root,
        &["claude-code"],
    );
    let store = tempfile::tempdir().unwrap();

    let started = std::time::Instant::now();
    let (units, first) =
        measure(|| observe_agents(&scope, store.path(), 1_000, &EventCoverage::untrusted()));
    let first_elapsed = started.elapsed();
    assert!(
        units > 0,
        "precondition: the synthetic home must produce units"
    );
    assert!(
        first.header_bytes_read > 0,
        "the first observation has to read the session headers it identifies from"
    );

    let started = std::time::Instant::now();
    let (_, second) =
        measure(|| observe_agents(&scope, store.path(), 2_000, &quiet_window(&root, 1_000)));
    let second_elapsed = started.elapsed();

    // Reported rather than only asserted: a number in the session note
    // is worth more than a pass/fail here.
    println!(
        "agent home, {SESSIONS} sessions in {CONTAINERS} containers: first pass \
         {first_elapsed:?} ({} header bytes, {} dirs listed, {} files statted, {} containers \
         identified); unchanged second pass {second_elapsed:?} ({} header bytes, {} dirs \
         listed, {} files statted, {} containers reused)",
        first.header_bytes_read,
        first.dirs_listed,
        first.files_statted,
        first.containers_identified,
        second.header_bytes_read,
        second.dirs_listed,
        second.files_statted,
        second.containers_reused,
    );

    // THE CLOSED GAP. The previous repair measured, and recorded,
    // 740 KB of header reads on an *unchanged* second pass over this
    // fixture -- every one of the 5,000 sessions re-read to re-derive a
    // `cwd` that had not moved. The identification cache arrived with
    // the `AgentAdapter` trait, keyed on each session file's own
    // `(len, mtime_ns, ctime_ns, inode)` plus the adapter version, so an
    // unchanged session is a table lookup.
    //
    // This assertion is strict on purpose. "Fewer" would pass with a
    // cache that worked for four sessions in five, and the number this
    // replaces was exactly the kind of "bounded reads are enough" claim
    // the review rejected.
    assert_eq!(
        second.header_bytes_read, 0,
        "an unchanged agent home must cost zero header bytes: {} sessions, {} bytes on the \
         first pass, {} on the second",
        SESSIONS, first.header_bytes_read, second.header_bytes_read
    );
    // THE SECOND CLOSED GAP (2026-09-22, stack/12). Zero header bytes was
    // never the whole cost: the per-file identification cache's validity
    // key is each session file's own `(len, mtime_ns, ctime_ns, inode)`,
    // so *knowing* a session was unchanged still cost one `stat` per
    // session and one listing per container. The reviewer's cost report
    // measured 10,580 stats and 46 listings on an unchanged pass.
    //
    // Container-level reuse (`crate::agents::ContainerCache`) replays a
    // `projects/<encoded-cwd>/` whose watched directories this pass's
    // FSEvents window reports untouched. The assertions below are the
    // handoff's words turned into numbers: unchanged work scales with
    // *containers*, not with files. Both are strict multiples of the
    // container count -- "fewer than last time" would pass with a reuse
    // that worked for four containers in five.
    assert_eq!(
        second.containers_reused, CONTAINERS as u64,
        "every unchanged container must be replayed from the store: {} of {CONTAINERS}",
        second.containers_reused
    );
    assert_eq!(
        second.containers_identified, 0,
        "no container may be re-identified when nothing changed: {}",
        second.containers_identified
    );
    assert!(
        second.dirs_listed <= CONTAINERS as u64,
        "an unchanged agent home must not list more directories than it has containers: {} \
         listings over {CONTAINERS} containers and {SESSIONS} sessions",
        second.dirs_listed
    );
    // Since the gate became the event window, a replayed container costs
    // no `stat` at all; what remains is the entries of the handful of
    // home-level listings above. The bound that matters is that it does
    // not grow with `SESSIONS`: 10 x CONTAINERS is still two orders of
    // magnitude below one stat per session.
    assert!(
        second.files_statted <= 10 * CONTAINERS as u64,
        "an unchanged agent home must cost stats per container, not per session: {} stats \
         over {CONTAINERS} containers and {SESSIONS} sessions (first pass: {})",
        second.files_statted,
        first.files_statted
    );
    assert_eq!(
        second.identification_cache_hits, 0,
        "a replayed container makes no per-file derivation at all, so there is nothing for \
         the derivation cache to answer: {} hits",
        second.identification_cache_hits
    );
}

/// One appended session re-lists exactly its own container and leaves
/// every other container replayed. The counterpart to the test above:
/// reuse that cannot notice a change is not reuse, and reuse that
/// notices a change by re-identifying *everything* is not container
/// reuse.
#[test]
fn appending_one_session_re_identifies_exactly_one_container() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = synthetic_claude_home(&root);
    let scope = scope_with(
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        &root,
        &["claude-code"],
    );
    let store = tempfile::tempdir().unwrap();
    let (before, _) =
        measure(|| observe_agents(&scope, store.path(), 1_000, &EventCoverage::untrusted()));

    let repo = root.join("repo");
    let added = home.join("projects/-bucket-3/aaaa-99999999-4000-8000-000000000000.jsonl");
    fs::write(
        &added,
        format!("{{\"type\":\"user\",\"cwd\":\"{}\"}}\n", repo.display()),
    )
    .unwrap();

    // What a real replay would report for that write: the file and its
    // parent directory, and nothing else.
    let events = window(
        &root,
        &[added.clone(), home.join("projects/-bucket-3")],
        1_000,
    );
    let (after, third) = measure(|| observe_agents(&scope, store.path(), 3_000, &events));
    println!(
        "one appended session: {} containers identified, {} reused, {} dirs listed, {} files \
         statted, {} header bytes",
        third.containers_identified,
        third.containers_reused,
        third.dirs_listed,
        third.files_statted,
        third.header_bytes_read,
    );
    assert_eq!(after, before + 1, "the appended session must be reported");
    assert_eq!(
        third.containers_identified, 1,
        "exactly the changed container may be re-identified: {}",
        third.containers_identified
    );
    assert_eq!(
        third.containers_reused,
        CONTAINERS as u64 - 1,
        "every other container must still be replayed: {}",
        third.containers_reused
    );
    assert_eq!(
        third.identification_cache_misses, 1,
        "exactly one derivation may miss the per-file cache: the session that is actually new"
    );
}

/// A container whose identification *folds* directories, not only lists
/// them: Claude Code's per-session companion directory, `file-history/`,
/// and the always-present `memory/` leftover.
///
/// Two things this pins that the flat fixture above cannot. First, the
/// recorder has to survive `IdentifyCtx::folded_bytes` reaching back
/// into it for every directory the fold walked (a `RefCell` re-entry
/// away from a panic). Second, a directory *outside* the container that
/// its units depend on -- `file-history/<session-id>/` -- has to be
/// watched, or a session acquiring checkpoint data would be invisible
/// until something else in the project directory moved.
#[test]
fn a_container_that_folds_directories_records_them_and_notices_them() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("claude");
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let project = home.join("projects/-repo");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join(format!("{id}.jsonl")),
        format!("{{\"type\":\"user\",\"cwd\":\"{}\"}}\n", repo.display()),
    )
    .unwrap();
    fs::create_dir_all(project.join(id)).unwrap();
    fs::write(project.join(id).join("subagent.jsonl"), vec![b'a'; 1024]).unwrap();
    fs::create_dir_all(home.join("projects/-repo/memory")).unwrap();
    fs::write(
        home.join("projects/-repo/memory/MEMORY.md"),
        vec![b'm'; 512],
    )
    .unwrap();

    let scope = scope_with(
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        &root,
        &["claude-code"],
    );
    let store = tempfile::tempdir().unwrap();
    let total = |at: u64, coverage: &EventCoverage| -> u64 {
        agent_bytes(&scope, store.path(), at, coverage)
    };
    let first = total(1_000, &EventCoverage::untrusted());
    assert!(
        first >= 1536,
        "precondition: the folded members count: {first}"
    );

    let (second, cost) = measure(|| total(2_000, &quiet_window(&root, 1_000)));
    assert_eq!(second, first, "an unchanged home reports the same bytes");
    assert_eq!(
        cost.containers_reused, 1,
        "the container must be replayed, folds and all"
    );

    // `file-history/<session-id>/` did not exist when the container was
    // stored. The container *watches* that directory, so an event under
    // it must re-identify the container even though nothing inside
    // `projects/-repo` moved.
    let fh = home.join("file-history");
    fs::create_dir_all(fh.join(id)).unwrap();
    fs::write(fh.join(id).join("snap.json"), vec![b'f'; 4096]).unwrap();
    let events = window(&root, &[fh.join(id), fh.clone()], 2_000);
    let (third, cost) = measure(|| total(3_000, &events));
    assert_eq!(
        cost.containers_identified, 1,
        "a watched sibling directory gaining an entry must re-identify the container"
    );
    assert!(
        third >= first + 4096,
        "the new checkpoint data's bytes must be reported: {third} vs {first}"
    );
}

/// **The 2026-09-22 integration decision, asserted three ways.**
///
/// Until this chunk, container reuse was keyed on the container
/// directory's own `mtime`/`ctime`. A transcript **appended in place**
/// -- which is how a running agent writes, and therefore the normal case
/// for this catalog -- moves the file's own size and mtime and not its
/// parent's, so the growing session kept its stored byte total until
/// something else happened in that container. The integration owner
/// ruled that unacceptable for a growth tool: what grew must be seen.
///
/// Reuse is now gated on trusted event coverage, so all three branches
/// below report the appended bytes:
///
/// 1. under a replayed/live window that names the appended file, the
///    container is re-identified;
/// 2. under no window at all (a full walk, any refusal), nothing is
///    replayed and the container is re-identified anyway;
/// 3. a container the window vouches for is replayed, and -- because the
///    window has already answered the question directory stamps used to
///    be asked -- costs no listing and no `stat`.
#[test]
fn an_appended_session_is_seen_under_event_coverage_and_under_a_full_walk() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = synthetic_claude_home(&root);
    let scope = scope_with(
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        &root,
        &["claude-code"],
    );
    let store = tempfile::tempdir().unwrap();
    let bytes = |at: u64, coverage: &EventCoverage| -> u64 {
        agent_bytes(&scope, store.path(), at, coverage)
    };
    let first = bytes(1_000, &EventCoverage::untrusted());

    // (3) Nothing touched: replayed, and free.
    let (unchanged, cost) = measure(|| bytes(2_000, &quiet_window(&root, 1_000)));
    assert_eq!(unchanged, first, "an untouched home reports the same bytes");
    assert_eq!(
        cost.containers_reused, CONTAINERS as u64,
        "every untouched container must be replayed: {}",
        cost.containers_reused
    );
    // The residual work is the adapter's own home-level structure scan
    // (`~/.claude` itself, `projects/`, and the absent `file-history/`,
    // `image-cache/`, `uploads/`) -- none of it inside a container.
    // `replaying_containers_costs_nothing_per_container` below pins the
    // per-container cost at exactly zero by holding the home level
    // constant and varying only the container count.
    assert!(
        cost.dirs_listed <= 8 && cost.files_statted <= 8,
        "an untouched home's residual must be the home-level scan, not per-container work: \
         {} listings and {} stats over {CONTAINERS} containers",
        cost.dirs_listed,
        cost.files_statted
    );
    assert_eq!(
        cost.identification_cache_hits, 0,
        "a replayed container makes no per-file derivation at all: {} hits",
        cost.identification_cache_hits
    );

    // (1) Append 1 KiB to an existing transcript -- same name, same
    // directory, so the container's own stamp does not move.
    let victim = home.join("projects/-bucket-1/0001-00000000-4000-8000-000000000000.jsonl");
    let append = |bytes: usize| {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(&victim).unwrap();
        f.write_all(&vec![b'x'; bytes]).unwrap();
        f.write_all(b"\n").unwrap();
    };
    let before_len = fs::metadata(&victim).unwrap().len();
    append(1024);
    let after_len = fs::metadata(&victim).unwrap().len();
    assert!(
        after_len >= before_len + 1024,
        "precondition: the transcript must have grown by about a KiB ({before_len} -> \
         {after_len})"
    );

    let events = window(
        &root,
        &[victim.clone(), home.join("projects/-bucket-1")],
        2_000,
    );
    let (appended, cost) = measure(|| bytes(3_000, &events));
    assert_eq!(
        cost.containers_identified, 1,
        "the appended session's container must be re-identified, not replayed: {} identified, \
         {} reused",
        cost.containers_identified, cost.containers_reused
    );
    assert!(
        appended >= first + 1024,
        "an in-place append must be reported in the same pass that sees its event: {appended} \
         vs {first}"
    );

    // (2) The same append, with no window at all: a full walk reuses
    // nothing, so it sees the growth for the same reason a fresh
    // identification would.
    let store2 = tempfile::tempdir().unwrap();
    let base = agent_bytes(&scope, store2.path(), 1_000, &EventCoverage::untrusted());
    append(2048);
    let (walked, cost) =
        measure(|| agent_bytes(&scope, store2.path(), 2_000, &EventCoverage::untrusted()));
    assert_eq!(
        cost.containers_reused, 0,
        "a pass with no trusted window must replay nothing: {} reused",
        cost.containers_reused
    );
    assert!(
        walked >= base + 2048,
        "a full walk must report the appended bytes: {walked} vs {base}"
    );
}

/// Per-container reuse cost, pinned at exactly zero by holding the home
/// level constant and varying only the container count.
///
/// The test above cannot assert `(0, 0)` outright because an unchanged
/// pass still scans the tool home's own structure, which is real work
/// and is counted as such. This one subtracts that: two synthetic homes
/// that differ only in how many `projects/<encoded-cwd>/` containers
/// they have must cost the *same* number of listings and stats on a pass
/// their window vouches for. Any per-container `stat` -- the directory
/// stamp check this chunk removed, for instance -- shows up as a
/// difference.
#[test]
fn replaying_containers_costs_nothing_per_container() {
    let _serial = serial();
    let unchanged_cost = |containers: usize, per_container: usize| -> (u64, u64, u64) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let home = claude_home_with(&root, containers, per_container);
        let scope = scope_with(
            HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
            &root,
            &["claude-code"],
        );
        let store = tempfile::tempdir().unwrap();
        let _ = observe_agents(&scope, store.path(), 1_000, &EventCoverage::untrusted());
        let (_, cost) =
            measure(|| observe_agents(&scope, store.path(), 2_000, &quiet_window(&root, 1_000)));
        assert_eq!(
            cost.containers_reused, containers as u64,
            "precondition: every container must be replayed"
        );
        // Keep the tempdir alive until the measurement is done.
        drop(tmp);
        (cost.dirs_listed, cost.files_statted, cost.header_bytes_read)
    };
    // Sessions are free: the same containers holding twelve times as
    // many transcripts cost exactly the same. This is the handoff's
    // claim -- unchanged work does not scale with files -- and it is an
    // equality, not a bound.
    let small = unchanged_cost(3, 4);
    let dense = unchanged_cost(3, 48);
    assert_eq!(
        small, dense,
        "a replayed container must not cost anything per session: 4 sessions each cost \
         {small:?}, 48 each cost {dense:?}"
    );

    // Containers cost exactly one `stat` each, and it is not the
    // container's: it is the entry `projects/` yields when the home's
    // own structure is scanned, which is how this pass learns the
    // container exists at all. Zero extra listings, zero extra header
    // bytes, and -- since this chunk removed the directory-stamp check
    // -- zero stats of the container itself.
    let (one_dirs, one_stats, one_hdr) = unchanged_cost(1, 4);
    let (many_dirs, many_stats, many_hdr) = unchanged_cost(9, 4);
    assert_eq!(
        (one_dirs, one_hdr),
        (many_dirs, many_hdr),
        "eight more containers must add no listing and no header read"
    );
    assert_eq!(
        many_stats - one_stats,
        8,
        "eight more containers must add exactly their eight `projects/` entries and nothing \
         else: {one_stats} vs {many_stats}"
    );
}

#[test]
fn appending_one_session_reads_exactly_one_header() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = synthetic_claude_home(&root);
    let scope = scope_with(
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        &root,
        &["claude-code"],
    );
    let store = tempfile::tempdir().unwrap();
    let _ = measure(|| observe_agents(&scope, store.path(), 1_000, &EventCoverage::untrusted()));

    let repo = root.join("repo");
    let added = home.join("projects/-bucket-0/aaaa-99999999-4000-8000-000000000000.jsonl");
    fs::write(
        &added,
        format!("{{\"type\":\"user\",\"cwd\":\"{}\"}}\n", repo.display()),
    )
    .unwrap();

    let events = window(
        &root,
        &[added.clone(), home.join("projects/-bucket-0")],
        1_000,
    );
    let (_, third) = measure(|| observe_agents(&scope, store.path(), 3_000, &events));
    println!(
        "one appended session: {} header bytes, {} dirs listed",
        third.header_bytes_read, third.dirs_listed
    );
    assert!(
        third.header_bytes_read > 0,
        "the new session's header must be read"
    );
    // Exactly one header read: the appended session, and nothing else.
    // The bound is one capped read, not `SESSIONS + 1` of them -- the
    // weaker form would have passed while every unchanged session was
    // still being re-read.
    assert!(
        third.header_bytes_read <= swamp_core::agents::bounded_io::MAX_HEADER_BYTES as u64,
        "one appended session costs one capped header read; {} bytes means the unchanged \
         sessions were re-read too",
        third.header_bytes_read
    );
    assert_eq!(
        third.identification_cache_misses, 1,
        "exactly one derivation may miss the cache: the session that is actually new"
    );
}

#[test]
fn an_unchanged_external_cache_root_is_not_re_traversed() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let cargo_home = root.join("cargo-home");
    let registry = cargo_home.join("registry/cache/index.crates.io-abc");
    fs::create_dir_all(&registry).unwrap();
    for i in 0..EXTERNAL_FILES {
        fs::write(registry.join(format!("crate-{i}.crate")), b"x").unwrap();
    }
    let scope = scope_with(
        HashMap::from([("CARGO_HOME".to_string(), cargo_home.display().to_string())]),
        &root,
        &["cargo-home"],
    );
    let store = tempfile::tempdir().unwrap();

    let started = std::time::Instant::now();
    let (units, first) = measure(|| {
        swamp_core::external::discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            1_000,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .expect("external discovery")
        .len()
    });
    let first_elapsed = started.elapsed();
    assert!(units > 0, "precondition: the Cargo home must be measured");

    let started = std::time::Instant::now();
    let (_, second) = measure(|| {
        swamp_core::external::discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            2_000,
            30,
            3600,
            &quiet_window(&root, 1_000),
        )
        .expect("external discovery")
        .len()
    });
    let second_elapsed = started.elapsed();

    println!(
        "external cache root, {EXTERNAL_FILES} files: first pass {first_elapsed:?} \
         ({} dirs listed, {} files statted); second pass {second_elapsed:?} \
         ({} dirs listed, {} files statted)",
        first.dirs_listed, first.files_statted, second.dirs_listed, second.files_statted
    );

    assert!(
        first.dirs_listed > 0,
        "the first pass must be counted, or the counters are not wired"
    );
    // THE CLOSED GAP (2026-09-22). External unit bytes used to come from
    // a fresh recursive folded measurement on every pass; this file's
    // previous version asserted only that the work was *counted*,
    // deliberately, so the gap stayed visible. It is now closed:
    // `folded_measurement::reuse_folded_measurement` answers an
    // unchanged unit from the folded rows the previous pass persisted,
    // paying one `stat` per directory and no listing at all.
    //
    // Strict on purpose. "Fewer" would pass with a reuse that worked for
    // the small directories and re-walked the big one.
    assert_eq!(
        second.dirs_listed, 0,
        "an unchanged external root must not re-list a single directory: {} listings over {} \
         files",
        second.dirs_listed, EXTERNAL_FILES
    );
    // Since the gate became the event window (2026-09-22) the stat cost
    // is not the directory count either: it is zero. The claim the
    // handoff makes ("unchanged work scales with roots and changed
    // containers, not all files") is now literal.
    assert_eq!(
        second.files_statted, 0,
        "a unit the window vouches for costs no stat at all: {} stats over {} files (first \
         pass: {})",
        second.files_statted, EXTERNAL_FILES, first.files_statted
    );
}

/// The reuse must not survive a change it cannot see the inside of: a
/// file added under the unit costs a real re-measurement, and the new
/// bytes are reported.
#[test]
fn a_changed_external_cache_root_is_measured_again() {
    let _serial = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let cargo_home = root.join("cargo-home");
    let registry = cargo_home.join("registry/cache/index.crates.io-abc");
    fs::create_dir_all(&registry).unwrap();
    for i in 0..50 {
        fs::write(registry.join(format!("crate-{i}.crate")), b"x").unwrap();
    }
    let scope = scope_with(
        HashMap::from([("CARGO_HOME".to_string(), cargo_home.display().to_string())]),
        &root,
        &["cargo-home"],
    );
    let store = tempfile::tempdir().unwrap();
    let measure_pass = |at: u64, coverage: &EventCoverage| {
        swamp_core::external::discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            at,
            30,
            3600,
            coverage,
        )
        .expect("external discovery")
        .into_iter()
        .map(|u| u.bytes)
        .sum::<u64>()
    };
    let (first_bytes, _) = measure(|| measure_pass(1_000, &EventCoverage::untrusted()));
    let (_, second) = measure(|| measure_pass(2_000, &quiet_window(&root, 1_000)));
    assert_eq!(
        second.dirs_listed, 0,
        "precondition: the unchanged pass reuses"
    );

    let added = registry.join("added.crate");
    fs::write(&added, vec![b'x'; 200_000]).unwrap();
    let events = window(&root, &[added.clone(), registry.clone()], 2_000);
    let (third_bytes, third) = measure(|| measure_pass(3_000, &events));
    assert!(
        third.dirs_listed > 0,
        "a directory whose contents changed must be listed again"
    );
    assert!(
        third_bytes > first_bytes,
        "the added file's bytes must be reported: {third_bytes} vs {first_bytes}"
    );
}

#[test]
fn the_work_counters_themselves_are_wired() {
    let _serial = serial();
    // A guard against the whole file passing vacuously: if nothing
    // increments the counters, every assertion above is meaningless.
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a"), b"x").unwrap();
    let (_, counted) = measure(|| swamp_core::locations::shallow_list(tmp.path()));
    assert_eq!(counted.dirs_listed, 1);
    assert_eq!(counted.files_statted, 1);
}
