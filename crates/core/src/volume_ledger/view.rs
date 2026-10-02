//! The reading of the ledger, as text and as JSON. Pure over an
//! [`Account`]: nothing here lists, stats or spawns.

use super::{Account, Exactness, Row, age_text};
use crate::render::{human_bytes_pub as human, human_count};
use serde_json::json;

fn signed(bytes: i64) -> String {
    if bytes < 0 {
        format!("-{}", human(bytes.unsigned_abs()))
    } else {
        format!("+{}", human(bytes as u64))
    }
}

fn exact_label(e: Exactness) -> &'static str {
    match e {
        Exactness::Exact => "exact",
        Exactness::Estimated => "estimated",
        Exactness::NotMeasured => "not measured",
    }
}

fn row_line(r: &Row, now: u64) -> String {
    let size = r.bytes.map(human).unwrap_or_else(|| "not measured".into());
    let mut line = format!(
        "  {size:>10}  {}  ({}, measured {})",
        r.path,
        exact_label(r.exactness),
        age_text(r.measured_at, now)
    );
    if let Some(n) = &r.note {
        line.push_str(&format!(" [{n}]"));
    }
    line
}

/// The `swamp report --view disk` text.
pub fn render_text(a: &Account, now: u64) -> String {
    let mut out = String::new();
    let age = age_text(a.measured_at, now);
    match (a.container.total, a.container.used, a.container.free) {
        (Some(t), Some(u), Some(f)) => out.push_str(&format!(
            "Disk: {} total, {} used, {} free (one APFS container; every volume in it shares the free space)\n",
            human(t),
            human(u),
            human(f)
        )),
        _ => out.push_str("Disk: container size not available\n"),
    }
    out.push_str(&format!(
        "Volume ledger measured {age}{}\n",
        if a.complete {
            String::new()
        } else {
            " (the pass has not finished; it continues at the next observe)".to_string()
        }
    ));
    out.push('\n');
    out.push_str(&format!(
        "Accounted (catalog and declared locations, counted once): {} across {} locations\n",
        human(a.accounted.bytes),
        human_count(a.accounted.locations as u64)
    ));
    out.push_str(&format!(
        "Everything else (measured, {} folders): {}\n",
        human_count(a.everything_else.folders as u64),
        human(a.everything_else.bytes)
    ));
    for r in &a.everything_else.top {
        out.push_str(&row_line(r, now));
        out.push('\n');
    }
    out.push_str(&format!(
        "System volumes: {} (separate volumes that share the container's free space)\n",
        human(a.system_volumes.bytes)
    ));
    for r in &a.system_volumes.volumes {
        out.push_str(&row_line(r, now));
        out.push('\n');
    }
    match &a.purgeable {
        Some(r) => match r.bytes {
            Some(b) => out.push_str(&format!(
                "Purgeable: {} (not added: it is inside the folders above)\n",
                human(b)
            )),
            None => out.push_str(&format!(
                "Purgeable: not measured [{}]\n",
                r.note.as_deref().unwrap_or("no answer")
            )),
        },
        None => out.push_str("Purgeable: not measured\n"),
    }
    match &a.snapshots {
        Some(r) => match (r.bytes, r.entries) {
            (Some(0), _) => out.push_str("Local snapshots: none listed\n"),
            (_, n) => out.push_str(&format!(
                "Local snapshots: {} listed, size not measured [{}]\n",
                n.map(human_count).unwrap_or_else(|| "?".into()),
                r.note.as_deref().unwrap_or("")
            )),
        },
        None => out.push_str("Local snapshots: not measured\n"),
    }
    out.push_str(&format!(
        "Not measured: {} folders could not be read\n",
        human_count(a.not_measured.count as u64)
    ));
    for name in a.not_measured.names.iter().take(20) {
        out.push_str(&format!("  {name}\n"));
    }
    if a.not_measured.count > 20 {
        out.push_str(&format!(
            "  ... and {} more (see --json for up to {} names)\n",
            human_count((a.not_measured.count - 20) as u64),
            human_count(super::MAX_NAMED_ROWS as u64)
        ));
    }
    if a.not_measured.not_yet_measured > 0 {
        out.push_str(&format!(
            "Not measured yet this pass: {} locations (the pass continues at the next observe)\n",
            human_count(a.not_measured.not_yet_measured as u64)
        ));
        for name in a.not_measured.not_yet_measured_names.iter().take(10) {
            out.push_str(&format!("  {name}\n"));
        }
    }
    if let Some(est) = a.not_measured.estimate_bytes {
        out.push_str(&format!(
            "Protected folders: not measured ({} folders); the unexplained part of the Data volume, up to {}, may be inside them (an estimate, not part of any check)\n",
            human_count(a.not_measured.count as u64),
            human(est)
        ));
    }
    match (a.residual.bytes, a.residual.percent_of_used) {
        (Some(b), Some(p)) => {
            out.push_str(&format!(
                "{} (bookkeeping): {} ({p:+.1}% of used)\n",
                capitalize(a.residual.name),
                signed(b)
            ));
            if a.residual.residual_flag {
                out.push_str(
                    "FLAG: the parts do not add up to the container's used bytes (the bookkeeping is outside the 1% tolerance); a measurement above may be missing or wrong\n",
                );
            }
        }
        _ => out.push_str(&format!("{}: not computed\n", capitalize(a.residual.name))),
    }
    if a.audit.folders.is_empty() {
        out.push_str(&format!(
            "Walk spot audit: not run ({})\n",
            a.audit.skipped.as_deref().unwrap_or("no reason recorded")
        ));
    } else {
        out.push_str(&format!(
            "Walk spot-audited: {} folders, max difference {:.1}%\n",
            human_count(a.audit.folders.len() as u64),
            a.audit.max_difference_percent
        ));
        for f in &a.audit.folders {
            if f.outside_tolerance {
                out.push_str(&format!(
                    "FLAG: walk spot audit disagrees on {}: ledger {} vs audit {} ({:+.1}%)\n",
                    f.path,
                    human(f.ledger_bytes),
                    human(f.audited_bytes),
                    f.percent
                ));
            } else {
                out.push_str(&format!(
                    "  audited {}: ledger {} vs audit {} ({:+.1}%)\n",
                    f.path,
                    human(f.ledger_bytes),
                    human(f.audited_bytes),
                    f.percent
                ));
            }
        }
    }
    if !a.external_volumes.is_empty() {
        out.push_str("On other volumes (not part of this container, not added):\n");
        for r in &a.external_volumes {
            out.push_str(&row_line(r, now));
            out.push('\n');
        }
    }
    if !a.mounted_views.is_empty() {
        out.push_str("Mounted disk images (not added: their bytes are the image files counted where they are stored):\n");
        for r in &a.mounted_views {
            out.push_str(&row_line(r, now));
            out.push('\n');
        }
    }
    for n in &a.notes {
        out.push_str(&format!("Note: {n}\n"));
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn row_json(r: &Row, now: u64) -> serde_json::Value {
    json!({
        "path": r.path,
        "category": r.category.as_str(),
        "allocated_bytes": r.bytes,
        "overlap_bytes": r.overlap_bytes,
        "entries": r.entries,
        "unreadable": r.unreadable,
        "measured_at": r.measured_at,
        "age_secs": now.saturating_sub(r.measured_at),
        "method": r.method,
        "exactness": r.exactness.as_str(),
        "note": r.note,
    })
}

/// The `disk` object of `report --json` and the result of `--view disk
/// --json`.
pub fn to_json(a: &Account, now: u64, limit: Option<usize>) -> serde_json::Value {
    let mut listed: Vec<&Row> = a
        .rows
        .iter()
        .filter(|r| r.method != super::METHOD_EXPANDED)
        .collect();
    let rows_total = listed.len();
    if let Some(n) = limit {
        // The largest rows first; the totals above cover every row.
        listed.sort_by(|x, y| y.bytes.cmp(&x.bytes).then(x.path.cmp(&y.path)));
        listed.truncate(n);
    }
    let rows_truncated = listed.len() < rows_total;
    let rows: Vec<serde_json::Value> = listed.iter().map(|r| row_json(r, now)).collect();
    json!({
        "measured_at": a.measured_at,
        "age_secs": now.saturating_sub(a.measured_at),
        "cycle_complete_at": a.cycle_complete_at,
        "complete": a.complete,
        "budget_secs": a.budget_secs,
        "budget_used_ms": a.budget_used_ms,
        "container": {
            "total": a.container.total,
            "used": a.container.used,
            "free": a.container.free,
            "data_volume_used": a.container.data_volume_used,
            "statfs_at": a.statfs_at,
        },
        "accounted": { "bytes": a.accounted.bytes, "locations": a.accounted.locations },
        "everything_else": {
            "bytes": a.everything_else.bytes,
            "folders": a.everything_else.folders,
            "top": a.everything_else.top.iter().map(|r| row_json(r, now)).collect::<Vec<_>>(),
        },
        "system_volumes": {
            "bytes": a.system_volumes.bytes,
            "volumes": a.system_volumes.volumes.iter().map(|r| row_json(r, now)).collect::<Vec<_>>(),
        },
        "purgeable": a.purgeable.as_ref().map(|r| row_json(r, now)),
        "snapshots": a.snapshots.as_ref().map(|r| row_json(r, now)),
        "not_measured": {
            "count": a.not_measured.count,
            "names": a.not_measured.names,
            "not_yet_measured": a.not_measured.not_yet_measured,
            "not_yet_measured_names": a.not_measured.not_yet_measured_names,
            "estimate_bytes": a.not_measured.estimate_bytes,
            "estimate_name": a.not_measured.estimate_name,
        },
        "residual": {
            "name": a.residual.name,
            "bytes": a.residual.bytes,
            "percent_of_used": a.residual.percent_of_used,
            "bookkeeping_balanced": a.residual.bookkeeping_balanced,
            "residual_flag": a.residual.residual_flag,
            "unexplained_bytes": a.residual.unexplained_bytes,
            "within_one_percent": a.residual.within_one_percent,
        },
        "audit": {
            "folders": a.audit.folders,
            "skipped": a.audit.skipped,
            "max_difference_percent": a.audit.max_difference_percent,
            "audit_flag": a.audit.audit_flag,
        },
        "external_volumes": a.external_volumes.iter().map(|r| row_json(r, now)).collect::<Vec<_>>(),
        "mounted_views": a.mounted_views.iter().map(|r| row_json(r, now)).collect::<Vec<_>>(),
        "notes": a.notes,
        "rows": rows,
        "rows_total": rows_total,
        "rows_truncated": rows_truncated,
    })
}

/// What `report --view disk` says when no pass has run.
pub const NOT_MEASURED_YET: &str = "Disk ledger: not measured yet; run `swamp observe --volume`\n";

/// The text of `report --view disk`, with or without a ledger.
pub fn render_disk_view(a: Option<&Account>, now: u64) -> String {
    match a {
        Some(a) => render_text(a, now),
        None => NOT_MEASURED_YET.to_string(),
    }
}

/// Rows `report --json` carries by default; `--all` carries every row.
pub const DEFAULT_JSON_ROWS: usize = 50;

/// The JSON of `report --view disk` and the `disk` object of
/// `report --json`: the reading (`limit` bounds the `rows` list, never
/// the totals), or `{"measured": false, ...}`.
pub fn disk_json(a: Option<&Account>, now: u64, limit: Option<usize>) -> serde_json::Value {
    match a {
        Some(a) => {
            let mut v = to_json(a, now, limit);
            v["measured"] = json!(true);
            v
        }
        None => json!({
            "measured": false,
            "note": "not measured yet; run `swamp observe --volume`",
        }),
    }
}
