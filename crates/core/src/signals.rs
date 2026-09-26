//! Git activity signals: facts about a worktree's git state, never a
//! verdict. Every signal carries the value observed; a timeout, missing
//! upstream, or parse failure records `Unknown`, never a guess.
//!
//! Computed with `gix` (gitoxide, through the read-only `fs_gate::git`)
//! directly against the on-disk object store during discovery, in the same thread pool as the rest of the
//! walk (see `walk.rs`). No `git` subprocess is spawned here, except for
//! `idle_for`'s newest-mtime scan, which is plain filesystem I/O with no
//! git object access at all.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::fs_gate::git::{Repo, Unpushed};
use crate::report::Signal;

/// Bound on the `status` (dirty) walk: if a repo's status has more
/// entries than this, or takes longer than [`STATUS_TIMEOUT`], the
/// result is `Unknown` rather than a partial/guessed answer.
const STATUS_ENTRY_CAP: usize = 4_000;
const STATUS_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalValue {
    LastCommitAgeSecs(u64),
    Dirty(bool),
    UnpushedCount(u32),
    UnpushedUnknownNoUpstream,
    Locked(bool),
    IdleForSecs(u64),
    Unknown,
}

impl SignalValue {
    fn render(&self) -> String {
        match self {
            SignalValue::LastCommitAgeSecs(secs) => human_age(*secs),
            SignalValue::Dirty(true) => "dirty".to_string(),
            SignalValue::Dirty(false) => "clean".to_string(),
            SignalValue::UnpushedCount(n) => format!("{n} unpushed"),
            SignalValue::UnpushedUnknownNoUpstream => "unknown (no upstream)".to_string(),
            SignalValue::Locked(true) => "locked".to_string(),
            SignalValue::Locked(false) => "unlocked".to_string(),
            SignalValue::IdleForSecs(secs) => format!("idle {}", human_duration(*secs)),
            SignalValue::Unknown => "unknown".to_string(),
        }
    }
}

fn human_age(secs: u64) -> String {
    if secs < 3600 {
        format!("last commit {}m", (secs / 60).max(1))
    } else if secs < 86_400 {
        format!("last commit {}h", secs / 3600)
    } else {
        format!("last commit {}d", secs / 86_400)
    }
}

/// Renders a bare duration ("3d", "4h", "12m", "45s") with no "last
/// commit"/other prefix -- `idle_for`'s own render adds the "idle "
/// prefix itself.
fn human_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// HEAD commit time, read straight from the commit object. `Unknown` on
/// an unborn HEAD, a corrupt object, or a commit timestamp we cannot
/// trust (negative/unparsable).
fn last_commit_age(repo: &Repo, observed_at: u64) -> SignalValue {
    let Some(seconds) = repo.head_commit_seconds() else {
        return SignalValue::Unknown;
    };
    if seconds < 0 {
        return SignalValue::Unknown;
    }
    let commit_ts = seconds as u64;
    if commit_ts <= observed_at {
        SignalValue::LastCommitAgeSecs(observed_at - commit_ts)
    } else {
        SignalValue::LastCommitAgeSecs(0)
    }
}

/// Dirty = any index/worktree change, from `gix`'s status walk (covers
/// modified/added/removed/untracked, mirroring `git status --porcelain`).
/// Bounded by both an entry cap and a wall-clock timeout: a pathological
/// worktree (huge untracked tree, slow filesystem) degrades to `Unknown`
/// instead of blocking the report.
fn dirty(repo: &Repo) -> SignalValue {
    match repo.any_status_change(STATUS_ENTRY_CAP, STATUS_TIMEOUT) {
        Some(any) => SignalValue::Dirty(any),
        None => SignalValue::Unknown,
    }
}

/// Ahead-of-upstream commit count, via a revwalk from HEAD hidden behind
/// the upstream tracking ref. `UnpushedUnknownNoUpstream` (not `Unknown`)
/// when no upstream is configured for the current branch -- this is a
/// normal, expected state, not a failure, and the renderer labels it
/// distinctly (`unpushed: unknown (no upstream)`).
fn unpushed(repo: &Repo) -> SignalValue {
    match repo.unpushed() {
        Unpushed::Count(n) => SignalValue::UnpushedCount(n),
        Unpushed::NoUpstream => SignalValue::UnpushedUnknownNoUpstream,
        Unpushed::Unknown => SignalValue::Unknown,
    }
}

/// A worktree is locked when its administrative `<gitdir>/locked` file
/// exists (linked worktrees only; a main checkout is never locked this
/// way). No object database access, so it never depends on `repo`
/// having opened cleanly.
fn locked(repo: &Repo) -> SignalValue {
    SignalValue::Locked(repo.worktree_locked())
}

/// Directories never descended into while looking for the newest mtime
/// under a worktree's source: `.git` itself, plus the same artifact stop
/// list `git.rs` discovery uses. Artifact churn (a build writing new
/// object files) is not "the human touched this project"; only source
/// content should move the idle clock.
const IDLE_STOP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build"];
/// Bound on the whole newest-mtime walk. A large worktree degrades to
/// "whatever was found before the budget ran out" rather than blocking
/// the report.
const IDLE_WALK_BUDGET: Duration = Duration::from_millis(300);

/// Newest mtime found under `dir` (excluding [`IDLE_STOP_DIRS`]),
/// time-bounded by [`IDLE_WALK_BUDGET`]. `None` only when nothing at all
/// could be read (e.g. permission denied on the root itself). Plain
/// filesystem I/O -- no git object access, so it works even when `gix`
/// failed to open the repo.
fn newest_mtime_secs(dir: &Path) -> Option<u64> {
    let deadline = Instant::now() + IDLE_WALK_BUDGET;
    let mut best: Option<u64> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if Instant::now() >= deadline {
            break;
        }
        let Ok(entries) = crate::fs_gate::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_symlink() {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
                && let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                let secs = dur.as_secs();
                best = Some(best.map_or(secs, |b: u64| b.max(secs)));
            }
            if file_type.is_dir() && !IDLE_STOP_DIRS.contains(&name.as_ref()) {
                stack.push(entry.path());
            }
        }
    }
    best
}

/// `idle_for` = now - max(last commit time, newest mtime observed under
/// the worktree's source). A worktree with no commits and no readable
/// files is `Unknown`, never zero by default.
fn idle_for(dir: &Path, observed_at: u64, last_commit_secs: Option<u64>) -> SignalValue {
    let mtime_epoch = newest_mtime_secs(dir);
    match last_commit_secs.into_iter().chain(mtime_epoch).max() {
        Some(newest) if newest <= observed_at => SignalValue::IdleForSecs(observed_at - newest),
        Some(_) => SignalValue::IdleForSecs(0),
        None => SignalValue::Unknown,
    }
}

/// Raw (unrendered) signal values for one worktree, used by the
/// composite `merge_complete` fact and by `filter.rs`'s `idle >`
/// predicate, in addition to the human-rendered `Signal` rows this
/// module has always produced.
#[derive(Debug, Clone)]
pub struct RawSignals {
    pub last_commit_age_secs: Option<u64>,
    pub dirty: Option<bool>,
    pub unpushed: Option<u32>,
    pub locked: Option<bool>,
    pub idle_for_secs: Option<u64>,
}

/// Computes all git activity signals for one worktree at `dir`, bounded
/// and side-effect free. `observed_at` is the report's single observation
/// timestamp so every worktree's ages are measured from the same instant.
pub fn compute_signals(dir: &Path, observed_at: u64) -> Vec<Signal> {
    compute_signals_raw(dir, observed_at).0
}

/// Same as [`compute_signals`], but also returns the raw (unrendered)
/// values the `merge_complete` composite and `filter.rs`'s `idle >`
/// predicate need, so neither has to re-parse a rendered string.
/// Re-renders a worktree's signals `elapsed` seconds after they were
/// computed, for a worktree FSEvents reported no change under: ages and
/// idle time grow with the clock; dirty, unpushed and locked cannot have
/// changed without a filesystem event under the worktree.
pub fn age_signals(rows: &[Signal], raw: &RawSignals, elapsed: u64) -> (Vec<Signal>, RawSignals) {
    let mut raw = raw.clone();
    raw.last_commit_age_secs = raw.last_commit_age_secs.map(|a| a + elapsed);
    raw.idle_for_secs = raw.idle_for_secs.map(|i| i + elapsed);
    let rows = rows
        .iter()
        .map(
            |r| match (r.name.as_str(), raw.last_commit_age_secs, raw.idle_for_secs) {
                ("last_commit", Some(a), _) => Signal {
                    name: r.name.clone(),
                    value: SignalValue::LastCommitAgeSecs(a).render(),
                },
                ("idle_for", _, Some(i)) => Signal {
                    name: r.name.clone(),
                    value: SignalValue::IdleForSecs(i).render(),
                },
                _ => r.clone(),
            },
        )
        .collect();
    (rows, raw)
}

pub fn compute_signals_raw(dir: &Path, observed_at: u64) -> (Vec<Signal>, RawSignals) {
    let Some(repo) = Repo::open(dir) else {
        let unknown = SignalValue::Unknown.render();
        let idle_v = idle_for(dir, observed_at, None);
        return (
            vec![
                Signal {
                    name: "last_commit".to_string(),
                    value: unknown.clone(),
                },
                Signal {
                    name: "dirty".to_string(),
                    value: unknown.clone(),
                },
                Signal {
                    name: "unpushed".to_string(),
                    value: unknown,
                },
                Signal {
                    name: "locked".to_string(),
                    value: SignalValue::Locked(false).render(),
                },
                Signal {
                    name: "idle_for".to_string(),
                    value: idle_v.render(),
                },
            ],
            RawSignals {
                last_commit_age_secs: None,
                dirty: None,
                unpushed: None,
                locked: Some(false),
                idle_for_secs: match idle_v {
                    SignalValue::IdleForSecs(s) => Some(s),
                    _ => None,
                },
            },
        );
    };

    let last_commit = last_commit_age(&repo, observed_at);
    let dirty_v = dirty(&repo);
    let unpushed_v = unpushed(&repo);
    let locked_v = locked(&repo);
    let last_commit_secs = match &last_commit {
        SignalValue::LastCommitAgeSecs(s) => Some(observed_at.saturating_sub(*s)),
        _ => None,
    };
    let idle_v = idle_for(dir, observed_at, last_commit_secs);

    let rows = vec![
        Signal {
            name: "last_commit".to_string(),
            value: last_commit.render(),
        },
        Signal {
            name: "dirty".to_string(),
            value: dirty_v.render(),
        },
        Signal {
            name: "unpushed".to_string(),
            value: unpushed_v.render(),
        },
        Signal {
            name: "locked".to_string(),
            value: locked_v.render(),
        },
        Signal {
            name: "idle_for".to_string(),
            value: idle_v.render(),
        },
    ];
    let raw = RawSignals {
        last_commit_age_secs: match last_commit {
            SignalValue::LastCommitAgeSecs(s) => Some(s),
            _ => None,
        },
        dirty: match dirty_v {
            SignalValue::Dirty(d) => Some(d),
            _ => None,
        },
        unpushed: match unpushed_v {
            SignalValue::UnpushedCount(n) => Some(n),
            _ => None,
        },
        locked: match locked_v {
            SignalValue::Locked(l) => Some(l),
            _ => None,
        },
        idle_for_secs: match idle_v {
            SignalValue::IdleForSecs(s) => Some(s),
            _ => None,
        },
    };
    (rows, raw)
}

/// The worktree's current commit hash, used as the cache-invalidation
/// key for GitHub enrichment (`github.rs`). `None` on any failure
/// (unborn HEAD, corrupt object, `gix` couldn't open the repo).
pub fn tip_sha(dir: &Path) -> Option<String> {
    Repo::open(dir)?.head_id_hex()
}

/// Parallel equivalent of [`compute_signals_raw`], preserving input
/// order. Used by `report.rs` so the merge_complete composite and
/// `idle_secs` field get the raw values without a second per-worktree
/// pass.
type SignalsRawResult = (Vec<Signal>, RawSignals);

pub fn compute_signals_raw_parallel(
    paths: &[std::path::PathBuf],
    observed_at: u64,
) -> Vec<SignalsRawResult> {
    let n = paths.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(4)
        .min(n)
        .max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<SignalsRawResult>>> =
        (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let next = &next;
            let results = &results;
            let paths = &paths;
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let sigs = compute_signals_raw(&paths[i], observed_at);
                    *results[i].lock().unwrap() = Some(sigs);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|m| {
            m.into_inner()
                .unwrap()
                .expect("every index visited exactly once")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::now;
    use std::process::Command as PCommand;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        crate::work_counters::record_spawn();
        let status = PCommand::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn fresh_commit_has_small_age_and_is_clean_with_no_upstream() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);

        let observed_at = now();
        let signals = compute_signals(dir.path(), observed_at);
        let by_name = |n: &str| signals.iter().find(|s| s.name == n).unwrap().value.clone();

        assert!(by_name("last_commit").starts_with("last commit"));
        assert_eq!(by_name("dirty"), "clean");
        assert_eq!(by_name("unpushed"), "unknown (no upstream)");
        assert_eq!(by_name("locked"), "unlocked");
    }

    #[test]
    fn dirty_worktree_reports_dirty() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("f.txt"), "changed").unwrap();

        let signals = compute_signals(dir.path(), now());
        let dirty = signals.iter().find(|s| s.name == "dirty").unwrap();
        assert_eq!(dirty.value, "dirty");
    }

    #[test]
    fn unpushed_count_reflects_commits_ahead_of_upstream() {
        let remote_dir = tempdir().unwrap();
        git(remote_dir.path(), &["init", "-q", "--bare"]);

        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
        );
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);

        std::fs::write(dir.path().join("f2.txt"), "more").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);

        let signals = compute_signals(dir.path(), now());
        let unpushed = signals.iter().find(|s| s.name == "unpushed").unwrap();
        assert_eq!(unpushed.value, "1 unpushed");
    }

    #[test]
    fn linked_worktree_lock_file_is_detected() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);

        let linked_parent = tempdir().unwrap();
        let linked = linked_parent.path().join("linked-wt");
        git(
            dir.path(),
            &[
                "worktree",
                "add",
                "-q",
                linked.to_str().unwrap(),
                "-b",
                "linked",
            ],
        );

        let signals = compute_signals(&linked, now());
        let locked = signals.iter().find(|s| s.name == "locked").unwrap();
        assert_eq!(locked.value, "unlocked");

        git(dir.path(), &["worktree", "lock", linked.to_str().unwrap()]);
        let signals = compute_signals(&linked, now());
        let locked = signals.iter().find(|s| s.name == "locked").unwrap();
        assert_eq!(locked.value, "locked");
    }

    #[test]
    fn idle_for_uses_commit_time_when_newer_than_filesystem() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);

        let (_, raw) = compute_signals_raw(dir.path(), now());
        assert!(raw.idle_for_secs.is_some());
    }

    #[test]
    fn tip_sha_matches_head() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("f.txt"), "hi").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);

        crate::work_counters::record_spawn();
        let out = PCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(tip_sha(dir.path()), Some(expected));
    }
}
