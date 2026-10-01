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

/// Entries the pre-open sweep of one git dir may lstat before the repo
/// is declined as not measured (#190). Loose objects and packs are not
/// counted: they are skipped, never opened by the queries swamp asks.
const SWEEP_CAP: usize = 50_000;

/// Repositories declined by the pre-open sweep in this process, with the
/// reason: reported as not measured by `swamp observe`.
static DECLINED: std::sync::Mutex<Vec<(std::path::PathBuf, &'static str)>> =
    std::sync::Mutex::new(Vec::new());

/// The repositories this process declined to open, and why.
pub fn declined() -> Vec<(std::path::PathBuf, &'static str)> {
    DECLINED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn decline(dir: &Path, why: &'static str) -> bool {
    let mut d = DECLINED.lock().unwrap_or_else(|e| e.into_inner());
    if !d.iter().any(|(p, _)| p == dir) {
        d.push((dir.to_path_buf(), why));
    }
    false
}

/// A path that, followed through symlinks, is a FIFO or a device, or is
/// a dataless placeholder: opening it can block. A socket (git's
/// fsmonitor keeps one in `.git`) or a dangling symlink cannot.
fn blocks(path: &Path) -> bool {
    super::read::would_block_on_open(path)
}

/// False when some file gix could open for this repository could block
/// that open forever (#190). gix opens loose refs, reflogs, `info/exclude`,
/// `objects/info/alternates`, `config.worktree` and more with a plain
/// blocking `open(2)`, and which ones depends on the query, so instead of
/// naming them the whole git dir (and a linked worktree's common dir) is
/// swept once with `lstat`, skipping loose objects and packs, and the
/// repository is declined if any entry could block or the sweep passes
/// [`SWEEP_CAP`] entries.
fn safe_to_open(dir: &Path) -> bool {
    use super::read::{BoundedCap, bounded_string};
    let dot_git = dir.join(".git");
    if blocks(&dot_git) {
        return decline(dir, "its .git is not a regular file or directory");
    }
    let git_dir = match super::symlink_metadata(&dot_git) {
        Ok(m) if m.is_file() => match bounded_string(&dot_git, BoundedCap::POINTER) {
            Ok(text) => match text.trim().strip_prefix("gitdir:") {
                Some(p) => dir.join(p.trim()),
                None => return true,
            },
            Err(_) => return decline(dir, "its .git pointer could not be read"),
        },
        Ok(_) => dot_git,
        // A bare/git dir passed directly.
        Err(_) => dir.to_path_buf(),
    };
    let mut dirs = vec![git_dir.clone()];
    if blocks(&git_dir.join("commondir")) {
        return decline(dir, "a file in its git dir could block a read");
    }
    if let Ok(common) = bounded_string(git_dir.join("commondir"), BoundedCap::POINTER) {
        dirs.push(git_dir.join(common.trim()));
    }
    let mut seen = 0usize;
    for d in &dirs {
        match sweep(d, d, &mut seen) {
            Sweep::Clean => {}
            Sweep::Blocking => {
                return decline(dir, "a file in its git dir could block a read");
            }
            Sweep::TooLarge => {
                return decline(
                    dir,
                    "its git dir has more entries than the pre-open check reads",
                );
            }
        }
    }
    true
}

enum Sweep {
    Clean,
    Blocking,
    TooLarge,
}

fn sweep(top: &Path, dir: &Path, seen: &mut usize) -> Sweep {
    let Ok(entries) = super::read_dir(dir) else {
        return Sweep::Clean;
    };
    for entry in entries.flatten() {
        *seen += 1;
        if *seen > SWEEP_CAP {
            return Sweep::TooLarge;
        }
        let path = entry.path();
        if dir == top.join("objects") {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let loose = name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit());
            if loose || name == "pack" {
                continue;
            }
        }
        crate::work_counters::record_git_dir_entries(1);
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            match sweep(top, &path, seen) {
                Sweep::Clean => {}
                other => return other,
            }
        } else if blocks(&path) {
            return Sweep::Blocking;
        }
    }
    Sweep::Clean
}

/// Whether the global/system git config and ignore files gix reads on
/// every open are safe to open; checked once per process. When one could
/// block, repositories are opened isolated (no global or system config).
fn global_config_safe() -> bool {
    static SAFE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SAFE.get_or_init(|| {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".config")));
        let mut files = vec![std::path::PathBuf::from("/etc/gitconfig")];
        if let Some(h) = &home {
            files.push(h.join(".gitconfig"));
        }
        if let Some(x) = &xdg {
            files.push(x.join("git/config"));
            files.push(x.join("git/ignore"));
        }
        !files.iter().any(|f| blocks(f))
    })
}

fn gix_open(dir: &Path) -> Option<gix::Repository> {
    if global_config_safe() {
        gix::open(dir).ok()
    } else {
        gix::open_opts(dir, gix::open::Options::isolated()).ok()
    }
}

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

/// Remote-tracking branches checked per worktree before the answer is
/// "not established" instead of "no".
const TIP_REF_CAP: usize = 400;

impl Repo {
    /// Is HEAD contained in a remote-tracking branch (`refs/remotes/*`,
    /// `*/HEAD` excluded) or in the local default branch? The default
    /// branch (`origin/HEAD`'s target) is checked first, then the other
    /// remote branches, then local `main`/`master`. The worktree's own current local branch is never a
    /// candidate: HEAD is trivially in it. Detached HEAD is fine.
    pub fn tip_reachable(&self, budget: Duration) -> crate::signals::TipReach {
        let repo = &self.0;
        let start = Instant::now();
        let Ok(head_id) = repo.head_id() else {
            return crate::signals::TipReach::Unknown;
        };
        let head = head_id.detach();
        let own = repo
            .head()
            .ok()
            .and_then(|h| h.referent_name().map(|n| n.as_bstr().to_string()));
        let mut candidates: Vec<(String, gix::ObjectId)> = Vec::new();
        let mut first: Vec<String> = Vec::new();
        if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD")
            && let gix::refs::TargetRef::Symbolic(t) = r.target()
        {
            first.push(t.as_bstr().to_string());
        }
        let mut locals: Vec<(String, gix::ObjectId)> = Vec::new();
        for local in ["refs/heads/main", "refs/heads/master"] {
            if own.as_deref() != Some(local)
                && let Ok(mut r) = repo.find_reference(local)
                && let Ok(id) = r.peel_to_id()
            {
                locals.push((short_ref(local), id.detach()));
            }
        }
        for full in &first {
            if let Ok(mut r) = repo.find_reference(full.as_str())
                && let Ok(id) = r.peel_to_id()
            {
                candidates.push((short_ref(full), id.detach()));
            }
        }
        let Ok(platform) = repo.references() else {
            return crate::signals::TipReach::Unknown;
        };
        let Ok(iter) = platform.prefixed("refs/remotes/") else {
            return crate::signals::TipReach::Unknown;
        };
        let mut complete = true;
        let mut seen = 0usize;
        let mut rest: Vec<(String, gix::ObjectId)> = Vec::new();
        for r in iter {
            let Ok(mut r) = r else {
                complete = false;
                continue;
            };
            let full = r.name().as_bstr().to_string();
            if full.ends_with("/HEAD") || first.contains(&full) {
                continue;
            }
            seen += 1;
            if seen > TIP_REF_CAP {
                complete = false;
                break;
            }
            match r.peel_to_id() {
                Ok(id) => rest.push((short_ref(&full), id.detach())),
                Err(_) => complete = false,
            }
        }
        rest.sort();
        candidates.extend(rest);
        // The local default branch last: a remote branch that has the
        // commit is the better name for it.
        candidates.extend(locals);
        for (name, tip) in candidates {
            if start.elapsed() >= budget {
                return crate::signals::TipReach::Unknown;
            }
            if tip == head {
                return crate::signals::TipReach::Reachable(name);
            }
            match repo.merge_base(head, tip) {
                Ok(base) if base.detach() == head => {
                    return crate::signals::TipReach::Reachable(name);
                }
                Ok(_) => {}
                // No common history: not an ancestor.
                Err(gix::repository::merge_base::Error::NotFound { .. }) => {}
                Err(_) => complete = false,
            }
        }
        if complete {
            crate::signals::TipReach::NotReachable
        } else {
            crate::signals::TipReach::Unknown
        }
    }

    /// Opens the repository at (or containing) `dir`.
    pub fn open(dir: &Path) -> Option<Repo> {
        if !safe_to_open(dir) {
            return None;
        }
        gix_open(dir).map(Repo)
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

/// `refs/remotes/origin/x` as `origin/x`, `refs/heads/main` as `main`.
fn short_ref(full: &str) -> String {
    full.strip_prefix("refs/remotes/")
        .or_else(|| full.strip_prefix("refs/heads/"))
        .unwrap_or(full)
        .to_string()
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
        if !safe_to_open(root) {
            return None;
        }
        let repo = gix_open(root)?;
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
        // gix reads each `.gitignore` from the checkout root down to the
        // entry with a blocking open; one that could block makes the
        // answer unknown rather than a wait (#190).
        if let Some(root) = self.repo.workdir() {
            let dirs = if is_dir {
                rel_trimmed
            } else {
                rel_trimmed.rsplit_once('/').map_or("", |(d, _)| d)
            };
            let mut at = root.to_path_buf();
            if blocks(&at.join(".gitignore")) {
                return TrackState::Unknown;
            }
            for part in dirs.split('/').filter(|p| !p.is_empty()) {
                at.push(part);
                if blocks(&at.join(".gitignore")) {
                    return TrackState::Unknown;
                }
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
