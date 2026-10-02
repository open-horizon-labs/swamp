use swamp_core::growth::VolumeMetaRow;
use swamp_core::volume_ledger::{Category, Exactness, LedgerReading, Row as LedgerRow, account};
use swamp_tui::model::{disk_gaps_rows, disk_rows};

fn ledger_row(path: impl Into<String>, category: Category, bytes: Option<u64>) -> LedgerRow {
    LedgerRow {
        path: path.into(),
        category,
        bytes,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: 1,
        method: "test fixture".into(),
        exactness: if bytes.is_some() {
            Exactness::Exact
        } else {
            Exactness::NotMeasured
        },
        note: None,
    }
}

fn make_reading(rows: &[LedgerRow]) -> LedgerReading {
    let meta = VolumeMetaRow {
        measured_at: 10,
        cycle_started_at: 1,
        cycle_complete_at: 10,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1,
        statfs_at: 10,
        container_total: Some(1_000),
        container_used: Some(900),
        container_free: Some(100),
        data_volume_used: Some(900),
    };
    LedgerReading::Measured(Box::new(account(rows, &meta)))
}

#[test]
fn disk_children_and_parent_rows_keep_unknown_separate_from_zero() {
    let mut rows = vec![
        ledger_row("/outside/known-zero", Category::Other, Some(0)),
        ledger_row("/outside/unmeasured", Category::Other, None),
        ledger_row("APFS Data", Category::System, Some(0)),
        ledger_row("APFS Preboot", Category::System, None),
    ];
    let mut expanded = ledger_row("/outside/expanded-parent", Category::Other, None);
    expanded.method = swamp_core::volume_ledger::METHOD_EXPANDED.into();
    rows.push(expanded);
    let mut reading = make_reading(&rows);
    let account = match &mut reading {
        LedgerReading::Measured(account) => account,
        _ => unreachable!(),
    };
    account.everything_else.bytes = 0;
    account.system_volumes.bytes = 0;

    let rows = disk_rows(&reading, None);
    assert!(
        !rows
            .iter()
            .any(|row| row.label == "/outside/expanded-parent")
    );
    let find = |label: &str| rows.iter().find(|row| row.label == label).unwrap();
    assert_eq!(find("/outside/known-zero").bytes, 0);
    assert_eq!(find("/outside/known-zero").size_text, None);
    assert_eq!(
        find("/outside/unmeasured").size_text.as_deref(),
        Some("not measured")
    );
    assert_eq!(
        find("Everything else · 2 folders").size_text.as_deref(),
        Some("≥ 0B")
    );
    assert_eq!(find("System volumes").size_text.as_deref(), Some("≥ 0B"));
    assert_eq!(find("APFS Data").size_text, None);
    assert_eq!(
        find("APFS Preboot").size_text.as_deref(),
        Some("not measured")
    );
}

#[test]
fn disk_hidden_rollup_labels_partial_and_all_unknown_sizes() {
    let mut rows: Vec<_> = (0..30)
        .map(|i| {
            ledger_row(
                format!("/outside/measured-{i:02}"),
                Category::Other,
                Some(100),
            )
        })
        .collect();
    rows.push(ledger_row("/outside/known-zero", Category::Other, Some(0)));
    rows.push(ledger_row("/outside/unknown-a", Category::Other, None));
    rows.push(ledger_row("/outside/unknown-b", Category::Other, None));
    let reading = make_reading(&rows);
    let disk = disk_rows(&reading, None);
    let hidden = disk
        .iter()
        .find(|row| row.label == "and 3 more folders")
        .unwrap();
    assert_eq!(hidden.bytes, 0);
    assert_eq!(hidden.size_text.as_deref(), Some("≥ 0B"));

    let mut rows: Vec<_> = (0..30)
        .map(|i| {
            ledger_row(
                format!("/outside/measured-{i:02}"),
                Category::Other,
                Some(100),
            )
        })
        .collect();
    rows.push(ledger_row("/outside/unknown-a", Category::Other, None));
    rows.push(ledger_row("/outside/unknown-b", Category::Other, None));
    let reading = make_reading(&rows);
    let disk = disk_rows(&reading, None);
    let hidden = disk
        .iter()
        .find(|row| row.label == "and 2 more folders")
        .unwrap();
    assert_eq!(hidden.size_text.as_deref(), Some("not measured"));
}

#[test]
fn disk_gaps_top_entry_unknown_is_not_actionable_or_zero_sized() {
    let mut reading = make_reading(&[ledger_row("/outside/unmeasured-top", Category::Other, None)]);
    if let LedgerReading::Measured(account) = &mut reading {
        account.everything_else.top =
            vec![ledger_row("/outside/unmeasured-top", Category::Other, None)];
    }
    let rows = disk_gaps_rows(&reading);
    let top = rows
        .iter()
        .find(|row| row.label == "/outside/unmeasured-top")
        .unwrap();
    assert_eq!(top.bytes, 0);
    assert_eq!(top.size_text.as_deref(), Some("not measured"));
    assert_eq!(top.unit, None);
}
