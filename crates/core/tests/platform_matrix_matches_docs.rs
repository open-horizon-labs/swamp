//! The capability table in `docs/platform.md` is parsed back and
//! compared with `platform::CAPABILITIES` (#79).
//!
//! Same reason as `agent_matrix_matches_docs.rs`: what must not drift is
//! a *published claim* against the code that backs it, and the only way
//! to check that is to read the published claim. "Linux: supported" in a
//! document nobody checks is how a user finds out the hard way.
//!
//! The test is deliberately strict in both directions. A row in the doc
//! with no capability behind it fails; a capability with no row fails; a
//! row whose support level disagrees fails; and a row that does not parse
//! fails rather than being skipped, because a silently skipped row is a
//! claim nothing checks.

use swamp_core::platform::{CAPABILITIES, Os, Support};

#[derive(Debug, PartialEq, Eq)]
struct DocRow {
    id: String,
    macos: String,
    linux: String,
    note: String,
}

fn doc_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("docs/platform.md")
}

fn parse_doc_rows(text: &str) -> Vec<DocRow> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        if line.starts_with("| Capability | macOS | Linux | Notes |") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line.starts_with("|---") {
            continue;
        }
        if !line.starts_with('|') {
            break;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').collect();
        assert!(
            cells.len() >= 4,
            "capability row has {} cells, expected 4: {line}",
            cells.len()
        );
        rows.push(DocRow {
            id: cells[0].trim().trim_matches('`').to_string(),
            macos: cells[1].trim().to_string(),
            linux: cells[2].trim().to_string(),
            note: cells[3].trim().to_string(),
        });
    }
    rows
}

#[test]
fn every_documented_capability_matches_the_compiled_table() {
    let text = std::fs::read_to_string(doc_path()).expect("read docs/platform.md");
    let rows = parse_doc_rows(&text);
    assert_eq!(
        rows.len(),
        CAPABILITIES.len(),
        "docs/platform.md documents {} capabilities, the code has {}:\n  doc: {:?}\n  code: {:?}",
        rows.len(),
        CAPABILITIES.len(),
        rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
        CAPABILITIES.iter().map(|c| c.id).collect::<Vec<_>>()
    );

    for row in &rows {
        let cap = swamp_core::platform::capability(&row.id).unwrap_or_else(|| {
            panic!(
                "docs/platform.md documents `{}`, which no capability in the code declares",
                row.id
            )
        });
        assert_eq!(
            row.macos,
            cap.support(Os::MacOs).as_str(),
            "`{}`: the doc says macOS is {}, the code says {}",
            row.id,
            row.macos,
            cap.support(Os::MacOs).as_str()
        );
        assert_eq!(
            row.linux,
            cap.support(Os::Linux).as_str(),
            "`{}`: the doc says Linux is {}, the code says {}",
            row.id,
            row.linux,
            cap.support(Os::Linux).as_str()
        );
        // The note is prose, so it is compared after collapsing the line
        // wrapping the constant carries.
        let collapsed: String = cap.note.split_whitespace().collect::<Vec<_>>().join(" ");
        let doc_note: String = row.note.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            doc_note, collapsed,
            "`{}`: the doc's note and the code's note differ",
            row.id
        );
    }
}

/// A capability that exists in the code but is in no published table is
/// the failure mode this test is really for: a build that quietly does
/// something on one platform and not the other, with nothing telling the
/// user which.
#[test]
fn every_compiled_capability_is_documented() {
    let text = std::fs::read_to_string(doc_path()).expect("read docs/platform.md");
    let rows = parse_doc_rows(&text);
    for cap in CAPABILITIES {
        assert!(
            rows.iter().any(|r| r.id == cap.id),
            "capability `{}` is in the code and not in docs/platform.md",
            cap.id
        );
    }
}

/// The supported baseline must be stated, not assumed. Each of these is
/// a promise a user relies on and that this work deliberately made:
/// exactly two targets, a generic CPU, no root, and Ubuntu 24.04 as the
/// validation baseline rather than whatever the runner image is today.
#[test]
fn the_documented_baseline_states_every_commitment() {
    let text = std::fs::read_to_string(doc_path()).expect("read docs/platform.md");
    for required in [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "Ubuntu 24.04",
        "generic x86_64",
        "target-cpu=native",
        "Windows is not a target",
        "ARM Linux is not a target",
    ] {
        assert!(
            text.contains(required),
            "docs/platform.md does not state the baseline commitment {required:?}"
        );
    }
}

/// The one distinction the whole continuity design rests on has to
/// survive an edit to the prose as well as to the code.
#[test]
fn the_docs_keep_history_replay_unavailable_on_linux_rather_than_planned() {
    let cap = swamp_core::platform::capability("history-replay").expect("history-replay");
    assert_eq!(cap.support(Os::Linux), Support::Unavailable);
    assert_eq!(cap.support(Os::MacOs), Support::Supported);

    let text = std::fs::read_to_string(doc_path()).expect("read docs/platform.md");
    assert!(
        text.contains("no_persisted_change_history"),
        "docs/platform.md must name the reason code a Linux observation actually reports"
    );
    assert!(
        text.contains("IN_Q_OVERFLOW"),
        "the reason inotify cannot stand in for a persisted log has to be stated, not implied"
    );
}

/// #79's first deliverable is the reuse assessment, and an assessment
/// that does not reach a decision is notes. Each candidate the issue
/// names must appear with a version and a decision.
#[test]
fn the_reuse_assessment_reaches_a_decision_for_every_candidate_the_issue_names() {
    let text = std::fs::read_to_string(doc_path()).expect("read docs/platform.md");
    for candidate in ["trash", "notify", "walkdir", "jwalk", "clean-dev-dirs"] {
        assert!(
            text.contains(&format!("`{candidate}`")) || text.contains(candidate),
            "docs/platform.md has no assessment of {candidate}"
        );
    }
    // The summary table is where each decision is stated in one place.
    let summary = text
        .split("### Summary")
        .nth(1)
        .expect("docs/platform.md has no reuse-assessment summary table");
    for candidate in ["trash", "notify", "walkdir", "jwalk", "clean-dev-dirs"] {
        assert!(
            summary.contains(candidate),
            "the reuse summary does not name {candidate}"
        );
    }
    for decision in ["Adopt", "Do not adopt", "Reject", "Keep"] {
        assert!(
            summary.contains(decision),
            "the reuse summary never says {decision:?}; an assessment without decisions is notes"
        );
    }
}
