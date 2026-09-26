//! Filter grammar shared by the CLI `--filter` flag (interactive and
//! `--json`) and (per #35) the TUI. Kept small and pure:
//! `parse` builds a `Filter`, `matches_worktree`/`matches_artifact`
//! evaluate it against a report row. No verdict vocabulary lives here;
//! predicates read facts (`merge-complete`, `idle`, `growth`, `kind`,
//! `project`, `pr`), they don't rename them into "safe"/"stale"/etc.
//!
//! Grammar (whitespace-separated conjunction, every predicate must match):
//!
//! ```text
//! merge-complete
//! idle > <duration>            (e.g. "idle > 48h")
//! growth > <size> in <duration>   (e.g. "growth > 10MB in 24h")
//! growth < <size> in <duration>
//! kind:<artifact-kind>
//! project:<name>              (substring, or a glob with * and ?)
//! type:<ecosystem>            (rs|rust, js|node, py|python, …)
//! size > <size>               (unit bytes; on a project row its total)
//! size < <size>
//! age > <duration>            (time since the artifact was last written)
//! pr:open|merged|closed|none
//! ```
//!
//! Sizes: `500MB`, `1.5GB` are decimal (×1000, matching the formatter);
//! `500MiB`, `1.5GiB` are binary (×1024); a bare number is bytes.

use crate::github::PrStatus;
use crate::growth::parse_duration_secs;
use crate::report::{ArtifactKind, ArtifactRow, ProjectRow, WorktreeRow};
use anyhow::{Result, anyhow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    MergeComplete,
    IdleGreaterThan(u64),
    Growth {
        greater: bool,
        bytes: u64,
        within_secs: u64,
    },
    Kind(String),
    Project(String),
    /// Ecosystem tag on the project (`type:rust`, `type:js`), see
    /// `ecosystem::ECOSYSTEMS`; the human names accepted too.
    Type(String),
    /// Bytes of the unit (an artifact, a worktree's total, a project's total).
    Size {
        greater: bool,
        bytes: u64,
    },
    /// Seconds since the artifact was last written (`mtime_max`). An
    /// artifact with no recorded mtime never passes: unknown is not old.
    AgeGreaterThan(u64),
    Pr(PrFilter),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrFilter {
    Open,
    Merged,
    Closed,
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub predicates: Vec<Predicate>,
}

/// Extra per-worktree facts a `Filter` needs that aren't already on
/// `WorktreeRow`: the composite `merge_complete` verdict-free fact, the
/// idle duration, and the raw GitHub PR status. Computed once per
/// worktree by `report.rs`/`render.rs` and passed in here rather than
/// recomputed by the filter.
pub struct WorktreeFacts<'a> {
    pub merge_complete: bool,
    pub idle_secs: Option<u64>,
    pub pr: &'a PrStatus,
}

/// `500MB` / `1.5GB` decimal (×1000, the same base the formatter prints
/// in), `500MiB` / `1.5GiB` binary (×1024), `KB`/`K` accepted, a bare
/// number is bytes. Public so the CLI's `--budget` and `--keep-size`-style
/// flags parse the one way.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', "");
    let upper = s.to_uppercase();
    const UNITS: &[(&str, u64)] = &[
        ("TIB", 1 << 40),
        ("GIB", 1 << 30),
        ("MIB", 1 << 20),
        ("KIB", 1 << 10),
        ("TB", 1_000_000_000_000),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
        ("T", 1_000_000_000_000),
        ("G", 1_000_000_000),
        ("M", 1_000_000),
        ("K", 1_000),
        ("B", 1),
    ];
    let (num_part, mult) = UNITS
        .iter()
        .find_map(|(u, m)| upper.strip_suffix(u).map(|n| (n, *m)))
        .unwrap_or((upper.as_str(), 1));
    let n: f64 = num_part.trim().parse().ok()?;
    (n >= 0.0).then_some((n * mult as f64) as u64)
}

/// `project:` values are substrings unless they carry `*` or `?`, then
/// they are globs over the whole name (case-insensitive either way).
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    if !p.contains('*') && !p.contains('?') {
        return n.contains(&p);
    }
    fn glob(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => glob(&p[1..], n) || (!n.is_empty() && glob(p, &n[1..])),
            (Some(b'?'), Some(_)) => glob(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => glob(&p[1..], &n[1..]),
            _ => false,
        }
    }
    glob(p.as_bytes(), n.as_bytes())
}

/// Parses a filter expression into a `Filter`. Tokens are whitespace
/// separated except for the two multi-token forms (`idle > X`,
/// `growth > X in Y`), which are consumed greedily from the token
/// stream. Unknown or malformed predicates produce an `Err` naming the
/// offending token.
pub fn parse(input: &str) -> Result<Filter> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let mut predicates = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if tok == "merge-complete" {
            predicates.push(Predicate::MergeComplete);
            i += 1;
        } else if tok == "idle" {
            let op = tokens
                .get(i + 1)
                .ok_or_else(|| anyhow!("filter: 'idle' needs an operator, e.g. 'idle > 48h'"))?;
            if *op != ">" {
                return Err(anyhow!("filter: 'idle' only supports '>', got {op:?}"));
            }
            let dur_tok = tokens
                .get(i + 2)
                .ok_or_else(|| anyhow!("filter: 'idle >' needs a duration, e.g. '48h'"))?;
            let secs = parse_duration_secs(dur_tok)
                .ok_or_else(|| anyhow!("filter: invalid duration {dur_tok:?}"))?;
            predicates.push(Predicate::IdleGreaterThan(secs));
            i += 3;
        } else if tok == "growth" {
            let op = tokens.get(i + 1).ok_or_else(|| {
                anyhow!("filter: 'growth' needs an operator, e.g. 'growth > 10MB in 24h'")
            })?;
            let greater = match *op {
                ">" => true,
                "<" => false,
                other => {
                    return Err(anyhow!(
                        "filter: 'growth' only supports '>'/'<', got {other:?}"
                    ));
                }
            };
            let size_tok = tokens
                .get(i + 2)
                .ok_or_else(|| anyhow!("filter: 'growth {op}' needs a size, e.g. '10MB'"))?;
            let bytes =
                parse_size(size_tok).ok_or_else(|| anyhow!("filter: invalid size {size_tok:?}"))?;
            let in_tok = tokens.get(i + 3);
            if in_tok != Some(&"in") {
                return Err(anyhow!(
                    "filter: 'growth {op} {size_tok}' needs 'in <duration>', e.g. 'in 24h'"
                ));
            }
            let dur_tok = tokens
                .get(i + 4)
                .ok_or_else(|| anyhow!("filter: 'growth {op} {size_tok} in' needs a duration"))?;
            let within_secs = parse_duration_secs(dur_tok)
                .ok_or_else(|| anyhow!("filter: invalid duration {dur_tok:?}"))?;
            predicates.push(Predicate::Growth {
                greater,
                bytes,
                within_secs,
            });
            i += 5;
        } else if tok == "size" {
            let op = tokens
                .get(i + 1)
                .ok_or_else(|| anyhow!("filter: 'size' needs an operator, e.g. 'size > 500MB'"))?;
            let greater = match *op {
                ">" => true,
                "<" => false,
                other => {
                    return Err(anyhow!(
                        "filter: 'size' only supports '>'/'<', got {other:?}"
                    ));
                }
            };
            let size_tok = tokens
                .get(i + 2)
                .ok_or_else(|| anyhow!("filter: 'size {op}' needs a size, e.g. '500MB'"))?;
            let bytes =
                parse_size(size_tok).ok_or_else(|| anyhow!("filter: invalid size {size_tok:?}"))?;
            predicates.push(Predicate::Size { greater, bytes });
            i += 3;
        } else if tok == "age" {
            let op = tokens
                .get(i + 1)
                .ok_or_else(|| anyhow!("filter: 'age' needs an operator, e.g. 'age > 30d'"))?;
            if *op != ">" {
                return Err(anyhow!("filter: 'age' only supports '>', got {op:?}"));
            }
            let dur_tok = tokens
                .get(i + 2)
                .ok_or_else(|| anyhow!("filter: 'age >' needs a duration, e.g. '30d'"))?;
            let secs = parse_duration_secs(dur_tok)
                .ok_or_else(|| anyhow!("filter: invalid duration {dur_tok:?}"))?;
            predicates.push(Predicate::AgeGreaterThan(secs));
            i += 3;
        } else if let Some(k) = tok.strip_prefix("kind:") {
            if k.is_empty() {
                return Err(anyhow!("filter: 'kind:' needs a value"));
            }
            predicates.push(Predicate::Kind(k.to_string()));
            i += 1;
        } else if let Some(t) = tok.strip_prefix("type:") {
            if t.is_empty() {
                return Err(anyhow!("filter: 'type:' needs a value, e.g. type:rust"));
            }
            predicates.push(Predicate::Type(t.to_string()));
            i += 1;
        } else if let Some(p) = tok.strip_prefix("project:") {
            if p.is_empty() {
                return Err(anyhow!("filter: 'project:' needs a value"));
            }
            predicates.push(Predicate::Project(p.to_string()));
            i += 1;
        } else if let Some(p) = tok.strip_prefix("pr:") {
            let pr = match p {
                "open" => PrFilter::Open,
                "merged" => PrFilter::Merged,
                "closed" => PrFilter::Closed,
                "none" => PrFilter::None,
                other => {
                    return Err(anyhow!(
                        "filter: 'pr:' must be one of open|merged|closed|none, got {other:?}"
                    ));
                }
            };
            predicates.push(Predicate::Pr(pr));
            i += 1;
        } else {
            return Err(anyhow!("filter: unrecognized token {tok:?}"));
        }
    }
    Ok(Filter { predicates })
}

/// `type:rs`, `type:rust`, `type:Rust` all match a Rust project.
fn project_has_type(project: &ProjectRow, want: &str) -> bool {
    project.ecosystems.iter().any(|tag| {
        tag.eq_ignore_ascii_case(want)
            || crate::ecosystem::name_for(tag).is_some_and(|n| {
                n.eq_ignore_ascii_case(want)
                    || n.split('/').any(|part| part.eq_ignore_ascii_case(want))
                    || (want.eq_ignore_ascii_case("node") && *tag == "js")
                    || (want.eq_ignore_ascii_case("javascript") && *tag == "js")
                    || (want.eq_ignore_ascii_case("dotnet") && *tag == "net")
            })
    })
}

fn kind_matches(kind: &ArtifactKind, name: &str) -> bool {
    format!("{kind:?}").eq_ignore_ascii_case(name)
}

impl Filter {
    /// True when every predicate matches. An empty filter (no
    /// predicates) matches everything.
    pub fn matches_worktree(
        &self,
        project: &ProjectRow,
        worktree: &WorktreeRow,
        facts: &WorktreeFacts,
    ) -> bool {
        self.predicates.iter().all(|p| match p {
            Predicate::MergeComplete => facts.merge_complete,
            Predicate::IdleGreaterThan(secs) => facts.idle_secs.is_some_and(|i| i > *secs),
            Predicate::Growth {
                greater,
                bytes,
                within_secs: _,
            } => worktree.artifacts.iter().any(|a| match a.growth_bytes {
                Some(g) => {
                    let g_abs = g.unsigned_abs();
                    if *greater {
                        g > 0 && g_abs > *bytes
                    } else {
                        g < 0 && g_abs > *bytes
                    }
                }
                None => false,
            }),
            Predicate::Kind(name) => worktree
                .artifacts
                .iter()
                .any(|a| kind_matches(&a.kind, name)),
            Predicate::Project(name) => name_matches(name, &project.name),
            Predicate::Type(t) => project_has_type(project, t),
            Predicate::Size { greater, bytes } => {
                let total: u64 = worktree.artifacts.iter().map(|a| a.bytes).sum();
                if *greater {
                    total > *bytes
                } else {
                    total < *bytes
                }
            }
            // Age is a fact about an artifact; a worktree passes when any
            // of its artifacts is that old.
            Predicate::AgeGreaterThan(secs) => worktree.artifacts.iter().any(|a| {
                a.mtime_max > 0 && crate::entities::now().saturating_sub(a.mtime_max) > *secs
            }),
            Predicate::Pr(want) => match (want, facts.pr) {
                (PrFilter::None, PrStatus::None) => true,
                (PrFilter::Open, PrStatus::Some(pr)) => {
                    matches!(pr.state, crate::github::PrState::Open)
                }
                (PrFilter::Merged, PrStatus::Some(pr)) => {
                    matches!(pr.state, crate::github::PrState::Merged)
                }
                (PrFilter::Closed, PrStatus::Some(pr)) => {
                    matches!(pr.state, crate::github::PrState::Closed)
                }
                _ => false,
            },
        })
    }

    /// True when every predicate that can apply to a single artifact row
    /// matches (`kind:`/`project:`/`growth`); worktree-level predicates
    /// (`merge-complete`, `idle`, `pr:`) are ignored here since an
    /// artifact row has no branch of its own -- callers filtering
    /// artifacts should pair this with `matches_worktree` on the
    /// containing worktree first.
    pub fn matches_artifact(&self, project: &ProjectRow, artifact: &ArtifactRow) -> bool {
        self.predicates.iter().all(|p| match p {
            Predicate::MergeComplete | Predicate::IdleGreaterThan(_) | Predicate::Pr(_) => true,
            Predicate::Growth {
                greater,
                bytes,
                within_secs: _,
            } => match artifact.growth_bytes {
                Some(g) => {
                    let g_abs = g.unsigned_abs();
                    if *greater {
                        g > 0 && g_abs > *bytes
                    } else {
                        g < 0 && g_abs > *bytes
                    }
                }
                None => false,
            },
            Predicate::Kind(name) => kind_matches(&artifact.kind, name),
            Predicate::Project(name) => name_matches(name, &project.name),
            Predicate::Type(t) => project_has_type(project, t),
            Predicate::Size { greater, bytes } => {
                if *greater {
                    artifact.bytes > *bytes
                } else {
                    artifact.bytes < *bytes
                }
            }
            Predicate::AgeGreaterThan(secs) => {
                artifact.mtime_max > 0
                    && crate::entities::now().saturating_sub(artifact.mtime_max) > *secs
            }
        })
    }
}

/// Whether `bytes` (a rollup: a project's or worktree's total) passes the
/// filter's `size` predicates. Other predicates are not consulted.
pub fn size_passes(filter: &Filter, bytes: u64) -> bool {
    filter.predicates.iter().all(|p| match p {
        Predicate::Size { greater, bytes: b } => {
            if *greater {
                bytes > *b
            } else {
                bytes < *b
            }
        }
        _ => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_decimal_unless_binary_is_asked_for() {
        assert_eq!(parse_size("500MB"), Some(500_000_000));
        assert_eq!(parse_size("1.5GB"), Some(1_500_000_000));
        assert_eq!(parse_size("1GiB"), Some(1 << 30));
        assert_eq!(parse_size("2KiB"), Some(2048));
        assert_eq!(parse_size("1,000"), Some(1000));
        assert_eq!(parse_size("10k"), Some(10_000));
        assert_eq!(parse_size("x"), None);
    }

    #[test]
    fn size_and_age_and_glob_predicates_parse() {
        let f = parse("size > 500MB age > 30d project:my-app*").unwrap();
        assert_eq!(
            f.predicates,
            vec![
                Predicate::Size {
                    greater: true,
                    bytes: 500_000_000
                },
                Predicate::AgeGreaterThan(30 * 86400),
                Predicate::Project("my-app*".into()),
            ]
        );
        assert!(name_matches("my-app*", "my-app-v2"));
        assert!(!name_matches("my-app*", "other-my-app"));
        assert!(name_matches("app", "my-app-v2"));
        assert!(name_matches("regex?", "REGEXX"));
        assert!(parse("size >= 1MB").is_err());
    }

    #[test]
    fn round_trips_merge_complete() {
        let f = parse("merge-complete").unwrap();
        assert_eq!(f.predicates, vec![Predicate::MergeComplete]);
    }

    #[test]
    fn round_trips_idle() {
        let f = parse("idle > 48h").unwrap();
        assert_eq!(f.predicates, vec![Predicate::IdleGreaterThan(48 * 3600)]);
    }

    #[test]
    fn round_trips_growth() {
        let f = parse("growth > 10MB in 24h").unwrap();
        assert_eq!(
            f.predicates,
            vec![Predicate::Growth {
                greater: true,
                bytes: 10_000_000,
                within_secs: 24 * 3600
            }]
        );
    }

    #[test]
    fn round_trips_conjunction() {
        let f = parse("merge-complete idle > 48h project:widgets pr:open").unwrap();
        assert_eq!(f.predicates.len(), 4);
    }

    #[test]
    fn rejects_garbage_with_a_message() {
        let err = parse("bogus-token").unwrap_err();
        assert!(err.to_string().contains("bogus-token"));

        let err = parse("idle >").unwrap_err();
        assert!(err.to_string().contains("duration"));

        let err = parse("pr:nope").unwrap_err();
        assert!(err.to_string().contains("pr:"));
    }
}
