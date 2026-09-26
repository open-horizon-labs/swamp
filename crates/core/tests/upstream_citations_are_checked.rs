//! Real provenance for the support matrix, replacing the 30-character
//! length check.
//!
//! What CI used to guarantee about a `Supported` row's "Verified
//! against" cell was that the string was longer than 30 characters
//! (`agent_matrix_matches_docs.rs`) and non-empty
//! (`matrix.rs::a_supported_row_cites_what_confirmed_it`). The
//! 2026-09-22 independent re-review fetched all fourteen rows' citations
//! and found six that did not establish what the row claimed -- four of
//! them consequential in the adapter (a dead path, silently uncounted
//! bytes, a missing protected store, and parsed field names that appear
//! in no cited source) -- with the suite green throughout. A length
//! check on a provenance string is not provenance.
//!
//! So each cited upstream file is **vendored** as a small excerpt under
//! `crates/core/tests/fixtures/upstream/<tool>/<commit-prefix>/`, pinned
//! by commit, recorded with its blake3 digest, and listed in `citations.toml`
//! together with the symbols the row's claim depends on. This test
//! greps the vendored excerpt for every claimed symbol and fails naming
//! the citation that does not support its claim. Nothing is fetched at
//! test time: the check is offline, deterministic, and breaks when a
//! claim is edited without re-reading the source.
//!
//! The excerpts are a few lines each, kept for verification, with the
//! upstream repository, commit and path recorded beside them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use swamp_core::agents::matrix::{MATRIX, SupportLevel};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream")
}

#[derive(Debug)]
struct Citation {
    tool: String,
    repo: String,
    commit: String,
    path: String,
    symbols: Vec<String>,
    excerpt: String,
    blake3: String,
    /// The blake3 of the **whole upstream file** at `commit`, as fetched
    /// when the excerpt was vendored -- or `doc-page` for a citation of a
    /// docs page, which has no commit and cannot be re-fetched byte for
    /// byte.
    upstream_blake3: String,
}

/// A deliberately tiny TOML-ish reader: `[[citation]]` blocks of
/// `key = "value"` and `symbols = ["a", "b"]`. A real TOML dependency in
/// a test would be fine too; this keeps the manifest format obvious to
/// anyone editing it and the failure messages specific.
fn load_manifest(path: &Path) -> Vec<Citation> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()));
    let mut out = Vec::new();
    let mut cur: BTreeMap<String, String> = BTreeMap::new();
    let mut symbols: Vec<String> = Vec::new();
    let flush =
        |cur: &mut BTreeMap<String, String>, symbols: &mut Vec<String>, out: &mut Vec<Citation>| {
            if cur.is_empty() {
                return;
            }
            let get = |k: &str| -> String {
                cur.get(k)
                    .unwrap_or_else(|| panic!("citation block is missing `{k}`: {cur:?}"))
                    .clone()
            };
            out.push(Citation {
                tool: get("tool"),
                repo: get("repo"),
                commit: get("commit"),
                path: get("path"),
                symbols: std::mem::take(symbols),
                excerpt: get("excerpt"),
                blake3: get("blake3"),
                upstream_blake3: get("upstream_blake3"),
            });
            cur.clear();
        };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[citation]]" {
            flush(&mut cur, &mut symbols, &mut out);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim();
        if key == "symbols" {
            // Single-quoted entries, so a symbol may itself contain a
            // double quote -- which several of them must: the claim
            // being checked is often a string literal in the upstream
            // source (`workspaceDirectory: ""`, `"state", "taskHistory.json"`).
            // A double-quoted list could not express those, and dropping
            // the quotes from the symbol would weaken exactly the
            // citations that need to be strongest.
            symbols = value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split("', '")
                .map(|s| s.trim().trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }
        cur.insert(key, value.trim_matches('"').to_string());
    }
    flush(&mut cur, &mut symbols, &mut out);
    out
}

/// blake3 rather than SHA-256 only because it is already a dependency of
/// this crate: adding a hash crate to check a vendored excerpt would be
/// a dependency for a digest, and the property needed here -- an edited
/// excerpt no longer matches -- is the same.
fn digest_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Every vendored excerpt hashes to the digest recorded beside it, and
/// contains every symbol the matrix claim depends on. An excerpt edited
/// to make a claim fit fails on the hash; a claim edited past what the
/// excerpt says fails on the symbol.
#[test]
fn every_vendored_citation_contains_the_symbol_it_is_cited_for() {
    let dir = fixtures();
    let citations = load_manifest(&dir.join("citations.toml"));
    assert!(
        !citations.is_empty(),
        "{}/citations.toml lists no citations",
        dir.display()
    );
    let mut problems: Vec<String> = Vec::new();
    for c in &citations {
        let excerpt = dir.join(&c.excerpt);
        let Ok(bytes) = std::fs::read(&excerpt) else {
            problems.push(format!(
                "{}: cited file {} @ {} has no vendored excerpt at {}",
                c.tool,
                c.path,
                c.commit,
                excerpt.display()
            ));
            continue;
        };
        let got = digest_hex(&bytes);
        if got != c.blake3 {
            problems.push(format!(
                "{}: {} hashes to {got}, manifest says {}; an excerpt edited to make a claim fit \
                 is not a citation",
                c.tool,
                excerpt.display(),
                c.blake3
            ));
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !c.symbols.is_empty(),
            "{}: citation for {} lists no symbols; a citation with nothing to check is the \
             30-character check again",
            c.tool,
            c.path
        );
        for sym in &c.symbols {
            if !text.contains(sym) {
                problems.push(format!(
                    "{}: {} ({} @ {}) does not contain `{sym}`",
                    c.tool, c.path, c.repo, c.commit
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every `Supported` row's citations pin a commit or a date, and every
/// citation that names a repository file has a vendored excerpt. A row
/// whose provenance is "read during chunk #93" cannot be re-checked by
/// anyone, which is what five rows said.
#[test]
fn every_supported_row_pins_its_citation_and_vendors_the_file() {
    let dir = fixtures();
    let citations = load_manifest(&dir.join("citations.toml"));
    let mut problems: Vec<String> = Vec::new();
    for entry in MATRIX {
        if entry.support != SupportLevel::Supported {
            continue;
        }
        assert!(
            !entry.verification.is_empty(),
            "{} is Supported and cites nothing",
            entry.display_name
        );
        for v in entry.verification {
            let pinned = v.revision.split_whitespace().any(|w| {
                let w = w.trim_end_matches(&[',', ';'][..]);
                (w.len() == 40 && w.chars().all(|c| c.is_ascii_hexdigit()))
                    || (w.len() == 10
                        && w.as_bytes()[4] == b'-'
                        && w.as_bytes()[7] == b'-'
                        && w.chars().filter(|c| c.is_ascii_digit()).count() == 8)
            });
            if !pinned {
                problems.push(format!(
                    "{}: revision `{}` pins neither a commit nor a date, so the citation cannot \
                     be re-checked",
                    entry.display_name, v.revision
                ));
            }
            // A citation that names a repository file (not a docs page)
            // must be vendored so the symbol check above can run on it.
            let is_repo_file = !v.source.starts_with("http") || v.source.contains("/blob/");
            if is_repo_file {
                // Matched by the full (tool, revision, path) key, never by a
                // path suffix: re-review 3 (F3) showed two upstream files
                // sharing a suffix were interchangeable, so a row could be
                // "vendored" by another row's file.
                let (path, url_commit) = match v.source.split_once("/blob/") {
                    Some((_, rest)) => match rest.split_once('/') {
                        Some((sha, path)) => (path, Some(sha)),
                        None => (rest, None),
                    },
                    None => (v.source, None),
                };
                let commit = url_commit.map(str::to_string).or_else(|| {
                    v.revision
                        .split(|c: char| !c.is_ascii_hexdigit())
                        .find(|w| w.len() == 40)
                        .map(str::to_string)
                });
                let matched = citations.iter().any(|c| {
                    let same_revision = match &commit {
                        Some(sha) => c.commit == *sha,
                        None => c
                            .commit
                            .strip_prefix("retrieved ")
                            .is_some_and(|date| v.revision.contains(date)),
                    };
                    c.tool == entry.id.slug() && c.path == path && same_revision
                });
                if !matched {
                    problems.push(format!(
                        "{}: citation `{}` names an upstream file with no vendored excerpt in \
                         citations.toml",
                        entry.display_name, v.source
                    ));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// No vendored file without a citation, and no citation without a
/// vendored file.
///
/// The second half is covered above. This is the first: a stray excerpt
/// nobody cites is either evidence for a claim that is not in the
/// manifest -- which is the 30-character check again, in file form -- or
/// dead weight that will rot. Exactly one exception, named here rather
/// than pattern-matched loosely: a `NO-VENDOR-*` file, which records
/// line numbers and symbol strings for a source whose licence forbids
/// redistribution. Such a file backs no claim, by construction.
#[test]
fn every_vendored_file_is_cited_by_the_manifest() {
    let dir = fixtures();
    let citations = load_manifest(&dir.join("citations.toml"));
    let mut files: Vec<PathBuf> = Vec::new();
    fn walk(at: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(&dir, &mut files);
    let mut problems: Vec<String> = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&dir).unwrap_or(f).display().to_string();
        if rel == "citations.toml" {
            continue;
        }
        if f.file_name()
            .map(|n| n.to_string_lossy().starts_with("NO-VENDOR-"))
            .unwrap_or(false)
        {
            continue;
        }
        if !citations.iter().any(|c| c.excerpt == rel) {
            problems.push(format!(
                "{rel} is vendored but cited by nothing in citations.toml: either cite it, or \
                 delete it -- an uncited excerpt is evidence for a claim CI cannot see"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// ---------------------------------------------------------------------
// The excerpt against upstream (re-review 3, F3)
// ---------------------------------------------------------------------
//
// The blake3 above pins the excerpt to the manifest, not the excerpt to
// upstream: an excerpt written by hand with its digest recomputed passes
// every check in this file. What closes that is recorded at vendoring
// time: `upstream_blake3`, the digest of the whole upstream file at the
// pinned commit. Offline, every pinned citation must carry one. Online
// (`SWAMP_FETCH_UPSTREAM=1`), each file is re-fetched at its commit, must
// hash to it, and must contain every substantive line of the excerpt
// verbatim. A docs page has no commit; its excerpt is author-supplied and
// says so (`upstream_blake3 = "doc-page"`), which offline verification
// cannot close.

fn pinned(c: &Citation) -> bool {
    c.repo.starts_with("github.com/")
        && c.commit.len() == 40
        && c.commit.chars().all(|x| x.is_ascii_hexdigit())
}

#[test]
fn every_pinned_citation_records_its_fetched_upstream_digest() {
    let citations = load_manifest(&fixtures().join("citations.toml"));
    let mut problems = Vec::new();
    for c in &citations {
        let recorded = &c.upstream_blake3;
        let is_digest = recorded.len() == 64 && recorded.chars().all(|x| x.is_ascii_hexdigit());
        if pinned(c) && !is_digest {
            problems.push(format!(
                "{} {}: a pinned citation records no upstream digest ({recorded})",
                c.repo, c.path
            ));
        }
        if !pinned(c) && recorded != "doc-page" {
            problems.push(format!(
                "{} {}: an unpinned citation must say `doc-page`, not {recorded}",
                c.repo, c.path
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The lines of an excerpt that are quoted upstream text: not the
/// provenance header, not a line-range marker, not an elision.
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

#[test]
fn a_pinned_citation_re_fetches_to_its_recorded_digest_and_contains_its_excerpt() {
    let record = std::env::var("SWAMP_RECORD_UPSTREAM").is_ok();
    if std::env::var("SWAMP_FETCH_UPSTREAM").is_err() && !record {
        eprintln!("skipped: set SWAMP_FETCH_UPSTREAM=1 to re-fetch every pinned citation");
        return;
    }
    let dir = fixtures();
    let citations = load_manifest(&dir.join("citations.toml"));
    let tmp = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (i, c) in citations.iter().enumerate().filter(|(_, c)| pinned(c)) {
        let repo = c.repo.trim_start_matches("github.com/");
        let url = format!(
            "https://raw.githubusercontent.com/{repo}/{}/{}",
            c.commit, c.path
        );
        let out = tmp.path().join(format!("f{i}"));
        let ok = std::process::Command::new("curl")
            .args(["-sfL", "-m", "30", "-o"])
            .arg(&out)
            .arg(&url)
            .status()
            .is_ok_and(|s| s.success());
        let Ok(bytes) = std::fs::read(&out)
            .map_err(|_| ())
            .and_then(|b| if ok { Ok(b) } else { Err(()) })
        else {
            problems.push(format!("{url}: could not be fetched"));
            continue;
        };
        let digest = digest_hex(&bytes);
        if record {
            println!("RECORD {}|{}|{} {digest}", c.repo, c.commit, c.path);
            continue;
        }
        if digest != c.upstream_blake3 {
            problems.push(format!(
                "{url}: upstream digest {digest} is not the recorded {}",
                c.upstream_blake3
            ));
        }
        let upstream = String::from_utf8_lossy(&bytes);
        let excerpt = std::fs::read_to_string(dir.join(&c.excerpt)).unwrap_or_default();
        for line in quoted_lines(&excerpt) {
            if !upstream.contains(line.as_str()) {
                problems.push(format!(
                    "{}: excerpt line not found upstream at the pinned commit: {line}",
                    c.excerpt
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
