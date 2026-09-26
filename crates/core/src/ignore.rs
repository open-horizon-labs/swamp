//! Git tracking status for paths inside a checkout: is this directory or
//! file **tracked**, **ignored**, or merely **untracked**?
//!
//! This is the decisive fact for a Source subtree. A tracked tree holds
//! authored work recoverable from the remote; an ignored tree is outside
//! version control entirely — generated output (rebuildable) or private
//! data (irrecoverable), and deleting it can never be undone with git.
//! The tool states the fact and never turns it into a verdict.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrackState {
    /// At least one path here is in git's index.
    Tracked,
    /// Matched by a gitignore rule.
    Ignored,
    /// In the worktree, not in the index, not ignored.
    Untracked,
    /// No repository, or git could not answer.
    Unknown,
}

impl TrackState {
    pub fn label(self) -> &'static str {
        match self {
            TrackState::Tracked => "tracked",
            TrackState::Ignored => "ignored",
            TrackState::Untracked => "untracked",
            TrackState::Unknown => "",
        }
    }

    /// Inverse of [`Self::label`] (R18a-3: `worktree_entries.parquet`'s
    /// `track` column). `""`/`"unknown"`/anything unrecognized reads back
    /// as `Unknown` -- the same value `label()` maps *to* `""`, so a
    /// round trip through this pair is exact.
    pub fn from_label(label: &str) -> Self {
        match label {
            "tracked" => TrackState::Tracked,
            "ignored" => TrackState::Ignored,
            "untracked" => TrackState::Untracked,
            _ => TrackState::Unknown,
        }
    }
}

/// The per-checkout ignore/index lens: `gix` lives in the capability
/// gate (`fs_gate::git`), which answers only this query.
pub use crate::fs_gate::git::IgnoreLens;

/// The first few paths under `root` that git neither tracks nor ignores,
/// with their sizes: content that exists **only here**. Removing a whole
/// checkout destroys these, and nothing (remote, rebuild) brings them
/// back — so they are the bar a whole-checkout removal has to clear.
///
/// Bounded: stops after `limit` findings or `max_entries` directory
/// entries, and never descends into an ignored directory.
pub fn untracked_content(
    root: &Path,
    limit: usize,
    max_entries: usize,
) -> Vec<(std::path::PathBuf, u64)> {
    let Some(lens) = IgnoreLens::open(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        if found.len() >= limit || seen > max_entries {
            break;
        }
        let Ok(entries) = crate::fs_gate::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if found.len() >= limit || seen > max_entries {
                break;
            }
            let path = e.path();
            let Ok(meta) = crate::fs_gate::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel = rel.display().to_string();
            if rel == ".git" {
                continue;
            }
            let is_dir = meta.is_dir();
            match lens.status(&rel, is_dir) {
                TrackState::Ignored => {}
                TrackState::Untracked => {
                    let bytes = if is_dir {
                        crate::walk::resize_artifact(&path, crate::report::ArtifactKind::Unknown, 0)
                            .bytes
                    } else {
                        meta.len()
                    };
                    found.push((path.clone(), bytes));
                }
                TrackState::Tracked | TrackState::Unknown => {
                    if is_dir {
                        stack.push(path);
                    }
                }
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.1));
    found
}

/// Bytes under one worktree that no classified artifact claims, split by
/// what git says about them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackSplit {
    pub tracked: u64,
    pub ignored: u64,
    pub untracked: u64,
}

impl TrackSplit {
    pub fn total(&self) -> u64 {
        self.tracked + self.ignored + self.untracked
    }

    fn give(&mut self, state: TrackState, bytes: u64) {
        match state {
            TrackState::Ignored => self.ignored += bytes,
            TrackState::Untracked => self.untracked += bytes,
            TrackState::Tracked | TrackState::Unknown => self.tracked += bytes,
        }
    }

    fn take(&mut self, state: TrackState, bytes: u64) {
        match state {
            TrackState::Ignored => self.ignored = self.ignored.saturating_sub(bytes),
            TrackState::Untracked => self.untracked = self.untracked.saturating_sub(bytes),
            TrackState::Tracked | TrackState::Unknown => {
                self.tracked = self.tracked.saturating_sub(bytes)
            }
        }
    }
}

/// Splits a worktree's non-artifact bytes by git tracking state, from
/// the directory rows the walk already produced.
///
/// The split runs at **directory** granularity, then corrects every file
/// the store holds a row of its own for. Directories are what the column
/// store keeps a row per, and git's rule that nothing under an excluded
/// directory can be re-included makes a directory's state carry down.
/// That alone would call an ignored file in a tracked directory tracked
/// — a stray `payload.bin` or `*.log` — so each large-file row is asked
/// about separately and moved to the bucket it really belongs in. Small
/// ignored files inside tracked directories still count as tracked;
/// they are below the store's own threshold for naming a file at all.
///
/// `artifact_rels` names the artifact roots under this worktree; their
/// subtrees are already folded into their own rows and are skipped.
/// Returns `None` when the checkout is not a repository, which is the
/// caller's signal to leave the bytes as one undivided row.
pub fn split_dirs_by_track(
    wt_root: &Path,
    dirs: &[crate::report::DirRollup],
    files: &[crate::report::FileRow],
    artifact_rels: &std::collections::HashSet<String>,
) -> Option<TrackSplit> {
    let lens = IgnoreLens::open(wt_root)?;
    // Parents before children, so an ignored directory can hand its
    // state down instead of every descendant re-asking.
    let mut order: Vec<&crate::report::DirRollup> = dirs.iter().collect();
    order.sort_by_key(|d| (d.rel_path.matches('/').count(), d.rel_path.clone()));
    let mut state_of: std::collections::HashMap<&str, TrackState> =
        std::collections::HashMap::new();
    let mut split = TrackSplit::default();
    for d in order {
        if artifact_rels
            .iter()
            .any(|a| d.rel_path == *a || under(&d.rel_path, a))
        {
            continue;
        }
        let inherited = d
            .parent_rel_path
            .as_deref()
            .and_then(|p| state_of.get(p).copied());
        // git cannot re-include anything under an excluded directory, so
        // an ignored parent settles every path beneath it.
        let state = match inherited {
            Some(TrackState::Ignored) => TrackState::Ignored,
            _ => lens.status(&d.rel_path, true),
        };
        state_of.insert(d.rel_path.as_str(), state);
        // A directory git cannot answer about is authored work until
        // something says otherwise; this never invents a reason to call
        // bytes disposable.
        split.give(state, d.own_allocated);
    }
    // A file the store named individually is big enough to matter, so
    // ask about it rather than let its directory answer for it. Its
    // bytes are already inside that directory's own total, so what
    // moves is the difference between the two answers.
    for f in files {
        if artifact_rels.iter().any(|a| under(&f.rel_path, a)) {
            continue;
        }
        let dir_rel = f.rel_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let Some(&dir_state) = state_of.get(dir_rel) else {
            continue; // its directory is not part of the remainder
        };
        let file_state = match dir_state {
            TrackState::Ignored => TrackState::Ignored,
            _ => lens.status(&f.rel_path, false),
        };
        if file_state == dir_state {
            continue;
        }
        split.take(dir_state, f.allocated);
        split.give(file_state, f.allocated);
    }
    Some(split)
}

/// Whether `rel` is `prefix` itself or sits underneath it.
fn under(rel: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    rel == prefix || rel.starts_with(&format!("{prefix}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        crate::work_counters::record_spawn();
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn tracked_ignored_and_untracked_are_distinguished() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@e"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::write(root.join(".gitignore"), "raw/\n*.log\nbuild/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        std::fs::write(root.join("raw/data.csv"), "a,b").unwrap();
        std::fs::create_dir_all(root.join("scratch")).unwrap();
        std::fs::write(root.join("scratch/note.txt"), "x").unwrap();
        std::fs::write(root.join("run.log"), "x").unwrap();
        git(root, &["add", ".gitignore", "src/main.rs"]);
        git(root, &["commit", "-qm", "init"]);

        let lens = IgnoreLens::open(root).expect("repo opens");
        assert_eq!(lens.status("src", true), TrackState::Tracked);
        assert_eq!(lens.status("src/main.rs", false), TrackState::Tracked);
        assert_eq!(lens.status("raw", true), TrackState::Ignored);
        assert_eq!(lens.status("run.log", false), TrackState::Ignored);
        assert_eq!(lens.status("scratch", true), TrackState::Untracked);
        assert_eq!(
            lens.status("scratch/note.txt", false),
            TrackState::Untracked
        );
        assert_eq!(lens.status(".", true), TrackState::Tracked);
    }

    fn dir(rel: &str, parent: Option<&str>, own: u64) -> crate::report::DirRollup {
        crate::report::DirRollup {
            worktree_id: "w1".into(),
            track: None,
            rel_path: rel.into(),
            parent_rel_path: parent.map(|p| p.to_string()),
            allocated_total: own,
            own_allocated: own,
            file_count: 1,
            entry_count: 1,
            symlink_count: 0,
            mod_time_min: 0,
            complete: true,
            growth_bytes: None,
        }
    }

    #[test]
    fn remainder_bytes_split_by_what_git_says_about_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@e"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::write(root.join(".gitignore"), "raw/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::create_dir_all(root.join("raw/deep")).unwrap();
        std::fs::create_dir_all(root.join("scratch")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        git(root, &["add", ".gitignore", "src/main.rs"]);
        git(root, &["commit", "-qm", "init"]);

        let dirs = vec![
            dir("", None, 10),
            dir("src", Some(""), 100),
            dir("raw", Some(""), 1000),
            // A child of an ignored directory is ignored too, and is not
            // asked about separately: git cannot re-include it.
            dir("raw/deep", Some("raw"), 2000),
            dir("scratch", Some(""), 30),
            // An artifact root and everything under it is already its
            // own row, so none of it counts here.
            dir("node_modules", Some(""), 99_999),
        ];
        let artifacts: std::collections::HashSet<String> =
            ["node_modules".to_string()].into_iter().collect();
        let split = split_dirs_by_track(root, &dirs, &[], &artifacts).expect("repo opens");
        assert_eq!(split.tracked, 110, "the root's own files and src/");
        assert_eq!(split.ignored, 3000, "raw/ and everything under it");
        assert_eq!(split.untracked, 30, "scratch/");
        assert_eq!(split.total(), 3140);
    }

    #[test]
    fn a_large_ignored_file_does_not_count_as_source() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@e"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::write(root.join(".gitignore"), "payload.bin\n").unwrap();
        std::fs::write(root.join("payload.bin"), "x").unwrap();
        std::fs::write(root.join("README.md"), "x").unwrap();
        git(root, &["add", ".gitignore", "README.md"]);
        git(root, &["commit", "-qm", "init"]);

        // The root directory is tracked, so directory granularity alone
        // calls its 18MB ignored file source. The file's own row is what
        // corrects that.
        let dirs = vec![dir("", None, 18_000_100)];
        let files = vec![crate::report::FileRow {
            worktree_id: "w1".into(),
            rel_path: "payload.bin".into(),
            allocated: 18_000_000,
            mod_time_min: 0,
            growth_bytes: None,
        }];
        let split = split_dirs_by_track(root, &dirs, &files, &Default::default()).unwrap();
        assert_eq!(split.ignored, 18_000_000, "the ignored file's own bytes");
        assert_eq!(split.tracked, 100, "what is left is the tracked files");
        assert_eq!(split.total(), 18_000_100, "the total is preserved");
    }

    #[test]
    fn a_non_repository_has_no_split() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            split_dirs_by_track(tmp.path(), &[dir("", None, 1)], &[], &Default::default())
                .is_none()
        );
    }

    #[test]
    fn a_non_repository_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(IgnoreLens::open(tmp.path()).is_none());
    }
}
