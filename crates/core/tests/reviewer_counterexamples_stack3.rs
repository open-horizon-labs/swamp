//! Independent re-review 3 (stack/14 tip, PRs #116-#129).
//!
//! Each test asserts the behaviour the stack's own guardrails and
//! session notes require, not the behaviour the code has. Disposable
//! `tempfile` fixtures only; nothing reads a real agent or editor home,
//! a real tool store, or the user's own swamp state. No production code
//! is modified by anything in this file.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

// ---------------------------------------------------------------------
// CE1 -- the spawn counter is self-reported and nothing pairs it with
// the spawn.
// ---------------------------------------------------------------------

/// The stack replaced the reviewer's own out-of-process oracle with
/// swamp's own instrumentation.
///
/// `reviewer_counterexamples_stack2::a_disabled_detector_must_not_probe_its_tool`
/// as delivered put a directory of fake `docker`/`lsof`/... scripts at
/// the front of `PATH` and counted the lines they appended to a log:
/// an oracle **outside** the program, which no change to swamp could
/// make lie. The version in the tree replaces it with
/// `work_counters::measured` + `counted.subprocess_spawns`, which counts
/// only the spawns that call `work_counters::record_spawn`.
///
/// Today every `Command::new` in `swamp-core` does call it -- which is
/// why the substantive claim still holds when re-measured with the
/// original PATH-shim oracle (`rr3_cost.rs`: zero spawns over four
/// passes). But the pairing is a habit, not a structure: no audit, no
/// test and no type requires it, so the first `Command::new` written
/// without it makes `subprocess_spawns == 0` true and meaningless --
/// which is precisely the failure the 2026-09-22 re-review found in
/// these same counters ("a 20,000-file traversal reporting 2 dirs
/// listed").
///
/// Required behaviour: every process spawn in the workspace is counted,
/// checkably.
#[test]
fn every_command_new_must_record_a_spawn() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut unpaired: Vec<String> = Vec::new();
    for krate in ["core", "cli", "tui"] {
        let dir = root.join(format!("crates/{krate}/src"));
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let text = fs::read_to_string(&p).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.contains("Command::new(") || line.trim_start().starts_with("//") {
                        continue;
                    }
                    // The counter call may sit on any of the few lines
                    // before the spawn (the real code puts it
                    // immediately above).
                    let from = i.saturating_sub(4);
                    let paired = lines[from..=i].iter().any(|l| l.contains("record_spawn"));
                    if !paired {
                        unpaired.push(format!(
                            "{}:{}: {}",
                            p.strip_prefix(&root).unwrap_or(&p).display(),
                            i + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        unpaired.is_empty(),
        "these process spawns are invisible to `work_counters::subprocess_spawns`, which is now \
         the only oracle the disabled-detector counterexample has:\n  {}",
        unpaired.join("\n  ")
    );
}

// ---------------------------------------------------------------------
// CE2 -- a vendored citation excerpt is pinned to the manifest, not to
// upstream, and a row's citation is matched to an excerpt by path
// suffix.
// ---------------------------------------------------------------------

/// `upstream_citations_are_checked.rs` is a real improvement on the
/// 30-character length check, and it catches the mutation the brief
/// names (an excerpt that no longer contains its symbol fails, both on
/// the blake3 and on the symbol).
///
/// What it cannot catch: the excerpt is vendored *by the author of the
/// claim*, and the blake3 pins the excerpt to the manifest rather than
/// the excerpt to upstream. An excerpt written by hand, with the digest
/// recomputed, passes every check in the file.
///
/// The checkable half of that is the matching rule: a `Supported` row's
/// citation is matched to a vendored excerpt with
/// `tail.ends_with(&c.path) || c.path.ends_with(tail) ||
/// v.source.contains(&c.path)`. Two upstream files whose paths share a
/// suffix (`.../storage.ts` and `.../src/storage.ts`, `.../index.d.ts`)
/// are interchangeable, so a row can be "vendored" by another row's
/// file, whose symbols say nothing about this row's claim.
///
/// Required behaviour: no manifest path may be a suffix of another, so
/// the row -> excerpt mapping is unambiguous.
#[test]
fn no_vendored_citation_path_is_a_suffix_of_another() {
    let manifest =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/citations.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", manifest.display()));
    let paths: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("path ="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .collect();
    assert!(
        !paths.is_empty(),
        "citations.toml lists no `path =` entries"
    );
    let mut ambiguous: Vec<String> = Vec::new();
    for a in &paths {
        for b in &paths {
            if a != b && (a.ends_with(b.as_str()) || b.ends_with(a.as_str())) {
                ambiguous.push(format!("`{a}` and `{b}` match each other by suffix"));
            }
        }
    }
    ambiguous.sort();
    ambiguous.dedup();
    assert!(
        ambiguous.is_empty(),
        "a Supported row is matched to its vendored excerpt by path suffix, so these pairs are \
         interchangeable and a row can be evidenced by another row's file:\n  {}",
        ambiguous.join("\n  ")
    );
}

// ---------------------------------------------------------------------
// CE3 -- a rewrite in place inside a reused unit.
// ---------------------------------------------------------------------

fn small_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let claude = root.join("claude");
    let proj = claude.join("projects/-Users-dev-proj0");
    fs::create_dir_all(&proj).unwrap();
    for s in 0..20 {
        fs::write(
            proj.join(format!("sess-{s}.jsonl")),
            format!("{{\"cwd\":\"/Users/dev/proj0\",\"type\":\"user\"}}\n"),
        )
        .unwrap();
    }
    let cargo_home = root.join("cargo-home");
    let registry = cargo_home.join("registry/cache/index.crates.io-abc");
    fs::create_dir_all(&registry).unwrap();
    for i in 0..20 {
        fs::write(registry.join(format!("crate-{i}.crate")), vec![b'x'; 1024]).unwrap();
    }
    (claude, cargo_home)
}

fn small_scope(root: &Path, claude: &Path, cargo_home: &Path) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let keep = ["claude-code", "cargo-home"];
    let cfg = ScanConfig {
        defaults: false,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !keep.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(
        root.to_path_buf(),
        HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                claude.display().to_string(),
            ),
            ("CARGO_HOME".to_string(), cargo_home.display().to_string()),
        ]),
        Platform::MacOS,
    );
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn observe_units(scope: &EffectiveScope, store: &Path) -> u64 {
    let o = swamp_core::report::observe_scope(
        scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("scope observation");
    o.external_units.iter().map(|u| u.bytes).sum::<u64>()
        + o.agent_units.iter().map(|u| u.bytes).sum::<u64>()
}

/// `folded_measurement::reuse_folded_measurement`'s doc comment says the
/// event gate detects "writes into existing files", which is the whole
/// reason stamp-only reuse was removed on 2026-09-22.
///
/// This drives the fixture to the warm state (three spaced
/// observations -- see `rr3_cost.rs` for why it takes three), then
/// **rewrites an existing crate file in place with more bytes** and
/// observes again. The next observation must report the new total.
#[test]
fn an_in_place_rewrite_inside_a_reused_unit_must_be_measured() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (claude, cargo_home) = small_fixture(&root);
    let store = tempfile::tempdir().unwrap();
    let scope = small_scope(&root, &claude, &cargo_home);

    let mut warm = 0u64;
    for pass in 0..3 {
        if pass > 0 {
            std::thread::sleep(std::time::Duration::from_millis(3_500));
        }
        warm = observe_units(&scope, store.path());
    }

    // An in-place rewrite of an existing file: same path, same inode,
    // more bytes. No directory entry is created, removed or renamed, so
    // no directory stamp moves.
    let victim = cargo_home
        .join("registry/cache/index.crates.io-abc")
        .join("crate-3.crate");
    use std::os::unix::fs::MetadataExt;
    // Allocated blocks, because the folded measurement counts allocated
    // bytes: a 1 KiB file occupies a whole block.
    let before = fs::metadata(&victim).unwrap().blocks() * 512;
    fs::write(&victim, vec![b'y'; 1024 * 1024]).unwrap();
    let after = fs::metadata(&victim).unwrap().blocks() * 512;
    let grew_by = after - before;

    std::thread::sleep(std::time::Duration::from_millis(3_500));
    let seen = observe_units(&scope, store.path());

    assert!(
        seen >= warm + grew_by,
        "an in-place rewrite inside a reused unit was not measured: warm total {warm}, after a \
         {grew_by}-byte in-place rewrite the observation still reports {seen}"
    );
}

// ---------------------------------------------------------------------
// CE4 -- a shared-blob reference count that is quietly short.
// ---------------------------------------------------------------------

/// `agents::ContainerFacts`' own doc comment states the rule:
///
/// > A caller summing partials may add the first and must refuse to
/// > print a total on the second: a shared-blob reference count that is
/// > quietly short is worse than an absent one, because it is the number
/// > a future GC would act on.
///
/// `oh_my_pi::identify` folds a partial for every container it lists.
/// The listing is `IdentifyCtx::list` -> `locations::shallow_list`,
/// which stops at `SHALLOW_LIST_CAP` (4,096) and **returns no truncation
/// signal** (`crates/core/src/locations/mod.rs:99-121`). Containers past
/// the cap are never listed, so `fold_refs` is never called for them --
/// with `Some` or with `None` -- and `BlobReferences::complete` stays
/// `true` (`oh_my_pi.rs:277-299`).
///
/// A home with more than 4,096 entries under `sessions/` therefore
/// reports every blob's count under the note "in this pass's **full
/// coverage**" while the count is short. `MAX_CONTAINERS`' own `break`
/// (`oh_my_pi.rs:280-286`) has the same shape.
///
/// Required behaviour: a truncated listing makes the aggregate
/// incomplete, exactly as a truncated session body does.
#[test]
fn a_truncated_container_listing_must_not_report_a_complete_blob_count() {
    use swamp_core::agents::{ContainerCache, IdentificationCache, IdentifyCtx};

    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("omp");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("config.yml"), b"model: x\n").unwrap();

    let hash = "b".repeat(64);
    fs::create_dir_all(home.join("blobs")).unwrap();
    fs::write(home.join("blobs").join(&hash), b"png-bytes").unwrap();

    // Past `SHALLOW_LIST_CAP`. Every container holds one session that
    // references the same blob, so the true count is CONTAINERS.
    const CONTAINERS: usize = 4_200;
    for i in 0..CONTAINERS {
        let dir = home.join("sessions").join(format!("c{i:05}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("s0.jsonl"),
            format!(
                "{{\"cwd\":\"{}\",\"type\":\"user\",\"image_url\":\"blob:sha256:{hash}\"}}\n",
                root.display()
            ),
        )
        .unwrap();
    }

    let store = tempfile::tempdir().unwrap();
    let cache = IdentificationCache::load(store.path());
    let containers = ContainerCache::disabled();
    let ctx = IdentifyCtx::with_containers(1_000, &cache, &containers);
    let units = swamp_core::agents::oh_my_pi::identify(&home, &ctx);

    let note = units
        .iter()
        .find(|u| u.path.ends_with(&hash))
        .and_then(|u| u.note.clone())
        .unwrap_or_else(|| "<no blob unit>".to_string());

    let complete_claim = note.contains("full coverage");
    let counted_all = note.contains(&format!("referenced by {CONTAINERS} known session(s)"));
    assert!(
        !complete_claim || counted_all,
        "the blob's note claims this pass's full coverage while the container listing was \
         truncated at SHALLOW_LIST_CAP={} of {CONTAINERS} containers: {note}",
        swamp_core::locations::SHALLOW_LIST_CAP
    );
}
