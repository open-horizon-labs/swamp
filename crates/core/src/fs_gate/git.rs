//! Every use of `gix` (gitoxide), read-only.
//!
//! `gix` is a filesystem and process capability in its own right: it
//! re-exports `gix_fs` (whose `symlink::remove` is `std::fs::remove_file`),
//! `gix_command` (which spawns), `gix_tempfile` and `gix_lock` (which
//! create and rename files). Re-review 5 found it named, ungated, in
//! `signals.rs` and `ignore.rs`. So the crate lives here, behind the
//! handful of *queries* swamp asks of a repository, and
//! `gate_paths_only_inside_gates` rejects `gix` and every `gix_*` crate
//! anywhere else (clippy's `disallowed_types`/`disallowed_methods` name
//! the entry points too).
//!
//! Nothing here writes: no index write, no ref update, no lock file, no
//! temp file, no hook, no subprocess. The types that leave this module
//! are plain data ([`Unpushed`], [`crate::ignore::TrackState`]) or
//! opaque handles ([`Repo`], [`IgnoreLens`]) whose only methods are
//! those queries.

use crate::ignore::TrackState;
use gix::bstr::ByteSlice;
use std::path::Path;
use std::time::{Duration, Instant};

/// An opened repository. Opaque: the queries below are all it answers.
pub struct Repo(gix::Repository);

/// What `unpushed` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpushed {
    /// Commits on HEAD that the upstream tracking ref does not have.
    Count(u32),
    /// Detached HEAD, or no upstream configured: a normal state.
    NoUpstream,
    /// The walk or a lookup failed.
    Unknown,
}

impl Repo {
    /// Opens the repository at (or containing) `dir`.
    pub fn open(dir: &Path) -> Option<Repo> {
        gix::open(dir).ok().map(Repo)
    }

    /// HEAD's commit time in seconds, `None` on an unborn HEAD or an
    /// unreadable commit.
    pub fn head_commit_seconds(&self) -> Option<i64> {
        let commit = self.0.head_commit().ok()?;
        Some(commit.time().ok()?.seconds)
    }

    /// HEAD's object id as hex.
    pub fn head_id_hex(&self) -> Option<String> {
        Some(self.0.head_id().ok()?.to_string())
    }

    /// Whether `git status` would list anything: `None` when the status
    /// walk failed, exceeded `cap` entries, or ran past `timeout`.
    pub fn any_status_change(&self, cap: usize, timeout: Duration) -> Option<bool> {
        let start = Instant::now();
        let platform = self.0.status(gix::progress::Discard).ok()?;
        let iter = platform.into_iter(None).ok()?;
        let mut count = 0usize;
        let mut any = false;
        for item in iter {
            if start.elapsed() >= timeout {
                return None;
            }
            count += 1;
            if count > cap {
                return None;
            }
            item.ok()?;
            any = true;
        }
        Some(any)
    }

    /// Commits ahead of the current branch's upstream tracking ref.
    pub fn unpushed(&self) -> Unpushed {
        let repo = &self.0;
        let Ok(head) = repo.head() else {
            return Unpushed::Unknown;
        };
        let Some(branch_name) = head.referent_name().map(|n| n.to_owned()) else {
            return Unpushed::NoUpstream;
        };
        let tracking_name = match repo
            .branch_remote_tracking_ref_name(branch_name.as_ref(), gix::remote::Direction::Fetch)
        {
            Some(Ok(name)) => name,
            Some(Err(_)) | None => return Unpushed::NoUpstream,
        };
        let Ok(mut tracking_ref) = repo.find_reference(tracking_name.as_ref()) else {
            return Unpushed::NoUpstream;
        };
        let Ok(tracking_id) = tracking_ref.peel_to_id() else {
            return Unpushed::Unknown;
        };
        let Ok(head_id) = repo.head_id() else {
            return Unpushed::Unknown;
        };
        let walk = repo
            .rev_walk([head_id.detach()])
            .with_hidden([tracking_id.detach()])
            .all();
        match walk {
            Ok(iter) => {
                let mut n: u32 = 0;
                for item in iter {
                    if item.is_err() {
                        return Unpushed::Unknown;
                    }
                    n += 1;
                    if n == u32::MAX {
                        break;
                    }
                }
                Unpushed::Count(n)
            }
            Err(_) => Unpushed::Unknown,
        }
    }

    /// Whether this is a linked worktree whose `<gitdir>/locked` exists.
    pub fn worktree_locked(&self) -> bool {
        self.0.worktree().is_some_and(|wt| wt.is_locked())
    }
}

/// An exclude stack plus index for one checkout, reused across many
/// lookups (building it per path would re-read every `.gitignore`).
///
/// The stack is behind a `RefCell` because `at_entry` needs `&mut` while
/// callers hold the lens by shared reference; the lens is per-worktree
/// and used from one thread at a time.
pub struct IgnoreLens {
    repo: gix::Repository,
    index: gix::index::State,
    stack: std::cell::RefCell<Option<gix::worktree::Stack>>,
}

impl IgnoreLens {
    /// Opens the checkout at `root`. `None` when it is not a repository.
    pub fn open(root: &Path) -> Option<Self> {
        let repo = gix::open(root).ok()?;
        let snapshot = repo.index_or_empty().ok()?;
        let index: gix::index::State = (**snapshot).clone().into();
        Some(Self {
            repo,
            index,
            stack: std::cell::RefCell::new(None),
        })
    }

    /// Status of `rel` (relative to the checkout root) — `is_dir` matters
    /// because gitignore rules can be directory-only.
    pub fn status(&self, rel: &str, is_dir: bool) -> TrackState {
        let rel_trimmed = rel
            .trim_start_matches("./")
            .trim_end_matches('/')
            .trim_end_matches('.');
        let rel_trimmed = rel_trimmed.trim_end_matches('/');
        if rel_trimmed.is_empty() {
            return TrackState::Tracked; // the checkout root itself
        }
        // Tracked wins: a path with any index entry under it is tracked,
        // even if a broad ignore rule would also match it.
        let bytes = rel_trimmed.as_bytes().as_bstr();
        if self.index.entry_by_path(bytes).is_some() {
            return TrackState::Tracked;
        }
        if is_dir {
            let with_slash = format!("{rel_trimmed}/");
            if self
                .index
                .prefixed_entries(with_slash.as_bytes().as_bstr())
                .is_some_and(|e| !e.is_empty())
            {
                return TrackState::Tracked;
            }
        }
        let mut cached = self.stack.borrow_mut();
        if cached.is_none() {
            let Ok(built) = self.repo.excludes(
                &self.index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            ) else {
                return TrackState::Unknown;
            };
            // `detach` drops the borrow of the repository, so the stack
            // can be owned by the lens; `at_entry` then takes the object
            // database explicitly.
            *cached = Some(built.detach());
        }
        let Some(stack) = cached.as_mut() else {
            return TrackState::Unknown;
        };
        // A directory is ignored when everything inside it is: gix's stack
        // answers about a directory's *contents*, so ask about a probe path
        // inside it rather than the directory itself (a trailing slash
        // alone returns the container's state, not the rule's effect).
        let lookup = if is_dir {
            format!("{rel_trimmed}/.swamp-probe")
        } else {
            rel_trimmed.to_string()
        };
        let mode = is_dir.then_some(gix::index::entry::Mode::FILE);
        match stack.at_entry(lookup.as_bytes().as_bstr(), mode, &self.repo.objects) {
            Ok(platform) => {
                if platform.is_excluded() {
                    TrackState::Ignored
                } else {
                    TrackState::Untracked
                }
            }
            Err(_) => TrackState::Unknown,
        }
    }
}
