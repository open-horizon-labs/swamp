//! v0.8.0 G4b through the built binary: the developer-storage headline in
//! `swamp report`, its JSON, and above the Reclaim view. Every test uses a
//! temp store and a temp `HOME`; none reads the developer's real store or
//! disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use swamp_core::growth::{VolumeLedgerRow, VolumeMetaRow, write_volume_ledger};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn swamp(store: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run swamp")
}

fn git_project(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .status()
            .unwrap();
        assert!(status.success());
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), vec![b'x'; 200_000]).unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-q", "-m", "initial"]);
}

/// A store with one observation of one declared root, no detectors.
fn observed() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("work");
    git_project(&root.join("proj"));
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        store.path().join("config.toml"),
        format!(
            "[scan]\ndefaults = false\ninclude = [{:?}]\n",
            root.display().to_string()
        ),
    )
    .unwrap();
    let out = swamp(store.path(), home.path(), &["observe", "--no-enrich"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (store, home)
}

fn write_ledger(store: &Path, used: u64, measured_ago: u64) {
    let now = swamp_core::entities::now();
    let rows: Vec<VolumeLedgerRow> = vec![VolumeLedgerRow {
        path: "/Users/x/Movies".into(),
        category: "other".into(),
        allocated_bytes: Some(9_000_000_000),
        overlap_bytes: 0,
        entry_count: None,
        unreadable_count: 0,
        measured_at: now - measured_ago,
        method: "walk: allocated bytes, lstat only".into(),
        exactness: "exact".into(),
        note: None,
    }];
    let meta = VolumeMetaRow {
        measured_at: now - measured_ago,
        cycle_started_at: 1,
        cycle_complete_at: now - measured_ago,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1000,
        statfs_at: now - measured_ago,
        container_total: Some(used * 4),
        container_used: Some(used),
        container_free: Some(used * 3),
        data_volume_used: Some(used - 1000),
    };
    // A second write must name the meta row it replaces.
    let previous = swamp_core::growth::read_volume_ledger(store)
        .unwrap()
        .1
        .map(|m| m.measured_at);
    write_volume_ledger(store, &rows, &meta, previous).unwrap();
}

fn json(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("json")
}

/// Tempting wrong patch: the headline prints only when a ledger exists (or
/// after the old overview), or its JSON is computed apart from the text.
/// Without a ledger it is the first line, with no percent and the command
/// to run; the JSON says the same numbers.
#[test]
fn the_report_starts_with_the_headline_and_says_what_to_run_without_a_ledger() {
    let (store, home) = observed();
    let out = swamp(store.path(), home.path(), &["report"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let first = text.lines().next().unwrap();
    assert!(first.starts_with("Developer storage: "), "{text}");
    assert!(first.ends_with(" across 1 project"), "{first}");
    assert!(!first.contains('%'), "{first}");
    assert!(
        text.contains("disk ledger: not measured yet; run swamp observe --volume"),
        "{text}"
    );
    // The declared roots block stays visible, after the headline.
    let dr = text
        .find("declared source roots:")
        .expect("declared roots block");
    assert!(dr > 0);
    // JSON: the same figures.
    let j = json(&swamp(store.path(), home.path(), &["report", "--json"]));
    let h = &j["headline"];
    assert_eq!(h["line"].as_str().unwrap(), first);
    let walked = j["reconciliation"]["walked_total"].as_u64().unwrap();
    assert!(walked > 100_000, "{walked}");
    assert_eq!(h["developer_bytes"].as_u64().unwrap(), walked);
    assert!(h["percent_of_used"].is_null());
    assert_eq!(h["disk"]["state"], "not_measured");
    let sum: u64 = h["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(sum, walked);
}

/// Tempting wrong patch: the percent is computed at print time from a
/// float, or against the Data volume. With a ledger it is the integer
/// floor of developer / container used, and the ages of both sources show.
#[test]
fn a_ledger_gives_the_percent_of_the_containers_used_bytes_and_both_ages() {
    let (store, home) = observed();
    let j0 = json(&swamp(store.path(), home.path(), &["report", "--json"]));
    let dev = j0["headline"]["developer_bytes"].as_u64().unwrap();
    // Choose used so the percent is a known non-round figure.
    let used = dev * 7;
    write_ledger(store.path(), used, 3 * 3600);
    let out = swamp(store.path(), home.path(), &["report"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let first = text.lines().next().unwrap();
    let tenths = dev as u128 * 1000 / used as u128; // 142 -> 14.2
    assert!(
        first.ends_with(&format!("({}.{}% of used)", tenths / 10, tenths % 10)),
        "{first}"
    );
    assert!(text.contains("disk ledger measured 3 h ago"), "{text}");
    assert!(
        text.contains("Everything else (measured, not developer storage)"),
        "{text}"
    );
    let j = json(&swamp(store.path(), home.path(), &["report", "--json"]));
    assert_eq!(
        j["headline"]["disk"]["percent_of_used_tenths"]
            .as_u64()
            .unwrap() as u128,
        tenths
    );
    assert_eq!(j["headline"]["line"].as_str().unwrap(), first);
    // A used figure smaller than developer storage is a flag, not a percent.
    write_ledger(store.path(), dev / 2, 60);
    let text =
        String::from_utf8_lossy(&swamp(store.path(), home.path(), &["report"]).stdout).to_string();
    assert!(!text.lines().next().unwrap().contains('%'), "{text}");
    assert!(
        text.contains("FLAG: developer storage (")
            && text.contains(") is more than the disk's used bytes"),
        "{text}"
    );
}

/// Tempting wrong patch: an explicit root prints the machine-wide percent
/// for one folder.
#[test]
fn a_root_named_on_the_command_line_is_not_the_machine() {
    let (store, home) = observed();
    write_ledger(store.path(), 1_000_000_000, 60);
    let root = home.path().join("work");
    // The explicit root has its own observation.
    let out = swamp(
        store.path(),
        home.path(),
        &["observe", "--no-enrich", root.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = swamp(
        store.path(),
        home.path(),
        &["report", root.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let first = text.lines().next().unwrap();
    assert!(
        first.contains("only the root named on the command line"),
        "{first}"
    );
    assert!(!first.contains("% of used"), "{first}");
}

/// The Reclaim view starts with the same headline, and says how its totals
/// add up to it.
#[test]
fn the_reclaim_view_starts_with_the_headline_and_its_relation_holds() {
    let (store, home) = observed();
    write_ledger(store.path(), 10_000_000_000, 60);
    let out = swamp(store.path(), home.path(), &["report", "--view", "reclaim"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.starts_with("Developer storage: "), "{text}");
    assert!(
        text.contains("Developer storage ") && text.contains(" = projects "),
        "{text}"
    );
    let j = json(&swamp(
        store.path(),
        home.path(),
        &["report", "--view", "reclaim", "--json"],
    ));
    assert_eq!(
        j["headline_relation"]["holds"], true,
        "{}",
        j["headline_relation"]
    );
    assert_eq!(
        j["headline"]["developer_bytes"],
        j["headline_relation"]["developer_from_parts"]
    );
}

/// Tempting wrong patch: reading the report asks the disk something.
/// A report with a headline writes nothing to the store.
#[test]
fn reading_the_headline_writes_nothing() {
    let (store, home) = observed();
    write_ledger(store.path(), 10_000_000_000, 60);
    let list = |p: &Path| -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = std::fs::read_dir(p)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let before = list(store.path());
    for args in [
        vec!["report"],
        vec!["report", "--json"],
        vec!["report", "--view", "reclaim"],
    ] {
        let out = swamp(store.path(), home.path(), &args);
        assert!(out.status.success());
    }
    assert_eq!(list(store.path()), before);
}
