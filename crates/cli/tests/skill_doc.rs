//! Structural checks on the installable skill at `skills/swamp/`: every
//! path SKILL.md references actually exists, the always-loaded body
//! stays under a stated size budget (so installing this skill does not
//! silently bloat an agent's context), and every reference is its own
//! standalone file an agent can open independently -- none is only
//! reachable by first reading another reference.

use std::fs;
use std::path::{Path, PathBuf};

/// Repository root: two levels up from this crate's manifest dir
/// (`crates/cli` -> repo root).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/cli is two levels under the repo root")
        .to_path_buf()
}

/// The always-loaded skill body must stay small: this is the budget
/// SKILL.md's own reference index states ("This file is 4.6 KB").
/// Generous headroom over that so small, honest edits don't trip the
/// test, while still catching the body growing into reference-sized
/// bulk.
const SKILL_BODY_BUDGET_BYTES: usize = 8 * 1024;

/// Every `references/....md` path mentioned in `text`, in the order
/// they first appear, deduplicated.
fn referenced_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("references/") {
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '`' | '"' | '|'))
            .unwrap_or(tail.len());
        let path = tail[..end].to_string();
        if path.ends_with(".md") && !out.contains(&path) {
            out.push(path);
        }
        rest = &tail[end.max(1)..];
    }
    out
}

#[test]
fn skill_frontmatter_has_name_and_description() {
    let root = repo_root();
    let text = fs::read_to_string(root.join("skills/swamp/SKILL.md")).expect("read SKILL.md");
    let fm_end = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---"))
        .expect("SKILL.md must start with a --- frontmatter block");
    let frontmatter = &text.strip_prefix("---\n").unwrap()[..fm_end];
    assert!(
        frontmatter.lines().any(|l| l.starts_with("name:")),
        "frontmatter missing name: {frontmatter}"
    );
    assert!(
        frontmatter.lines().any(|l| l.starts_with("description:")),
        "frontmatter missing description: {frontmatter}"
    );
}

#[test]
fn skill_body_stays_under_its_stated_size_budget() {
    let root = repo_root();
    let text = fs::read_to_string(root.join("skills/swamp/SKILL.md")).expect("read SKILL.md");
    assert!(
        text.len() <= SKILL_BODY_BUDGET_BYTES,
        "SKILL.md is {} bytes, over the {} byte budget for the always-loaded body; \
         move detail into a references/*.md file instead",
        text.len(),
        SKILL_BODY_BUDGET_BYTES
    );
}

#[test]
fn every_referenced_path_in_skill_md_exists() {
    let root = repo_root();
    let text = fs::read_to_string(root.join("skills/swamp/SKILL.md")).expect("read SKILL.md");
    let refs = referenced_paths(&text);
    assert!(
        !refs.is_empty(),
        "expected SKILL.md to reference at least one references/*.md file"
    );
    for rel in &refs {
        let path = root.join("skills/swamp").join(rel);
        assert!(
            path.is_file(),
            "SKILL.md references {rel} but skills/swamp/{rel} does not exist"
        );
    }
}

/// Every file actually present under `references/` is named by
/// SKILL.md's own index -- no orphaned reference an agent would never
/// discover, and no reference that exists only to be pulled in by
/// another reference rather than being independently listed.
#[test]
fn every_reference_file_on_disk_is_listed_in_skill_md() {
    let root = repo_root();
    let text = fs::read_to_string(root.join("skills/swamp/SKILL.md")).expect("read SKILL.md");
    let listed = referenced_paths(&text);
    let dir = root.join("skills/swamp/references");
    let mut on_disk: Vec<String> = fs::read_dir(&dir)
        .expect("read skills/swamp/references")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| format!("references/{}", e.file_name().to_string_lossy()))
        .collect();
    on_disk.sort();
    for rel in &on_disk {
        assert!(
            listed.contains(rel),
            "{rel} exists on disk but is not listed in SKILL.md's reference index"
        );
    }
}

/// Each reference is readable and makes sense as a standalone document:
/// it does not open by telling the reader to go read a sibling
/// reference first. A soft, text-level check -- the real guarantee is
/// that every reference is directly reachable from SKILL.md's flat
/// index (asserted above), not nested inside another reference's body.
#[test]
fn references_are_independently_loadable_not_chained() {
    let root = repo_root();
    let dir = root.join("skills/swamp/references");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("read references dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "md") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let first_nonblank = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default();
        assert!(
            first_nonblank.starts_with('#'),
            "{path:?} should open with its own heading, not a prerequisite pointer: {first_nonblank:?}"
        );
        checked += 1;
    }
    assert!(checked >= 2, "expected multiple reference files to check");
}
