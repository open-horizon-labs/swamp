//! GitHub enrichment: branch merge status and pull-request status for a
//! worktree, via `gh`. Every value that cannot be established from a
//! bounded, real call is `Unknown` -- never guessed, never a verdict.
//!
//! Mirrors the `Enricher` shape from `repo-native-alignment`
//! (`is_ready` + `enrich` + provenance/confidence), but synchronous with
//! a timeout rather than async: this project has no async runtime and a
//! shell-out to `gh` is what "provenance" means here.
//!
//! Every network-touching call goes through the [`GithubResponder`] trait
//! so tests can inject canned responses (`GITHUB_RESPONDER` seam) instead
//! of running the real `gh` binary. [`GhCliResponder`] is the production
//! implementation.
//!
//! Two entry points, deliberately separate (see #35 follow-up review):
//!
//! - [`read_cached`]: **never** shells out. Used by `report`/
//!   `--view worktrees` by default -- a report is a read of what's known,
//!   not a trigger for live GitHub calls on every invocation. Stale or
//!   missing cache entries surface as `Unknown` with a note telling the
//!   caller to run `observe`.
//! - [`observe_all`]: does the live work, concurrently (bounded worker
//!   pool) and coalesced per `(owner, repo)` -- one GraphQL call per repo
//!   covers every branch of interest in that repo, rather than 2-3 REST
//!   calls per worktree. This is what `swamp observe` and
//!   `report --enrich` call.

use anyhow::{Context, Result};
use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cache freshness window: re-query only when the tip changed or the
/// cached row is older than this (setting `github_ttl` in
/// `config.toml`, same file `growth.rs` reads).
pub const DEFAULT_GITHUB_TTL_SECS: u64 = 6 * 3600;
/// Bound on any single `gh api graphql` call.
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(8);
/// Overall wall-clock budget for a `swamp observe` (or
/// `report --enrich`) run, regardless of how many repos need a live
/// lookup. Concurrency plus per-repo coalescing is expected to keep real
/// runs well under this.
pub const DEFAULT_RUN_BUDGET_SECS: u64 = 60;
/// How many `(owner, repo)` lookups run at once. `gh`'s GraphQL rate
/// limit is 5000 points/hour, so this is generous headroom, not a limit
/// dictated by GitHub.
pub const DEFAULT_CONCURRENCY: usize = 8;

// ---------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergedStatus {
    Yes {
        merged_at: Option<String>,
        pr_number: Option<u64>,
    },
    No,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
    None,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: u64,
    pub state: PrState,
    pub draft: bool,
    pub url: String,
    /// Truncated to 60 chars (see `truncate_title`).
    pub title: String,
    pub review_decision: ReviewDecision,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrStatus {
    Some(PullRequestInfo),
    None,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubFacts {
    pub default_branch: Option<String>,
    pub branch_exists_on_remote: Option<bool>,
    pub merged: MergedStatus,
    pub pull_request: PrStatus,
    /// Set when this row is not a fresh live result: `gh` unavailable, a
    /// per-repo lookup failure, a budget cutoff, a stale cache entry, or
    /// "never enriched". Surfaced once per distinct reason in
    /// `Report.notes`, and per-row here so a caller can explain any one
    /// worktree without re-reading notes.
    pub unavailable_reason: Option<String>,
}

impl GithubFacts {
    pub fn unknown(reason: Option<String>) -> Self {
        Self {
            default_branch: None,
            branch_exists_on_remote: None,
            merged: MergedStatus::Unknown,
            pull_request: PrStatus::Unknown,
            unavailable_reason: reason,
        }
    }
}

fn truncate_title(title: &str) -> String {
    if title.chars().count() <= 60 {
        title.to_string()
    } else {
        let mut s: String = title.chars().take(57).collect();
        s.push_str("...");
        s
    }
}

fn human_age(secs: u64) -> String {
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

// ---------------------------------------------------------------------
// Responder seam
// ---------------------------------------------------------------------

/// One branch's GitHub-side facts within a [`RepoBatchResult`].
#[derive(Debug, Clone)]
pub struct BranchResult {
    /// Whether `refs/heads/<branch>` currently exists on the remote.
    pub exists: bool,
    /// The most recently updated PR (any state) whose head was this
    /// branch, if any.
    pub pr: Option<PullRequestInfo>,
    /// Set alongside `pr` when that PR's state is `Merged`.
    pub merged_at: Option<String>,
}

/// One `(owner, repo)` lookup's result: the default branch (fetched once
/// regardless of how many branches were asked about) plus one
/// [`BranchResult`] per requested branch.
#[derive(Debug, Clone, Default)]
pub struct RepoBatchResult {
    pub default_branch: Option<String>,
    pub branches: HashMap<String, BranchResult>,
}

/// One GitHub-facing call surface, injectable for tests. Implementations
/// must be `Sync`: [`observe_all`] calls `repo_batch` from a bounded
/// worker pool.
pub trait GithubResponder: Sync {
    /// Whether this responder can answer at all right now (e.g. `gh` is
    /// installed and authenticated). `Err` carries the reason to record
    /// once in `Report.notes`.
    fn is_ready(&self) -> Result<(), String>;
    /// One coalesced lookup for every `branches` entry of `owner/repo`:
    /// the default branch plus, per branch, whether it still exists on
    /// the remote and its most relevant PR (open, closed, or merged).
    fn repo_batch(
        &self,
        owner: &str,
        repo: &str,
        branches: &[String],
    ) -> Result<RepoBatchResult, String>;
}

/// Production responder: shells out to `gh`, bounded by
/// [`PER_CALL_TIMEOUT`]. Missing/unauthenticated/offline `gh` surfaces as
/// `Err(reason)` from every call rather than a panic.
pub struct GhCliResponder;

fn bounded_gh(args: &[String]) -> Result<String, String> {
    let out =
        crate::fs_gate::spawn::run(crate::fs_gate::spawn::Program::Gh, args, PER_CALL_TIMEOUT)
            .map_err(|e| format!("gh not runnable: {e}"))?;
    if out.timed_out {
        return Err(format!("gh {args:?} timed out after {PER_CALL_TIMEOUT:?}"));
    }
    if !out.success() {
        return Err(format!("gh {args:?} failed: {}", out.stderr_lossy().trim()));
    }
    Ok(out.stdout_lossy())
}

/// Builds the GraphQL query text and `gh api graphql -f ...` argument
/// list for one `(owner, repo)` batch covering `branches`. One aliased
/// `ref`/`pullRequests` pair per branch, indexed `r0`/`p0`, `r1`/`p1`,
/// ...; `defaultBranchRef` is fetched once regardless of branch count.
/// Branch names are passed as GraphQL variables (never interpolated
/// into the query text) so odd characters in a branch name can't break
/// or inject into the query.
fn build_graphql_args(owner: &str, repo: &str, branches: &[String]) -> Vec<String> {
    let mut query = String::from("query($owner: String!, $name: String!");
    for i in 0..branches.len() {
        query.push_str(&format!(", $branch{i}: String!, $ref{i}: String!"));
    }
    query.push_str(
        ") {\n  repository(owner: $owner, name: $name) {\n    defaultBranchRef { name }\n",
    );
    for i in 0..branches.len() {
        query.push_str(&format!(
            "    r{i}: ref(qualifiedName: $ref{i}) {{ name }}\n\
             p{i}: pullRequests(headRefName: $branch{i}, states: [OPEN, CLOSED, MERGED], first: 1, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{\n\
             nodes {{ number state isDraft url title reviewDecision updatedAt mergedAt }}\n    }}\n"
        ));
    }
    query.push_str("  }\n}");

    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={query}"),
        "-f".to_string(),
        format!("owner={owner}"),
        "-f".to_string(),
        format!("name={repo}"),
    ];
    for (i, branch) in branches.iter().enumerate() {
        args.push("-f".to_string());
        args.push(format!("branch{i}={branch}"));
        args.push("-f".to_string());
        args.push(format!("ref{i}=refs/heads/{branch}"));
    }
    args
}

fn parse_repo_batch(json_text: &str, branches: &[String]) -> Result<RepoBatchResult, String> {
    let v: serde_json::Value =
        serde_json::from_str(json_text).map_err(|e| format!("parse graphql response: {e}"))?;
    if let Some(errors) = v.get("errors") {
        return Err(format!("graphql errors: {errors}"));
    }
    let repo = v
        .get("data")
        .and_then(|d| d.get("repository"))
        .ok_or_else(|| "missing data.repository in graphql response".to_string())?;
    let default_branch = repo
        .get("defaultBranchRef")
        .and_then(|r| r.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from);

    let mut out = HashMap::new();
    for (i, branch) in branches.iter().enumerate() {
        let exists = repo
            .get(format!("r{i}"))
            .map(|r| !r.is_null())
            .unwrap_or(false);
        let pr_node = repo
            .get(format!("p{i}"))
            .and_then(|p| p.get("nodes"))
            .and_then(|n| n.as_array())
            .and_then(|a| a.first());
        let (pr, merged_at) = match pr_node {
            Some(node) => {
                let number = node.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                let raw_state = node.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let merged_at_val = node
                    .get("mergedAt")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let state = if raw_state.eq_ignore_ascii_case("MERGED") {
                    PrState::Merged
                } else if raw_state.eq_ignore_ascii_case("CLOSED") {
                    PrState::Closed
                } else {
                    PrState::Open
                };
                let draft = node
                    .get("isDraft")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let url = node
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title =
                    truncate_title(node.get("title").and_then(|v| v.as_str()).unwrap_or(""));
                // GraphQL returns null for `reviewDecision` when no
                // review is required, same meaning the old REST path's
                // empty string had.
                let review_decision = match node.get("reviewDecision").and_then(|v| v.as_str()) {
                    Some("APPROVED") => ReviewDecision::Approved,
                    Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
                    Some("REVIEW_REQUIRED") => ReviewDecision::ReviewRequired,
                    _ => ReviewDecision::None,
                };
                let updated_at = node
                    .get("updatedAt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (
                    Some(PullRequestInfo {
                        number,
                        state,
                        draft,
                        url,
                        title,
                        review_decision,
                        updated_at,
                    }),
                    merged_at_val,
                )
            }
            None => (None, None),
        };
        out.insert(
            branch.clone(),
            BranchResult {
                exists,
                pr,
                merged_at,
            },
        );
    }
    Ok(RepoBatchResult {
        default_branch,
        branches: out,
    })
}

impl GithubResponder for GhCliResponder {
    fn is_ready(&self) -> Result<(), String> {
        bounded_gh(&["auth".to_string(), "status".to_string()]).map(|_| ())
    }

    fn repo_batch(
        &self,
        owner: &str,
        repo: &str,
        branches: &[String],
    ) -> Result<RepoBatchResult, String> {
        if branches.is_empty() {
            return Ok(RepoBatchResult::default());
        }
        let args = build_graphql_args(owner, repo, branches);
        let out = bounded_gh(&args)?;
        parse_repo_batch(&out, branches)
    }
}

// ---------------------------------------------------------------------
// Remote parsing
// ---------------------------------------------------------------------

/// Parses `owner/repo` out of a git remote URL, but only when its host is
/// github.com. Accepts `git@github.com:owner/repo(.git)` and
/// `https://github.com/owner/repo(.git)` forms.
pub fn github_owner_repo(remote_url: &str) -> Option<(String, String)> {
    let url = remote_url
        .trim()
        .strip_suffix(".git")
        .unwrap_or(remote_url.trim());
    let rest = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let idx = url.find("://")?;
        let after_scheme = &url[idx + 3..];
        let after_scheme = after_scheme.rsplit('@').next().unwrap_or(after_scheme);
        after_scheme.strip_prefix("github.com/")?
    };
    let mut parts = rest.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches('/').to_string();
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some((owner, repo))
    }
}

/// Builds `GithubFacts` for one branch out of its repo's batch result.
/// Pure and call-free: `observe_all` calls this once per worktree after
/// its repo's single `repo_batch` call returns.
fn facts_from_batch(batch: &RepoBatchResult, branch: &str) -> GithubFacts {
    let default_branch = batch.default_branch.clone();
    let branch_result = batch.branches.get(branch);
    let branch_exists_on_remote = branch_result.map(|b| b.exists);

    let merged = match (&default_branch, branch_result) {
        (Some(default), _) if default == branch => MergedStatus::No,
        (_, Some(b)) => match &b.pr {
            Some(pr) if pr.state == PrState::Merged => MergedStatus::Yes {
                merged_at: b.merged_at.clone(),
                pr_number: Some(pr.number),
            },
            Some(_) => MergedStatus::No,
            // No PR at all: without a compare-style ancestry check we
            // cannot tell whether this branch merged some other way
            // (rebase/manual push to default), so this stays Unknown
            // rather than guessing No.
            None => MergedStatus::Unknown,
        },
        (_, None) => MergedStatus::Unknown,
    };

    let pull_request = match branch_result {
        Some(b) => match &b.pr {
            Some(pr) => PrStatus::Some(pr.clone()),
            None => PrStatus::None,
        },
        None => PrStatus::Unknown,
    };

    GithubFacts {
        default_branch,
        branch_exists_on_remote,
        merged,
        pull_request,
        unavailable_reason: None,
    }
}

/// Reads the current branch name from a worktree's `HEAD` file/ref.
/// Returns `None` for a detached HEAD (or any parse failure), which the
/// caller treats identically: nothing to ask GitHub about.
pub fn current_branch(worktree_dir: &Path) -> Option<String> {
    let head_path = worktree_dir.join(".git").join("HEAD");
    // A linked worktree's `.git` is a file, not a directory; `HEAD` lives
    // in its admin dir instead.
    let pointer = |p: &Path| {
        crate::fs_gate::read::bounded_string(p, crate::fs_gate::read::BoundedCap::POINTER).ok()
    };
    let head_content = if crate::fs_gate::exists(&head_path) {
        pointer(&head_path)?
    } else {
        let git_file = worktree_dir.join(".git");
        let contents = pointer(&git_file)?;
        let admin = contents.trim().strip_prefix("gitdir:")?.trim();
        pointer(&Path::new(admin).join("HEAD"))?
    };
    let line = head_content.trim();
    line.strip_prefix("ref: refs/heads/").map(String::from)
}

// ---------------------------------------------------------------------
// Cache: enrich.parquet
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CacheRow {
    worktree_id: String,
    branch: String,
    tip_sha: String,
    observed_at: u64,
    default_branch: String,
    branch_exists: String, // "yes" | "no" | "unknown"
    merged_status: String, // "yes" | "no" | "unknown"
    merged_at: String,
    merged_pr_number: i64, // -1 = none
    pr_status: String,     // "some" | "none" | "unknown"
    pr_number: i64,        // -1 = n/a
    pr_state: String,
    pr_draft: String, // "true" | "false" | ""
    pr_url: String,
    pr_title: String,
    pr_review_decision: String,
    pr_updated_at: String,
    unavailable_reason: String,
}

fn cache_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("worktree_id", DataType::Utf8, false),
        Field::new("branch", DataType::Utf8, false),
        Field::new("tip_sha", DataType::Utf8, false),
        Field::new("observed_at", DataType::UInt64, false),
        Field::new("default_branch", DataType::Utf8, false),
        Field::new("branch_exists", DataType::Utf8, false),
        Field::new("merged_status", DataType::Utf8, false),
        Field::new("merged_at", DataType::Utf8, false),
        Field::new("merged_pr_number", DataType::Int64, false),
        Field::new("pr_status", DataType::Utf8, false),
        Field::new("pr_number", DataType::Int64, false),
        Field::new("pr_state", DataType::Utf8, false),
        Field::new("pr_draft", DataType::Utf8, false),
        Field::new("pr_url", DataType::Utf8, false),
        Field::new("pr_title", DataType::Utf8, false),
        Field::new("pr_review_decision", DataType::Utf8, false),
        Field::new("pr_updated_at", DataType::Utf8, false),
        Field::new("unavailable_reason", DataType::Utf8, false),
    ]))
}

fn cache_path(swamp_dir: &Path, volume_id: u64) -> PathBuf {
    swamp_dir.join(volume_id.to_string()).join("enrich.parquet")
}

fn read_cache(path: &Path) -> Vec<CacheRow> {
    let Ok(Some(reader)) = crate::fs_gate::columns::open_parquet(path) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for batch in reader.flatten() {
        let get_str = |name: &str| -> Vec<String> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect())
                .unwrap_or_default()
        };
        let get_u64 = |name: &str| -> Vec<u64> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect())
                .unwrap_or_default()
        };
        let get_i64 = |name: &str| -> Vec<i64> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect())
                .unwrap_or_default()
        };
        let worktree_id = get_str("worktree_id");
        let branch = get_str("branch");
        let tip_sha = get_str("tip_sha");
        let observed_at = get_u64("observed_at");
        let default_branch = get_str("default_branch");
        let branch_exists = get_str("branch_exists");
        let merged_status = get_str("merged_status");
        let merged_at = get_str("merged_at");
        let merged_pr_number = get_i64("merged_pr_number");
        let pr_status = get_str("pr_status");
        let pr_number = get_i64("pr_number");
        let pr_state = get_str("pr_state");
        let pr_draft = get_str("pr_draft");
        let pr_url = get_str("pr_url");
        let pr_title = get_str("pr_title");
        let pr_review_decision = get_str("pr_review_decision");
        let pr_updated_at = get_str("pr_updated_at");
        let unavailable_reason = get_str("unavailable_reason");
        for i in 0..worktree_id.len() {
            rows.push(CacheRow {
                worktree_id: worktree_id[i].clone(),
                branch: branch[i].clone(),
                tip_sha: tip_sha[i].clone(),
                observed_at: observed_at[i],
                default_branch: default_branch[i].clone(),
                branch_exists: branch_exists[i].clone(),
                merged_status: merged_status[i].clone(),
                merged_at: merged_at[i].clone(),
                merged_pr_number: merged_pr_number[i],
                pr_status: pr_status[i].clone(),
                pr_number: pr_number[i],
                pr_state: pr_state[i].clone(),
                pr_draft: pr_draft[i].clone(),
                pr_url: pr_url[i].clone(),
                pr_title: pr_title[i].clone(),
                pr_review_decision: pr_review_decision[i].clone(),
                pr_updated_at: pr_updated_at[i].clone(),
                unavailable_reason: unavailable_reason[i].clone(),
            });
        }
    }
    rows
}

fn write_cache(path: &Path, rows: &[CacheRow]) -> Result<()> {
    let schema = cache_schema();
    macro_rules! col {
        ($f:ident) => {
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.$f.as_str()).collect::<Vec<_>>(),
            )) as ArrayRef
        };
    }
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            col!(worktree_id),
            col!(branch),
            col!(tip_sha),
            Arc::new(UInt64Array::from(
                rows.iter().map(|r| r.observed_at).collect::<Vec<_>>(),
            )),
            col!(default_branch),
            col!(branch_exists),
            col!(merged_status),
            col!(merged_at),
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.merged_pr_number).collect::<Vec<_>>(),
            )),
            col!(pr_status),
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.pr_number).collect::<Vec<_>>(),
            )),
            col!(pr_state),
            col!(pr_draft),
            col!(pr_url),
            col!(pr_title),
            col!(pr_review_decision),
            col!(pr_updated_at),
            col!(unavailable_reason),
        ],
    )?;
    crate::fs_gate::columns::write_parquet_atomic(
        path,
        schema,
        std::iter::once(Ok(batch)),
        crate::fs_gate::columns::DEFAULT_ZSTD_LEVEL,
    )
    .with_context(|| format!("create {}", path.display()))?;
    Ok(())
}

fn facts_to_row(
    worktree_id: &str,
    branch: &str,
    tip_sha: &str,
    observed_at: u64,
    facts: &GithubFacts,
) -> CacheRow {
    let (merged_status, merged_at, merged_pr_number) = match &facts.merged {
        MergedStatus::Yes {
            merged_at,
            pr_number,
        } => (
            "yes".to_string(),
            merged_at.clone().unwrap_or_default(),
            pr_number.map(|n| n as i64).unwrap_or(-1),
        ),
        MergedStatus::No => ("no".to_string(), String::new(), -1),
        MergedStatus::Unknown => ("unknown".to_string(), String::new(), -1),
    };
    let (
        pr_status,
        pr_number,
        pr_state,
        pr_draft,
        pr_url,
        pr_title,
        pr_review_decision,
        pr_updated_at,
    ) = match &facts.pull_request {
        PrStatus::Some(pr) => (
            "some".to_string(),
            pr.number as i64,
            match pr.state {
                PrState::Open => "open",
                PrState::Closed => "closed",
                PrState::Merged => "merged",
            }
            .to_string(),
            pr.draft.to_string(),
            pr.url.clone(),
            pr.title.clone(),
            match pr.review_decision {
                ReviewDecision::Approved => "approved",
                ReviewDecision::ChangesRequested => "changes_requested",
                ReviewDecision::ReviewRequired => "review_required",
                ReviewDecision::None => "none",
                ReviewDecision::Unknown => "unknown",
            }
            .to_string(),
            pr.updated_at.clone(),
        ),
        PrStatus::None => (
            "none".to_string(),
            -1,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ),
        PrStatus::Unknown => (
            "unknown".to_string(),
            -1,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ),
    };
    CacheRow {
        worktree_id: worktree_id.to_string(),
        branch: branch.to_string(),
        tip_sha: tip_sha.to_string(),
        observed_at,
        default_branch: facts.default_branch.clone().unwrap_or_default(),
        branch_exists: match facts.branch_exists_on_remote {
            Some(true) => "yes".to_string(),
            Some(false) => "no".to_string(),
            None => "unknown".to_string(),
        },
        merged_status,
        merged_at,
        merged_pr_number,
        pr_status,
        pr_number,
        pr_state,
        pr_draft,
        pr_url,
        pr_title,
        pr_review_decision,
        pr_updated_at,
        unavailable_reason: facts.unavailable_reason.clone().unwrap_or_default(),
    }
}

fn row_to_facts(row: &CacheRow) -> GithubFacts {
    let default_branch = if row.default_branch.is_empty() {
        None
    } else {
        Some(row.default_branch.clone())
    };
    let branch_exists_on_remote = match row.branch_exists.as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    };
    let merged = match row.merged_status.as_str() {
        "yes" => MergedStatus::Yes {
            merged_at: if row.merged_at.is_empty() {
                None
            } else {
                Some(row.merged_at.clone())
            },
            pr_number: if row.merged_pr_number < 0 {
                None
            } else {
                Some(row.merged_pr_number as u64)
            },
        },
        "no" => MergedStatus::No,
        _ => MergedStatus::Unknown,
    };
    let pull_request = match row.pr_status.as_str() {
        "some" => PrStatus::Some(PullRequestInfo {
            number: row.pr_number.max(0) as u64,
            state: match row.pr_state.as_str() {
                "open" => PrState::Open,
                "closed" => PrState::Closed,
                "merged" => PrState::Merged,
                _ => PrState::Open,
            },
            draft: row.pr_draft == "true",
            url: row.pr_url.clone(),
            title: row.pr_title.clone(),
            review_decision: match row.pr_review_decision.as_str() {
                "approved" => ReviewDecision::Approved,
                "changes_requested" => ReviewDecision::ChangesRequested,
                "review_required" => ReviewDecision::ReviewRequired,
                "none" => ReviewDecision::None,
                _ => ReviewDecision::Unknown,
            },
            updated_at: row.pr_updated_at.clone(),
        }),
        "none" => PrStatus::None,
        _ => PrStatus::Unknown,
    };
    GithubFacts {
        default_branch,
        branch_exists_on_remote,
        merged,
        pull_request,
        unavailable_reason: if row.unavailable_reason.is_empty() {
            None
        } else {
            Some(row.unavailable_reason.clone())
        },
    }
}

/// One worktree's enrichment input: identity for caching plus what's
/// needed to make a live call.
pub struct EnrichInput<'a> {
    pub worktree_id: &'a str,
    pub tip_sha: &'a str,
    pub branch: Option<&'a str>,
    pub owner: &'a str,
    pub repo: &'a str,
}

/// **Never shells out.** Reads `enrich.parquet` under
/// `swamp_dir/<volume_id>/` for every worktree in `inputs`. A fresh
/// row (younger than `ttl_secs`, same tip) is returned as-is; a stale
/// row or a missing one comes back as `Unknown` with a note telling the
/// caller to run `observe`. This is what `report`/`--view worktrees` use
/// by default (see the module doc for why).
pub fn read_cached(
    swamp_dir: &Path,
    volume_id: u64,
    inputs: &[EnrichInput],
    observed_at: u64,
    ttl_secs: u64,
) -> (HashMap<String, GithubFacts>, Vec<String>) {
    let path = cache_path(swamp_dir, volume_id);
    let cache: HashMap<(String, String, String), CacheRow> = read_cache(&path)
        .into_iter()
        .map(|r| {
            (
                (r.worktree_id.clone(), r.branch.clone(), r.tip_sha.clone()),
                r,
            )
        })
        .collect();

    let mut out = HashMap::new();
    let mut stale = 0u32;
    let mut missing = 0u32;
    for input in inputs {
        let key = (
            input.worktree_id.to_string(),
            input.branch.unwrap_or("").to_string(),
            input.tip_sha.to_string(),
        );
        match cache.get(&key) {
            Some(row) => {
                let age = observed_at.saturating_sub(row.observed_at);
                if age < ttl_secs {
                    out.insert(input.worktree_id.to_string(), row_to_facts(row));
                } else {
                    stale += 1;
                    out.insert(
                        input.worktree_id.to_string(),
                        GithubFacts::unknown(Some(format!("github: cache {} old", human_age(age)))),
                    );
                }
            }
            None => {
                missing += 1;
                out.insert(
                    input.worktree_id.to_string(),
                    GithubFacts::unknown(Some(
                        "github: not enriched (run `swamp observe` or wait for the schedule)"
                            .to_string(),
                    )),
                );
            }
        }
    }

    let mut notes = Vec::new();
    if stale > 0 {
        notes.push(format!(
            "github: {stale} worktree(s) have cached enrichment older than the refresh window (run `swamp observe` to refresh)"
        ));
    }
    if missing > 0 {
        notes.push(format!(
            "github: {missing} worktree(s) not enriched (run `swamp observe` or wait for the schedule)"
        ));
    }
    (out, notes)
}

/// Result of one [`observe_all`] run.
pub struct ObserveSummary {
    /// Number of `repo_batch` calls actually made (one per distinct
    /// `(owner, repo)` that needed a refresh).
    pub calls_made: u32,
    /// Number of worktrees whose cache row was refreshed by a live call.
    pub worktrees_enriched: u32,
    pub notes: Vec<String>,
}

/// Live GitHub enrichment: groups `inputs` by `(owner, repo)`, skips any
/// worktree whose cache entry is already fresh, then runs the remaining
/// repo lookups concurrently (bounded to `concurrency`) against
/// [`GithubResponder::repo_batch`] -- one call per repo covers every
/// branch of interest in it, regardless of how many worktrees in
/// `inputs` share that repo. Bounded overall by `budget_secs`: any repo
/// not dispatched before the deadline gets `Unknown` for its worktrees
/// instead of blocking the run. Writes every result (live or
/// budget-cut) back into `enrich.parquet`.
#[allow(clippy::too_many_arguments)]
pub fn observe_all(
    responder: &(dyn GithubResponder + Sync),
    swamp_dir: &Path,
    volume_id: u64,
    inputs: &[EnrichInput],
    observed_at: u64,
    ttl_secs: u64,
    budget_secs: u64,
    concurrency: usize,
) -> ObserveSummary {
    let path = cache_path(swamp_dir, volume_id);
    let mut cache: HashMap<(String, String, String), CacheRow> = read_cache(&path)
        .into_iter()
        .map(|r| {
            (
                (r.worktree_id.clone(), r.branch.clone(), r.tip_sha.clone()),
                r,
            )
        })
        .collect();
    let mut notes: Vec<String> = Vec::new();

    if let Err(reason) = responder.is_ready() {
        let msg = format!("github: unavailable ({reason})");
        notes.push(msg.clone());
        for input in inputs {
            let key = (
                input.worktree_id.to_string(),
                input.branch.unwrap_or("").to_string(),
                input.tip_sha.to_string(),
            );
            cache.insert(
                key,
                facts_to_row(
                    input.worktree_id,
                    input.branch.unwrap_or(""),
                    input.tip_sha,
                    observed_at,
                    &GithubFacts::unknown(Some(msg.clone())),
                ),
            );
        }
        let rows: Vec<CacheRow> = cache.into_values().collect();
        let _ = write_cache(&path, &rows);
        return ObserveSummary {
            calls_made: 0,
            worktrees_enriched: 0,
            notes,
        };
    }

    struct PendingGroup {
        owner: String,
        repo: String,
        branches: Vec<String>,
        // (worktree_id, branch, tip_sha)
        entries: Vec<(String, String, String)>,
    }

    let mut groups: HashMap<(String, String), PendingGroup> = HashMap::new();
    for input in inputs {
        // Detached HEAD: nothing to ask GitHub about.
        let Some(branch) = input.branch else { continue };
        let key = (
            input.worktree_id.to_string(),
            branch.to_string(),
            input.tip_sha.to_string(),
        );
        if let Some(row) = cache.get(&key)
            && observed_at.saturating_sub(row.observed_at) < ttl_secs
        {
            continue; // already fresh: no call needed for this worktree.
        }
        let gkey = (input.owner.to_string(), input.repo.to_string());
        let group = groups.entry(gkey).or_insert_with(|| PendingGroup {
            owner: input.owner.to_string(),
            repo: input.repo.to_string(),
            branches: Vec::new(),
            entries: Vec::new(),
        });
        if !group.branches.iter().any(|b| b == branch) {
            group.branches.push(branch.to_string());
        }
        group.entries.push((
            input.worktree_id.to_string(),
            branch.to_string(),
            input.tip_sha.to_string(),
        ));
    }

    if groups.is_empty() {
        return ObserveSummary {
            calls_made: 0,
            worktrees_enriched: 0,
            notes,
        };
    }

    let deadline = Instant::now() + Duration::from_secs(budget_secs);
    let group_keys: Vec<(String, String)> = groups.keys().cloned().collect();
    let work: Mutex<Vec<(String, String)>> = Mutex::new(group_keys);
    let results: Mutex<HashMap<(String, String), Result<RepoBatchResult, String>>> =
        Mutex::new(HashMap::new());
    let calls_made = AtomicU32::new(0);

    std::thread::scope(|scope| {
        let workers = concurrency.max(1);
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if Instant::now() >= deadline {
                        break;
                    }
                    let next = { work.lock().unwrap().pop() };
                    let Some(gkey) = next else { break };
                    let Some(group) = groups.get(&gkey) else {
                        continue;
                    };
                    let res = responder.repo_batch(&group.owner, &group.repo, &group.branches);
                    calls_made.fetch_add(1, Ordering::Relaxed);
                    results.lock().unwrap().insert(gkey, res);
                }
            });
        }
    });

    let results = results.into_inner().unwrap();
    let mut enriched = 0u32;
    for (gkey, group) in &groups {
        match results.get(gkey) {
            Some(Ok(batch)) => {
                for (worktree_id, branch, tip_sha) in &group.entries {
                    let facts = facts_from_batch(batch, branch);
                    let key = (worktree_id.clone(), branch.clone(), tip_sha.clone());
                    cache.insert(
                        key,
                        facts_to_row(worktree_id, branch, tip_sha, observed_at, &facts),
                    );
                    enriched += 1;
                }
            }
            Some(Err(e)) => {
                let reason = format!("github: {}/{} lookup failed ({e})", gkey.0, gkey.1);
                if !notes.contains(&reason) {
                    notes.push(reason.clone());
                }
                for (worktree_id, branch, tip_sha) in &group.entries {
                    let key = (worktree_id.clone(), branch.clone(), tip_sha.clone());
                    cache.insert(
                        key,
                        facts_to_row(
                            worktree_id,
                            branch,
                            tip_sha,
                            observed_at,
                            &GithubFacts::unknown(Some(reason.clone())),
                        ),
                    );
                }
            }
            None => {
                let reason = "github: run budget exhausted before this repo".to_string();
                if !notes.contains(&reason) {
                    notes.push(reason.clone());
                }
                for (worktree_id, branch, tip_sha) in &group.entries {
                    let key = (worktree_id.clone(), branch.clone(), tip_sha.clone());
                    cache.insert(
                        key,
                        facts_to_row(
                            worktree_id,
                            branch,
                            tip_sha,
                            observed_at,
                            &GithubFacts::unknown(Some(reason.clone())),
                        ),
                    );
                }
            }
        }
    }

    let rows: Vec<CacheRow> = cache.into_values().collect();
    let _ = write_cache(&path, &rows);

    ObserveSummary {
        calls_made: calls_made.load(Ordering::Relaxed),
        worktrees_enriched: enriched,
        notes,
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// Canned responder for tests: `repo_batch` returns a table entry
    /// keyed by `(owner, repo)`, `is_ready` succeeds unless
    /// `unready_reason` is set.
    #[derive(Default)]
    pub struct FakeResponder {
        pub unready_reason: Option<String>,
        pub batches: Mutex<HashMap<(String, String), Result<RepoBatchResult, String>>>,
        pub calls: Mutex<u32>,
    }

    impl FakeResponder {
        pub fn set_batch(&self, owner: &str, repo: &str, result: RepoBatchResult) {
            self.batches
                .lock()
                .unwrap()
                .insert((owner.to_string(), repo.to_string()), Ok(result));
        }
    }

    impl GithubResponder for FakeResponder {
        fn is_ready(&self) -> Result<(), String> {
            match &self.unready_reason {
                Some(r) => Err(r.clone()),
                None => Ok(()),
            }
        }
        fn repo_batch(
            &self,
            owner: &str,
            repo: &str,
            _branches: &[String],
        ) -> Result<RepoBatchResult, String> {
            *self.calls.lock().unwrap() += 1;
            self.batches
                .lock()
                .unwrap()
                .get(&(owner.to_string(), repo.to_string()))
                .cloned()
                .unwrap_or_else(|| Err("no batch configured".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeResponder;
    use super::*;

    fn pr(number: u64, state: PrState) -> PullRequestInfo {
        PullRequestInfo {
            number,
            state,
            draft: false,
            url: format!("https://github.com/acme/widgets/pull/{number}"),
            title: "Add feature".to_string(),
            review_decision: ReviewDecision::Approved,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn github_owner_repo_parses_ssh_and_https() {
        assert_eq!(
            github_owner_repo("git@github.com:acme/widgets.git"),
            Some(("acme".to_string(), "widgets".to_string()))
        );
        assert_eq!(
            github_owner_repo("https://github.com/acme/widgets"),
            Some(("acme".to_string(), "widgets".to_string()))
        );
        assert_eq!(github_owner_repo("git@gitlab.com:acme/widgets.git"), None);
    }

    #[test]
    fn detached_head_is_unknown_without_any_call() {
        let responder = FakeResponder::default();
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: None,
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        let summary = observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        assert_eq!(summary.calls_made, 0);
        assert_eq!(*responder.calls.lock().unwrap(), 0);
    }

    #[test]
    fn unavailable_gh_marks_everything_unknown_with_reason() {
        let responder = FakeResponder {
            unready_reason: Some("not logged in".to_string()),
            ..Default::default()
        };
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        let (facts, notes) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        // read_cached never calls gh at all -- unenriched cache means
        // "not enriched", not "gh unavailable" (that's observe_all's
        // failure mode).
        assert!(
            facts["wt1"]
                .unavailable_reason
                .as_ref()
                .unwrap()
                .contains("not enriched")
        );
        assert!(!notes.is_empty());

        let summary = observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        assert!(summary.notes.iter().any(|n| n.contains("not logged in")));
        let (facts2, _) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(facts2["wt1"].merged, MergedStatus::Unknown);
    }

    #[test]
    fn merged_pr_gives_merged_yes_with_pr_number() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([(
                    "feature".to_string(),
                    BranchResult {
                        exists: true,
                        pr: Some(pr(42, PrState::Merged)),
                        merged_at: Some("2026-01-01T00:00:00Z".to_string()),
                    },
                )]),
            },
        );
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        let (facts, _) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(
            facts["wt1"].merged,
            MergedStatus::Yes {
                merged_at: Some("2026-01-01T00:00:00Z".to_string()),
                pr_number: Some(42)
            }
        );
    }

    #[test]
    fn open_pr_is_merged_no_and_surfaced() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([(
                    "feature".to_string(),
                    BranchResult {
                        exists: true,
                        pr: Some(pr(123, PrState::Open)),
                        merged_at: None,
                    },
                )]),
            },
        );
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        let (facts, _) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(facts["wt1"].merged, MergedStatus::No);
        match &facts["wt1"].pull_request {
            PrStatus::Some(pr) => assert_eq!(pr.number, 123),
            other => panic!("expected Some PR, got {other:?}"),
        }
    }

    #[test]
    fn no_pull_request_is_none_not_unknown() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([(
                    "feature".to_string(),
                    BranchResult {
                        exists: true,
                        pr: None,
                        merged_at: None,
                    },
                )]),
            },
        );
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        let (facts, _) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(facts["wt1"].pull_request, PrStatus::None);
    }

    #[test]
    fn one_call_covers_every_branch_of_a_shared_repo() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([
                    (
                        "feature-a".to_string(),
                        BranchResult {
                            exists: true,
                            pr: None,
                            merged_at: None,
                        },
                    ),
                    (
                        "feature-b".to_string(),
                        BranchResult {
                            exists: true,
                            pr: Some(pr(9, PrState::Merged)),
                            merged_at: Some("2026-01-01T00:00:00Z".to_string()),
                        },
                    ),
                ]),
            },
        );
        let inputs = vec![
            EnrichInput {
                worktree_id: "wt1",
                tip_sha: "sha1",
                branch: Some("feature-a"),
                owner: "acme",
                repo: "widgets",
            },
            EnrichInput {
                worktree_id: "wt2",
                tip_sha: "sha2",
                branch: Some("feature-b"),
                owner: "acme",
                repo: "widgets",
            },
        ];
        let tmp = tempfile::tempdir().unwrap();
        let summary = observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        assert_eq!(summary.calls_made, 1, "one repo, one call, both branches");
        assert_eq!(*responder.calls.lock().unwrap(), 1);
        let (facts, _) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(facts["wt1"].merged, MergedStatus::Unknown);
        assert!(matches!(facts["wt2"].merged, MergedStatus::Yes { .. }));
    }

    #[test]
    fn report_never_calls_the_responder() {
        // read_cached takes no responder argument at all -- there is no
        // way for a caller of it to reach `gh`. This test exists to make
        // that contract explicit and regression-proof.
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        let (facts, notes) = read_cached(tmp.path(), 0, &inputs, 1000, 3600);
        assert_eq!(facts["wt1"].merged, MergedStatus::Unknown);
        assert!(notes.iter().any(|n| n.contains("not enriched")));
    }

    #[test]
    fn cache_hit_avoids_a_second_responder_call() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([(
                    "feature".to_string(),
                    BranchResult {
                        exists: true,
                        pr: Some(pr(42, PrState::Merged)),
                        merged_at: None,
                    },
                )]),
            },
        );
        let inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        let calls_after_first = *responder.calls.lock().unwrap();
        assert_eq!(calls_after_first, 1);

        // Same tip_sha, still within TTL: must not call the responder again.
        observe_all(&responder, tmp.path(), 0, &inputs, 1050, 3600, 20, 4);
        assert_eq!(*responder.calls.lock().unwrap(), calls_after_first);
    }

    #[test]
    fn tip_change_invalidates_the_cache() {
        let responder = FakeResponder::default();
        responder.set_batch(
            "acme",
            "widgets",
            RepoBatchResult {
                default_branch: Some("main".to_string()),
                branches: HashMap::from([(
                    "feature".to_string(),
                    BranchResult {
                        exists: true,
                        pr: Some(pr(42, PrState::Merged)),
                        merged_at: None,
                    },
                )]),
            },
        );
        let mut inputs = vec![EnrichInput {
            worktree_id: "wt1",
            tip_sha: "sha1",
            branch: Some("feature"),
            owner: "acme",
            repo: "widgets",
        }];
        let tmp = tempfile::tempdir().unwrap();
        observe_all(&responder, tmp.path(), 0, &inputs, 1000, 3600, 20, 4);
        let calls_after_first = *responder.calls.lock().unwrap();
        inputs[0].tip_sha = "sha2";
        observe_all(&responder, tmp.path(), 0, &inputs, 1050, 3600, 20, 4);
        assert!(*responder.calls.lock().unwrap() > calls_after_first);
    }

    #[test]
    fn graphql_query_uses_variables_not_string_interpolation_for_branch_names() {
        let branches = vec!["feature/a b\"c".to_string()];
        let args = build_graphql_args("acme", "widgets", &branches);
        let query = args.iter().find(|a| a.starts_with("query=")).unwrap();
        assert!(
            !query.contains("feature/a b\"c"),
            "branch name must never be interpolated into the query text"
        );
        assert!(args.contains(&"branch0=feature/a b\"c".to_string()));
    }
}

// ---------------------------------------------------------------------
// merge_complete: composite fact over git + GitHub facts
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriState {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeComplete {
    pub verdict: TriState,
    /// Every term that fed the verdict, always emitted together so the
    /// human sees *why* -- never just "no" or "yes" on its own.
    pub terms: Vec<String>,
}

/// `merge_complete` = Yes iff merged==Yes AND clean AND unpushed==0 AND
/// the branch tip is reachable from default; Unknown if any term is
/// Unknown; No otherwise. This is a composite *fact*, not a
/// recommendation: it is always reported alongside its terms.
///
/// "Tip reachable from default" is established by the same GitHub
/// evidence that produces `merged` (a merged PR means the tip is an
/// ancestor of default), so this composite reuses `merged` for that term
/// rather than re-deriving it from a second call.
pub fn merge_complete(
    dirty: Option<bool>,
    unpushed: Option<u32>,
    merged: &MergedStatus,
) -> MergeComplete {
    let merged_term = match merged {
        MergedStatus::Yes { .. } => TriState::Yes,
        MergedStatus::No => TriState::No,
        MergedStatus::Unknown => TriState::Unknown,
    };
    let clean_term = match dirty {
        Some(false) => TriState::Yes,
        Some(true) => TriState::No,
        None => TriState::Unknown,
    };
    let unpushed_term = match unpushed {
        Some(0) => TriState::Yes,
        Some(_) => TriState::No,
        None => TriState::Unknown,
    };
    // Reuses `merged_term` as established above (see doc comment).
    let tip_reachable_term = merged_term;

    let verdict = if [merged_term, clean_term, unpushed_term, tip_reachable_term]
        .contains(&TriState::Unknown)
    {
        TriState::Unknown
    } else if merged_term == TriState::Yes
        && clean_term == TriState::Yes
        && unpushed_term == TriState::Yes
        && tip_reachable_term == TriState::Yes
    {
        TriState::Yes
    } else {
        TriState::No
    };

    let term_str = |label: &str, t: TriState| {
        format!(
            "{label}={}",
            match t {
                TriState::Yes => "yes",
                TriState::No => "no",
                TriState::Unknown => "unknown",
            }
        )
    };
    // `unpushed` renders its actual commit count (`unpushed=0`,
    // `unpushed=3`), not the yes/no/unknown every other term uses --
    // "how many" is the fact a human reading this line wants, and
    // `unpushed=0` already carries whether the tri-state term was Yes.
    let unpushed_str = match unpushed {
        Some(n) => format!("unpushed={n}"),
        None => "unpushed=unknown".to_string(),
    };
    let mut terms = vec![
        term_str("merged", merged_term),
        term_str("clean", clean_term),
        unpushed_str,
        term_str("tip_reachable", tip_reachable_term),
    ];
    if let MergedStatus::Yes {
        pr_number: Some(n), ..
    } = merged
    {
        terms.push(format!("pr=#{n}"));
    }
    if clean_term == TriState::No {
        terms.push("dirty".to_string());
    }

    MergeComplete { verdict, terms }
}

#[cfg(test)]
mod merge_complete_tests {
    use super::*;

    #[test]
    fn yes_when_every_term_is_satisfied() {
        let merged = MergedStatus::Yes {
            merged_at: None,
            pr_number: Some(7),
        };
        let mc = merge_complete(Some(false), Some(0), &merged);
        assert_eq!(mc.verdict, TriState::Yes);
        assert!(mc.terms.iter().any(|t| t == "pr=#7"));
    }

    #[test]
    fn dirty_worktree_is_no_and_names_dirty() {
        let merged = MergedStatus::Yes {
            merged_at: None,
            pr_number: None,
        };
        let mc = merge_complete(Some(true), Some(0), &merged);
        assert_eq!(mc.verdict, TriState::No);
        assert!(mc.terms.iter().any(|t| t == "dirty"));
    }

    #[test]
    fn any_unknown_term_makes_the_whole_thing_unknown() {
        let merged = MergedStatus::Unknown;
        let mc = merge_complete(Some(false), Some(0), &merged);
        assert_eq!(mc.verdict, TriState::Unknown);

        let merged_yes = MergedStatus::Yes {
            merged_at: None,
            pr_number: None,
        };
        let mc2 = merge_complete(None, Some(0), &merged_yes);
        assert_eq!(mc2.verdict, TriState::Unknown);
    }

    /// The `unpushed` term renders its actual commit count
    /// (`unpushed=0`, `unpushed=3`), never the yes/no every other term
    /// uses -- "how many" is the fact a human wants here.
    #[test]
    fn unpushed_term_renders_the_count_not_yes_no() {
        let merged = MergedStatus::Yes {
            merged_at: None,
            pr_number: None,
        };
        let zero = merge_complete(Some(false), Some(0), &merged);
        assert!(zero.terms.iter().any(|t| t == "unpushed=0"));
        assert!(!zero.terms.iter().any(|t| t == "unpushed=yes"));

        let three = merge_complete(Some(false), Some(3), &merged);
        assert!(three.terms.iter().any(|t| t == "unpushed=3"));
        assert!(!three.terms.iter().any(|t| t == "unpushed=no"));

        let unknown = merge_complete(Some(false), None, &merged);
        assert!(unknown.terms.iter().any(|t| t == "unpushed=unknown"));
    }
}
