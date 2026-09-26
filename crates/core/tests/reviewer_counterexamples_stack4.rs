//! Independent re-review 4: runtime counterexamples on the stack/17 tip.
//!
//! Written by the reviewer, not the author. Every fixture is a disposable
//! `tempfile` tree; nothing reads a real agent or editor home.
//!
//! * F1 residual: `shallow_list` returns `Truncation::Complete` with no
//!   entries when the directory cannot be read, so an unreadable
//!   sessions directory -- or one unreadable container -- reads as full
//!   coverage of a smaller set.
//! * Steady-state fix: `has_stored_state` treats a stored state that has
//!   a rules version but no anchor as "no stored state", so a live
//!   (TUI) plan goes incremental over rows classified under older rules.
//! * F3 guard: every symbol a pinned citation depends on sits on a line
//!   the `SWAMP_FETCH_UPSTREAM=1` re-fetch actually verifies (holds
//!   today; the filter that excludes lines would let a hand-written
//!   symbol line through).

#[path = "fixture/mod.rs"]
mod fixture;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use swamp_core::agents::{ContainerCache, IdentificationCache, IdentifyCtx};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal};

// ---------------------------------------------------------------------
// F1 residual
// ---------------------------------------------------------------------

/// Restores a directory's permissions when dropped, so the tempdir can
/// be cleaned up even if an assertion fails.
struct Unlocked(PathBuf);
impl Drop for Unlocked {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

fn omp_home(root: &Path, containers: usize) -> (PathBuf, String) {
    let home = root.join("omp");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("config.yml"), b"model: x\n").unwrap();
    let hash = "c".repeat(64);
    fs::create_dir_all(home.join("blobs")).unwrap();
    fs::write(home.join("blobs").join(&hash), b"png-bytes").unwrap();
    for i in 0..containers {
        let dir = home.join("sessions").join(format!("c{i:03}"));
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
    (home, hash)
}

fn blob_note(home: &Path, hash: &str) -> String {
    let store = tempfile::tempdir().unwrap();
    let cache = IdentificationCache::load(store.path());
    let containers = ContainerCache::disabled();
    let ctx = IdentifyCtx::with_containers(1_000, &cache, &containers);
    let units = swamp_core::agents::oh_my_pi::identify(home, &ctx);
    units
        .iter()
        .find(|u| u.path.ends_with(hash))
        .and_then(|u| u.note.clone())
        .unwrap_or_else(|| "<no blob unit>".to_string())
}

#[test]
fn an_unreadable_sessions_directory_must_not_read_as_full_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, hash) = omp_home(&root, 3);
    let sessions = home.join("sessions");
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = Unlocked(sessions.clone());
    if fs::read_dir(&sessions).is_ok() {
        eprintln!("skipped: running with privileges that ignore directory permissions");
        return;
    }
    let note = blob_note(&home, &hash);
    assert!(
        !note.contains("full coverage"),
        "the sessions directory could not be listed, so no reference count is complete; the \
         blob's note claims this pass's full coverage anyway: {note}"
    );
}

#[test]
fn an_unreadable_container_must_not_read_as_full_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, hash) = omp_home(&root, 3);
    let locked = home.join("sessions").join("c001");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = Unlocked(locked.clone());
    if fs::read_dir(&locked).is_ok() {
        eprintln!("skipped: running with privileges that ignore directory permissions");
        return;
    }
    let note = blob_note(&home, &hash);
    assert!(
        !note.contains("full coverage"),
        "one of three containers could not be listed, so the count is short; the blob's note \
         claims this pass's full coverage anyway: {note}"
    );
}

// ---------------------------------------------------------------------
// Steady-state fix: a stored rules version with no anchor
// ---------------------------------------------------------------------

struct Refusing;
impl FsEventsSource for Refusing {
    fn replay(&self, _r: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan {
            incremental: false,
            refusal: Some(RefreshRefusal::NoStoredEventId),
            changed_dirs: Vec::new(),
            current_event_id: 999,
            device: Some(1),
            live: false,
            consume: None,
        }
    }
}

/// A live plan, as `App::observe_live` builds one: it answers
/// "incremental" whatever the stored state says.
struct Live(Vec<PathBuf>);
impl FsEventsSource for Live {
    fn replay(&self, _r: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::from_live(self.0.clone(), 1_000, Some(1))
    }
}

fn mode_of(r: &swamp_core::report::Report) -> String {
    r.notes
        .iter()
        .find(|n| n.starts_with("fsevents: mode="))
        .cloned()
        .unwrap_or_else(|| format!("<no fsevents note: {:?}>", r.notes))
}

/// A store whose FSEvents file records rows classified under an older
/// `RULES_VERSION` but no anchor -- the file the pre-fix code left after
/// a root's only observation took the rules-changed branch, and the
/// file a `--full`-only user has -- must still force the rules walk.
/// The fix reads "no event id and no timestamp" as "no stored state" and
/// lets a live plan go incremental; its checkpoint then stamps the
/// current rules version, so the reclassification never happens.
#[test]
fn a_stored_rules_version_without_an_anchor_still_forces_the_rules_walk() {
    unsafe { std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0") };
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(&fx.root).unwrap();

    let first = swamp_core::report::report_full_mode_with_source(
        &root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &Refusing,
    )
    .expect("first observation");
    assert!(mode_of(&first).contains("mode=full"), "{}", mode_of(&first));

    let dir = swamp_core::growth::volume_store_dir(store.path(), &root);
    let state = dir.join("cursors.parquet");
    assert!(
        state.exists(),
        "the first observation anchors at {}",
        state.display()
    );
    let older = swamp_core::ecosystem::RULES_VERSION - 1;
    swamp_core::growth::write_fsevents_anchor(
        &dir,
        &swamp_core::fs_events::FsEventsState {
            event_id: None,
            device: None,
            last_observed_at: None,
            rules_version: older,
            unit_root: None,
        },
    )
    .unwrap();
    assert!(dir.join("topology.parquet").exists(), "topology is stored");

    fs::write(fx.node_modules.join("touched.bin"), vec![b't'; 4096]).unwrap();
    let second = swamp_core::report::report_full_mode_with_source(
        &root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &Live(vec![fx.node_modules.clone()]),
    )
    .expect("second observation");
    let m = mode_of(&second);
    assert!(
        m.contains("mode=full"),
        "the stored rows were classified under rules version {older} (current {}); a live \
         plan must not carry them forward incrementally: {m}",
        swamp_core::ecosystem::RULES_VERSION
    );
}

// ---------------------------------------------------------------------
// F3 guard
// ---------------------------------------------------------------------

/// `quoted_lines` from `upstream_citations_are_checked.rs`, verbatim:
/// the lines the online re-fetch requires to be verbatim upstream.
fn quoted_lines(excerpt: &str) -> Vec<String> {
    excerpt
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let bare = l.trim_start_matches(|c: char| {
                c == '/' || c == '#' || c == '<' || c == '!' || c == '-' || c == ' '
            });
            !(bare.starts_with("repo:")
                || bare.starts_with("commit:")
                || bare.starts_with("retrieved:")
                || bare.starts_with("path:")
                || bare.starts_with("lines ")
                || bare.starts_with("...")
                || bare.starts_with("…")
                || bare.starts_with("(")
                || l.contains("--- line")
                || bare.is_empty())
        })
        .map(str::to_string)
        .collect()
}

/// A symbol on a line the re-fetch skips (one starting `(`, `path:`,
/// `lines `, `...`, or containing `--- line`) is checked against the
/// excerpt only -- that is, against the author. Holds today for all 42
/// pinned citations; kept as a guard.
#[test]
fn every_citation_symbol_is_on_a_line_the_refetch_verifies() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream");
    let text = fs::read_to_string(dir.join("citations.toml")).unwrap();
    let mut problems = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if line.trim() == "[[citation]]" {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
            block.push("");
            continue;
        }
        if !block.is_empty() {
            block.push(line);
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    let field = |b: &[&str], k: &str| -> String {
        b.iter()
            .find_map(|l| {
                let (key, v) = l.split_once('=')?;
                (key.trim() == k).then(|| v.trim().trim_matches('"').to_string())
            })
            .unwrap_or_default()
    };
    for b in &blocks {
        if field(b, "upstream_blake3") == "doc-page" {
            continue;
        }
        let excerpt = fs::read_to_string(dir.join(field(b, "excerpt"))).unwrap_or_default();
        let verified = quoted_lines(&excerpt).join("\n");
        let symbols = field(b, "symbols");
        for s in symbols
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split("', '")
            .map(|s| s.trim().trim_matches('\''))
            .filter(|s| !s.is_empty())
        {
            if !verified.contains(s) {
                problems.push(format!(
                    "{} {}: symbol `{s}` appears only on excerpt lines the re-fetch does not verify",
                    field(b, "tool"),
                    field(b, "path")
                ));
            }
        }
    }
    assert!(
        blocks.len() >= 40,
        "parsed {} citation blocks",
        blocks.len()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
