//! Reclaim to Trash: what a Reclaim unit or one of its folders says
//! about itself when the person marks it, and the one recheck and move
//! that follow. swamp reports; the human removes. Nothing here decides
//! that a row should go: it finds the row's real path, says what swamp
//! does and does not know about it, and moves exactly that path to the
//! Trash when the person confirms.
//!
//! **What refuses** (and only this): the path is not a real entry (gone,
//! a socket or device, not absolute, a `..` in it, a folder name that is
//! not a plain name), the OS will not let the move happen (its own error
//! is the reason), the person's own `swamp protect` mark covers it, the
//! marks cannot be checked, the target changed after the review (a
//! different entry at the path, a symlink where a folder was, a path
//! that now resolves somewhere else), or swamp's ledger cannot record
//! the move first. Category, location, tool records and use by a running
//! process are never refusals: they are lines on the confirm.
//!
//! **The two-row ledger write** is the tool-removal rule: a `started`
//! row first, the move second, the final row third. A ledger that cannot
//! take the first row means nothing moves.
//!
//! Trash is the way back: the move is one rename (`fs_gate::destroy`),
//! and the final ledger row names where it went. Space is freed only
//! when the Trash is emptied.

use crate::drilldown::ChildKind;
use crate::entities::{id_for, new_id, now};
use crate::evidence::{FactKind, FactStatus, FactValue};
use crate::ledger::{ActionRecord, Ledger, LedgerFact, NO_GRANT, Verb};
use crate::locations::RegenClass;
use crate::reclaim::{ReclaimView, hold_line};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// What a Reclaim row stands for, from the stored view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimTarget {
    /// The unit's path, or the unit's path joined with the folder name.
    pub path: PathBuf,
    /// The unit's category label (`cache`, `installation`, ...).
    pub category: String,
    /// True for a folder of a unit, false for the unit itself.
    pub is_folder: bool,
    /// How many listed folders the unit has (a unit only).
    pub listed_folders: usize,
    /// Allocated bytes from the last observation; `None` for a folder
    /// that was not measured.
    pub bytes: Option<u64>,
    /// The unit's own coverage note: the bytes are a lower bound.
    pub coverage_note: Option<String>,
    pub regeneration_class: RegenClass,
    pub regeneration_words: String,
    pub regeneration_source: String,
    pub last_used_text: String,
    pub consumers: String,
    /// Package-manager statements about the unit or folder, each quoted
    /// with the manager's name.
    pub manager_lines: Vec<String>,
    pub hold: Option<String>,
}

/// A child folder's path, refusing a name that is not one plain name:
/// the store could be edited, and `../` in a stored name must never walk
/// out of the unit.
fn child_path(unit: &Path, name: &str) -> Result<PathBuf, String> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0');
    if plain {
        Ok(unit.join(name))
    } else {
        Err(format!("{name:?} is not a plain folder name"))
    }
}

/// The path a Reclaim row is keyed by: the unit's path, or the folder's.
/// `None` for a row that is not a folder of the unit (a remainder, an
/// adjustment, a not-remeasured row) or whose name is not a plain name.
pub fn row_path(unit: &str, child: Option<(ChildKind, &str)>) -> Option<PathBuf> {
    match child {
        None => Some(PathBuf::from(unit)),
        Some((ChildKind::Entry, name)) => child_path(Path::new(unit), name).ok(),
        Some(_) => None,
    }
}

/// Why a child row of a unit cannot be marked, for its detail pane.
pub fn child_not_markable(kind: ChildKind, name: &str) -> &'static str {
    match kind {
        ChildKind::Entry if child_path(Path::new("/"), name).is_err() => {
            "not a plain folder name, so there is no single folder to move"
        }
        ChildKind::Entry => "",
        ChildKind::Remainder => {
            "this row stands for the other folders and the loose files of the unit: mark the unit, or a folder listed above"
        }
        ChildKind::Adjustment => {
            "this row is a size correction (a hardlinked file counted once), not a folder"
        }
        ChildKind::NotRemeasured => {
            "this row is what the last partial measurement did not cover, not a folder"
        }
    }
}

/// Finds the row keyed by `id` (a unit's path, or a folder's path) in the
/// view. A pure read of stored facts.
pub fn find_target(view: &ReclaimView, id: &Path) -> Result<ReclaimTarget, String> {
    for r in &view.rows {
        let unit_path = Path::new(&r.path);
        let base = |path: PathBuf, is_folder: bool| ReclaimTarget {
            path,
            category: r.kind.clone(),
            is_folder,
            listed_folders: if is_folder {
                0
            } else {
                r.children
                    .iter()
                    .filter(|c| c.kind == ChildKind::Entry)
                    .count()
            },
            bytes: Some(r.bytes),
            coverage_note: r.note.clone(),
            regeneration_class: r.regeneration.class,
            regeneration_words: r.regeneration.words.clone(),
            regeneration_source: r.regeneration.source.clone(),
            last_used_text: r.last_used_text.clone(),
            consumers: r.consumers.summary.clone(),
            manager_lines: r.manager.iter().map(|q| q.line()).collect(),
            hold: r.hold.as_ref().map(hold_line),
        };
        if unit_path == id {
            return Ok(base(unit_path.to_path_buf(), false));
        }
        for c in &r.children {
            if c.kind != ChildKind::Entry {
                continue;
            }
            let Ok(path) = child_path(unit_path, &c.name) else {
                continue;
            };
            if path == id {
                let mut t = base(path, true);
                t.bytes = c.bytes.map(|b| b.max(0) as u64);
                t.last_used_text = c.last_used.fact(view.observed_at);
                t.manager_lines = c.manager.iter().map(|q| q.line()).collect();
                t.hold = c.hold.as_ref().map(hold_line);
                return Ok(t);
            }
        }
    }
    Err(format!(
        "{} is not a row of the Reclaim view from the last observation",
        id.display()
    ))
}

/// What the entry at the path was when it was reviewed: enough to tell,
/// at the move, that it is still the same entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewed {
    pub path: PathBuf,
    /// The path's parent resolved, plus its own name unresolved: where
    /// the entry really is, without following a link at the end.
    pub canonical: PathBuf,
    device: u64,
    inode: u64,
    kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
}

impl EntryKind {
    fn label(self) -> &'static str {
        match self {
            EntryKind::Directory => "a folder",
            EntryKind::File => "a file",
            EntryKind::Symlink => "a symlink",
        }
    }
}

fn identify(path: &Path) -> Result<(EntryKind, u64, u64), String> {
    let meta = crate::fs_gate::symlink_metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{} is gone", path.display()),
        _ => format!("{} cannot be read: {e}", path.display()),
    })?;
    let t = meta.file_type();
    let kind = if t.is_symlink() {
        EntryKind::Symlink
    } else if t.is_dir() {
        EntryKind::Directory
    } else if t.is_file() {
        EntryKind::File
    } else {
        return Err(format!(
            "{} is not a folder or a file (a socket, device or pipe)",
            path.display()
        ));
    };
    Ok((kind, meta.dev(), meta.ino()))
}

fn canonical_of(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "{} has a . or .. in it, so it does not name one place",
            path.display()
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no name (the root of a volume)", path.display()))?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", path.display()))?;
    let parent = crate::fs_gate::canonicalize(parent)
        .map_err(|e| format!("{} cannot be resolved: {e}", parent.display()))?;
    Ok(parent.join(name))
}

/// The person's own keep marks: `swamp protect`. Both directions (the
/// target inside a protected path, or containing one), and a mark list
/// that cannot be read refuses (`.oh/guardrails/protection-fails-closed.md`).
fn check_protect(path: &Path, store: Option<&Path>) -> Result<(), String> {
    let Some(store) = store else {
        return Err("swamp's protect marks could not be checked (no store folder), so nothing may be marked".into());
    };
    match crate::protection::load_protect(store) {
        Ok(list) => match list.conflict(path) {
            Some(why) => Err(format!(
                "protected by you ({why}); `swamp protect remove {}` takes the mark off",
                path.display()
            )),
            None => Ok(()),
        },
        Err(e) => Err(format!(
            "swamp's protect marks could not be read ({e}), so nothing may be marked"
        )),
    }
}

/// The occupancy reading of the path as a line, or `None` when nothing
/// holds it. Tri-state: an unanswered probe says so, it is never read as
/// free. A line on the confirm, never a refusal (a Trash move can be put
/// back).
fn occupancy_line(path: &Path) -> Option<String> {
    occupancy_line_of(&crate::occupancy::open_file_evidence(path))
}

fn occupancy_line_of(ev: &crate::evidence::Evidence) -> Option<String> {
    if ev.kind != FactKind::CurrentUse {
        return None;
    }
    match &ev.status {
        FactStatus::Known(FactValue::Bool(true)) => Some(format!(
            "in use right now: {}",
            ev.note
                .as_deref()
                .unwrap_or("a process has a file open in it")
        )),
        FactStatus::Known(_) => None,
        FactStatus::Unavailable { reason } => Some(format!(
            "whether a process has it open could not be checked: {reason}"
        )),
        FactStatus::Unknown { reason } => Some(format!(
            "whether a process has it open is unresolved: {reason}"
        )),
        _ => Some("whether a process has it open is not established".to_string()),
    }
}

/// Everything the marked row adds to the confirm, plain facts in the
/// order a person deciding needs them. No line is a verdict.
pub fn warnings_for(t: &ReclaimTarget, kind_note: &str, home: Option<&Path>) -> Vec<String> {
    let mut w: Vec<String> = Vec::new();
    match t.bytes {
        None => w.push(
            "size not measured: the bytes shown are not a measurement, and what Trash takes is unknown"
                .to_string(),
        ),
        Some(_) => {
            // The unit's one note joins a coverage gap and the project
            // worktrees inside it (counted under their projects): they say
            // different things on a confirm.
            for part in t
                .coverage_note
                .iter()
                .flat_map(|n| n.split("; "))
                .filter(|p| !p.is_empty())
            {
                if part.contains("counted under projects") {
                    w.push(format!(
                        "project worktrees are inside it and go to Trash too: {part}"
                    ));
                } else {
                    w.push(format!("bytes are a lower bound ({part})"));
                }
            }
        }
    }
    if !kind_note.is_empty() {
        w.push(kind_note.to_string());
    }
    match t.regeneration_class {
        RegenClass::NotRegenerable => w.push(format!(
            "cannot be regenerated: {} (cost from {})",
            t.regeneration_words, t.regeneration_source
        )),
        RegenClass::NotEstablished => w.push(format!(
            "regeneration cost not established: {} (from {})",
            t.regeneration_words, t.regeneration_source
        )),
        RegenClass::Download | RegenClass::Rebuild => w.push(format!(
            "getting it back: {} (from {})",
            t.regeneration_words, t.regeneration_source
        )),
    }
    w.push(format!("last used: {}", t.last_used_text));
    w.push(t.consumers.clone());
    if let Some(h) = &t.hold {
        w.push(h.clone());
    }
    w.extend(t.manager_lines.iter().take(3).cloned());
    if !t.is_folder && t.listed_folders > 0 {
        w.push(format!(
            "this is the whole folder, including the {} folders listed under it",
            t.listed_folders
        ));
    }
    let path = t.path.as_path();
    // A string prefix, not `Path::starts_with`: the folder is named
    // `claude-<uid>`, and a path prefix compares whole components.
    let shown = path.to_string_lossy();
    if shown.starts_with("/private/tmp/claude-") || shown.starts_with("/tmp/claude-") {
        w.push(
            "Claude session scratch: a running Claude session that uses it breaks when it goes"
                .to_string(),
        );
    }
    if let Some(home) = home {
        if path == home.join("Library/Caches") {
            w.push(
                "this is the folder every app on this Mac keeps its cache in; swamp has not checked which apps are running"
                    .to_string(),
            );
        }
        if !path.starts_with(home) {
            w.push(format!(
                "outside your home folder ({}): the system may refuse the move",
                home.display()
            ));
        }
    }
    w
}

/// The installation consequence, for a unit whose category is one. The
/// tool's own removal path is offered separately (the `Y` flow on its
/// Tools row, where the tool has one); Trash is the other way.
pub fn installation_note(category: &str) -> &'static str {
    if category == crate::external::category_label(crate::locations::StorageCategory::Installation)
    {
        "an installation: the tool that installed it will still list it and may fail until it is reinstalled; the tool's own removal command (Y on its Tools row, where it has one) keeps the tool's records consistent, Trash does not"
    } else {
        ""
    }
}

/// What review produced for one marked row.
#[derive(Debug, Clone)]
pub struct Review {
    pub reviewed: Reviewed,
    pub warnings: Vec<String>,
}

/// Reviews one Reclaim row at mark time: a real entry at a resolvable
/// path, not covered by the person's protect marks, plus the warnings
/// (including the tri-state open-file reading).
pub fn review(
    t: &ReclaimTarget,
    store: Option<&Path>,
    home: Option<&Path>,
) -> Result<Review, String> {
    let canonical = canonical_of(&t.path)?;
    let (kind, device, inode) = identify(&t.path)?;
    check_protect(&t.path, store)?;
    check_protect(&canonical, store)?;
    let mut warnings = warnings_for(t, installation_note(&t.category), home);
    if kind == EntryKind::Symlink {
        warnings.insert(
            0,
            "this is a symlink: only the link goes to Trash, never what it points to".to_string(),
        );
    }
    if kind != EntryKind::Symlink
        && let Some(line) = occupancy_line(&t.path)
    {
        warnings.insert(0, line);
    }
    Ok(Review {
        reviewed: Reviewed {
            path: t.path.clone(),
            canonical,
            device,
            inode,
            kind,
        },
        warnings,
    })
}

/// The recheck at the move: the same entry is at the same place, and the
/// person's marks still do not cover it. Anything else is `changed since
/// review` and nothing moves.
pub fn recheck(r: &Reviewed, store: Option<&Path>) -> Result<(), String> {
    let changed = |what: String| format!("changed since review: {what}; nothing moved");
    let (kind, device, inode) = identify(&r.path).map_err(changed)?;
    if kind != r.kind {
        return Err(changed(format!(
            "{} was {} and is now {}",
            r.path.display(),
            r.kind.label(),
            kind.label()
        )));
    }
    if (device, inode) != (r.device, r.inode) {
        return Err(changed(format!(
            "a different entry is now at {}",
            r.path.display()
        )));
    }
    let canonical = canonical_of(&r.path).map_err(changed)?;
    if canonical != r.canonical {
        return Err(changed(format!(
            "{} now resolves to {}, not {}",
            r.path.display(),
            canonical.display(),
            r.canonical.display()
        )));
    }
    check_protect(&r.path, store)?;
    check_protect(&canonical, store)
}

/// The facts of one marked row the ledger keeps beside the move.
#[derive(Debug, Clone)]
pub struct MoveFacts {
    pub label: String,
    pub bytes: u64,
    pub observed_at: u64,
    pub warnings: Vec<String>,
    pub category: String,
}

fn record(
    id: &str,
    r: &Reviewed,
    facts: &MoveFacts,
    outcome: &str,
    recovery: Option<PathBuf>,
    state: &str,
) -> ActionRecord {
    ActionRecord {
        id: id.to_string(),
        verb: Verb::Delete,
        entity_id: id_for(&r.path.display().to_string()),
        evidence: vec![
            LedgerFact::new("label", &facts.label),
            LedgerFact::new("bytes", facts.bytes),
            LedgerFact::new("observed_at", facts.observed_at),
            LedgerFact::new("warnings_shown", facts.warnings.join("; ")),
            LedgerFact::new("reclaim_category", &facts.category),
            LedgerFact::new("canonical", r.canonical.display()),
        ],
        grant_id: NO_GRANT.to_string(),
        actor: "human:tui".to_string(),
        outcome: outcome.to_string(),
        recovery_location: recovery,
        measured_free_space_delta: None,
        observed_path_state: Some(state.to_string()),
        recorded_at: now(),
    }
}

/// Moves one reviewed Reclaim row to the Trash: recheck, a `started`
/// ledger row, the move, the final row. Returns where it went. A started
/// row that cannot be written means nothing moved.
pub fn trash(
    r: &Reviewed,
    facts: &MoveFacts,
    store: Option<&Path>,
    ledger: &Ledger,
    trash_root: &Path,
) -> Result<PathBuf, String> {
    recheck(r, store)?;
    let id = new_id();
    if let Err(e) = ledger.append(&record(&id, r, facts, "started", None, "moving to Trash")) {
        // Only an append that did write the row (into a new ledger, the
        // unreadable one kept aside) lets the move go on.
        if !e.to_string().starts_with(crate::ledger::KEPT_ASIDE) {
            return Err(format!(
                "swamp could not write its ledger ({e}), so nothing moved"
            ));
        }
    }
    let name = r
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("item");
    let moved =
        crate::fs_gate::destroy::trash_move(&r.path, trash_root, &format!("{name}-{}", now()))
            .map_err(|e| format!("the move to Trash failed: {e}"))?;
    let dest = moved.path().to_path_buf();
    ledger
        .replace(&record(
            &id,
            r,
            facts,
            "completed",
            Some(dest.clone()),
            "trashed",
        ))
        .map_err(|e| {
            format!(
                "moved to Trash at {}, but the final ledger row could not be written ({e}); the started row stays",
                dest.display()
            )
        })?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{Evidence, EvidenceSource, FactSubtype, Reason};

    fn source() -> EvidenceSource {
        EvidenceSource::ProcessQuery {
            tool: "lsof".into(),
        }
    }

    /// Tempting wrong patch: an open-file reading that could not be taken
    /// reads as "nothing holds it" (no line), or an occupied one is a
    /// refusal instead of a line. Occupied and unanswered both speak;
    /// only a completed empty reading is silent.
    #[test]
    fn occupancy_is_tri_state_lines_never_silence_for_unknown() {
        let occupied = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(true),
            source(),
            1,
        )
        .with_note("open handle found on /x/a");
        assert!(
            occupancy_line_of(&occupied)
                .unwrap()
                .contains("in use right now: open handle found on /x/a")
        );
        let free = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(false),
            source(),
            1,
        );
        assert_eq!(occupancy_line_of(&free), None);
        let unknown = Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            source(),
            1,
            Reason::carried("lsof timed out"),
        );
        assert!(
            occupancy_line_of(&unknown)
                .unwrap()
                .contains("could not be checked")
        );
    }
}
