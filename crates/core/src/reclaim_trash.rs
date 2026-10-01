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
//! take the first row means nothing moves. The move itself is
//! `actions::trash_reclaim`, an execution sink.
//!
//! Trash is the way back: the move is one rename (`fs_gate::destroy`),
//! and the final ledger row names where it went. Space is freed only
//! when the Trash is emptied.

use crate::drilldown::ChildKind;
use crate::evidence::{FactKind, FactStatus, FactValue};
use crate::fs_gate::MetadataExt;
use crate::locations::RegenClass;
use crate::reclaim::{ReclaimView, hold_line};
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
    /// Facts from wherever the row came from that are not one of the
    /// fields above (what an adapter could not establish, what a ledger
    /// row measured), worded, each a line on the confirm.
    pub notes: Vec<String>,
}

impl ReclaimTarget {
    /// A real path a view shows without a Reclaim row behind it (a
    /// store interior folder, a measured folder outside developer
    /// storage): what its row knows, and plainly that the rest is not
    /// established.
    pub fn for_path(
        path: PathBuf,
        category: &str,
        bytes: Option<u64>,
        consequence: Option<&str>,
        notes: Vec<String>,
    ) -> Self {
        let (class, words) = match consequence {
            Some(c) => (crate::reclaim::class_from_consequence(c), c.to_string()),
            None => (
                RegenClass::NotEstablished,
                "nothing recorded says what getting it back costs".to_string(),
            ),
        };
        ReclaimTarget {
            path,
            category: category.to_string(),
            is_folder: false,
            listed_folders: 0,
            bytes,
            coverage_note: None,
            regeneration_class: class,
            regeneration_words: words,
            regeneration_source: "the view's own row".to_string(),
            last_used_text: "no record".to_string(),
            consumers: "who needs it: not established for this path".to_string(),
            manager_lines: Vec::new(),
            hold: None,
            notes,
        }
    }
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
            notes: Vec::new(),
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
                let mut t = base(path.clone(), true);
                t.bytes = c.bytes.map(|b| b.max(0) as u64);
                t.last_used_text = c.last_used.fact(view.observed_at);
                t.manager_lines = c.manager.iter().map(|q| q.line()).collect();
                t.hold = c.hold.as_ref().map(hold_line);
                // A model folder: what it costs to get back and what a
                // move leaves behind (bytes in a shared blobs/ folder).
                if let Some(m) = model_at(r, &path) {
                    t.regeneration_class = crate::reclaim::class_from_consequence(&m.regeneration);
                    t.regeneration_words = m.regeneration.clone();
                    t.regeneration_source = "the model's own row".to_string();
                    t.notes = m.facts.clone();
                }
                return Ok(t);
            }
        }
        // An Ollama tag's manifest: a file, listed under the store.
        for m in &r.models {
            if Path::new(&m.path) == id {
                let mut t = base(id.to_path_buf(), false);
                t.listed_folders = 0;
                t.bytes = Some(m.bytes);
                t.regeneration_class = crate::reclaim::class_from_consequence(&m.regeneration);
                t.regeneration_words = m.regeneration.clone();
                t.regeneration_source = "the model's own row".to_string();
                t.last_used_text = m.last_read.clone();
                t.notes = m.facts.clone();
                return Ok(t);
            }
        }
    }
    Err(format!(
        "{} is not a row of the Reclaim view from the last observation",
        id.display()
    ))
}

fn model_at<'a>(
    r: &'a crate::reclaim::ReclaimRow,
    path: &Path,
) -> Option<&'a crate::build_adapters::model_stores::ModelRow> {
    let shown = crate::build_adapters::model_stores::shown_path(path);
    r.models.iter().find(|m| m.path == shown)
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
    /// The open-file reading the person was shown (None: nothing held it).
    occupancy: Option<String>,
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

fn identify(path: &Path) -> Result<(EntryKind, u64, u64, u64), String> {
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
    Ok((kind, meta.dev(), meta.ino(), meta.nlink()))
}

fn canonical_of(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    // A trailing slash makes the OS follow a symlink (`link/` is the
    // target): the path must be the one its components spell.
    let spelled: PathBuf = path.components().collect();
    if spelled.as_os_str() != path.as_os_str() {
        return Err(format!(
            "{} is not written in its plain form (a trailing or doubled slash would make the OS follow a symlink to its target)",
            path.display()
        ));
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

/// What the person's own keep marks (`swamp protect`) say about a path.
enum ProtectReading {
    Clear,
    /// A mark covers the path (or sits inside it): their own decision.
    Kept(String),
    /// The marks could not be read. Unknown, never an empty list: the
    /// confirm says so, and the person's single confirm decides.
    Unread(String),
}

/// Both directions, from the one predicate (`protection::conflict`).
fn read_protect(path: &Path, store: Option<&Path>) -> ProtectReading {
    let Some(store) = store else {
        return ProtectReading::Unread("there is no store folder to read them from".into());
    };
    match crate::protection::load_protect(store) {
        Ok(list) => match list.conflict(path) {
            Some(c) => ProtectReading::Kept(format!(
                "protected by you ({c}); `swamp protect remove {}` takes the mark off",
                c.entry.display()
            )),
            None => ProtectReading::Clear,
        },
        Err(e) => ProtectReading::Unread(e.to_string()),
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
pub fn warnings_for(t: &ReclaimTarget, kind_note: &str, around: &Surroundings<'_>) -> Vec<String> {
    let home = around.home;
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
                } else if part.starts_with("bytes are a lower bound") {
                    w.push(part.to_string());
                } else {
                    w.push(format!("bytes are a lower bound ({part})"));
                }
            }
        }
    }
    if !kind_note.is_empty() {
        w.push(kind_note.to_string());
    }
    // The words often already open with the fact ("cannot be regenerated
    // (...)"): say it once.
    let said = |lead: &str, words: &str, source: &str| {
        if words.starts_with(lead) {
            format!("{words} (from {source})")
        } else {
            format!("{lead}: {words} (from {source})")
        }
    };
    match t.regeneration_class {
        RegenClass::NotRegenerable => w.push(said(
            "cannot be regenerated",
            &t.regeneration_words,
            &t.regeneration_source,
        )),
        RegenClass::NotEstablished => w.push(said(
            "regeneration cost not established",
            &t.regeneration_words,
            &t.regeneration_source,
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
    w.extend(t.notes.iter().cloned());
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
        if path == home {
            w.push(format!(
                "this is your home folder ({}): everything you keep there goes with it",
                home.display()
            ));
        } else if home.starts_with(path) {
            w.push(format!(
                "this folder contains your whole home folder ({})",
                home.display()
            ));
        } else if !path.starts_with(home) {
            w.push(format!(
                "outside your home folder ({}): the system may refuse the move",
                home.display()
            ));
        }
    }
    w.extend(inside_facts(path, around));
    w
}

/// Where the review happens: the person's home, swamp's own store and the
/// Trash the move would go to. Facts about a path are stated against these.
#[derive(Debug, Clone, Copy, Default)]
pub struct Surroundings<'a> {
    pub home: Option<&'a Path>,
    pub store: Option<&'a Path>,
    pub trash_root: Option<&'a Path>,
    /// The folder swamp was started in.
    pub cwd: Option<&'a Path>,
    /// Units and facts swamp already knows of by path: what a folder that
    /// contains them takes along.
    pub known: &'a [Known],
}

/// A path swamp knows something about, by class (`class` is plain words,
/// plural, for a line on the confirm: "Reclaim units", "AI-tool units kept
/// by default").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub path: PathBuf,
    pub class: &'static str,
}

/// Escapes control and invisible format characters (newline, bidi
/// overrides, zero-width marks) so a folder name cannot write its own
/// line into a plan or reorder what is read. Printable text is unchanged.
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let hidden = c.is_control()
            || matches!(c as u32,
                0x00AD | 0x061C | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064
                | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB);
        if hidden {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Facts about what the path CONTAINS (a folder above a known unit takes
/// it along), about what it is (the system temp folder, a whole Library, a
/// Homebrew prefix, a volume root) and about where swamp is running.
/// Bounded: each class is one line with a count and at most three names.
fn inside_facts(path: &Path, around: &Surroundings<'_>) -> Vec<String> {
    let mut w: Vec<String> = Vec::new();
    let shown = path.to_string_lossy();
    if path == Path::new("/private/tmp")
        || path == Path::new("/tmp")
        || path == Path::new("/private")
    {
        w.push(
            "this is the system temp folder (or holds it): every running program's temp files are in it, including Claude session scratch that a running session needs"
                .to_string(),
        );
    }
    if let Some(home) = around.home
        && path == home.join("Library")
    {
        w.push(
            "this is your whole Library: app data, preferences, Keychains, Mail, containers and every app's cache"
                .to_string(),
        );
    }
    if path == Path::new("/opt/homebrew") || shown.ends_with("/.linuxbrew") {
        w.push(
            "this is Homebrew's own prefix: brew itself and every tool it installed go with it"
                .to_string(),
        );
    }
    if shown.starts_with("/Volumes/") && path.components().count() == 3 {
        w.push("this is the root of a mounted volume: everything on that volume".to_string());
    }
    if let Some(cwd) = around.cwd
        && cwd.starts_with(path)
    {
        w.push(format!(
            "this contains the folder swamp was started in ({})",
            cwd.display()
        ));
    }
    let mut classes: Vec<(&'static str, Vec<String>)> = Vec::new();
    for k in around.known {
        if k.path != path && k.path.starts_with(path) {
            let name = k
                .path
                .strip_prefix(path)
                .unwrap_or(&k.path)
                .display()
                .to_string();
            match classes.iter_mut().find(|(c, _)| *c == k.class) {
                Some((_, names)) => names.push(name),
                None => classes.push((k.class, vec![name])),
            }
        }
    }
    for (class, names) in classes {
        let few: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
        let more = names.len().saturating_sub(3);
        w.push(format!(
            "this contains {class} ({}): {}{}",
            names.len(),
            few.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
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
    review_in(
        t,
        &Surroundings {
            home,
            store,
            ..Default::default()
        },
    )
}

/// Whether the Trash is on another volume than the entry. Moves are one
/// rename (`fs_gate::destroy`), never a copy, so such a move is refused by
/// the backend; the confirm says so before Enter.
pub fn trash_volume_line(entry_device: u64, trash_device: Option<u64>) -> Option<String> {
    match trash_device {
        Some(d) if d != entry_device => Some(
            "the Trash is on another volume than this folder: swamp moves by rename and never copies, so the move will be refused; free space does not help"
                .to_string(),
        ),
        _ => None,
    }
}

/// The device of the Trash root, or of the nearest folder above it that
/// exists (the Trash is created on the first move).
fn trash_device(root: &Path) -> Option<u64> {
    let mut at = Some(root);
    while let Some(p) = at {
        if let Ok(m) = crate::fs_gate::symlink_metadata(p) {
            return Some(m.dev());
        }
        at = p.parent();
    }
    None
}

/// Folders directly inside `path` (and one level further, bounded) that
/// sit on another device: mounted volumes. Pure decision in
/// [`different_device`].
fn submounts(path: &Path, device: u64) -> Vec<PathBuf> {
    let mut seen: Vec<(PathBuf, u64)> = Vec::new();
    // The capped shallow listing (the one listing outside the walker).
    for e in crate::locations::shallow_list(path)
        .entries
        .iter()
        .filter(|e| e.is_dir)
    {
        let p = path.join(&e.name);
        if let Ok(m) = crate::fs_gate::symlink_metadata(&p) {
            seen.push((p, m.dev()));
        }
    }
    different_device(device, &seen)
}

/// Which of the listed entries are on another device than `device`.
fn different_device(device: u64, entries: &[(PathBuf, u64)]) -> Vec<PathBuf> {
    entries
        .iter()
        .filter(|(_, d)| *d != device)
        .map(|(p, _)| p.clone())
        .collect()
}

/// A `.git` at the path or one folder below it (bounded listing).
fn git_checkout_line(path: &Path) -> Option<String> {
    let has_git = |p: &Path| crate::fs_gate::symlink_metadata(p.join(".git")).is_ok();
    let found = if has_git(path) {
        Some(path.to_path_buf())
    } else {
        crate::locations::shallow_list(path)
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .map(|e| path.join(&e.name))
            .find(|p| has_git(p))
    };
    found.map(|p| {
        format!(
            "git checkout at {}: commits not pushed anywhere, and uncommitted work, are lost unless saved elsewhere; check `git status` first",
            p.display()
        )
    })
}

/// [`review`] with the Trash root known, so the confirm can say when the
/// Trash is on another volume.
pub fn review_in(t: &ReclaimTarget, around: &Surroundings<'_>) -> Result<Review, String> {
    let store = around.store;
    let canonical = canonical_of(&t.path)?;
    let (kind, device, inode, nlink) = identify(&t.path)?;
    let mut unread = None;
    for p in [&t.path, &canonical] {
        match read_protect(p, store) {
            ProtectReading::Kept(why) => return Err(why),
            ProtectReading::Unread(why) => unread = Some(why),
            ProtectReading::Clear => {}
        }
    }
    // The one refusal about swamp itself: a move that takes away the
    // ledger that records it (or the Trash it moves into) cannot be
    // recorded, and the ledger would be silently recreated elsewhere.
    let own = |p: &Path| {
        // A store that does not exist holds no ledger to lose.
        if crate::fs_gate::symlink_metadata(p).is_err() {
            return false;
        }
        let c = crate::scope::comparable(p);
        let me = crate::scope::comparable(&t.path);
        c == me || c.starts_with(&me)
    };
    let resolved_store = crate::fs_gate::StoreDir::resolved();
    // The store wherever this run resolved it, and the places swamp's
    // store lives by default (a TUI started with another SWAMP_DIR still
    // has its usual store under the home folder).
    let env_home = std::env::var_os("HOME").map(PathBuf::from);
    let usual = |h: &Path, n: &str| h.join(".local/share").join(n);
    let by_default = around
        .home
        .into_iter()
        .chain(env_home.as_deref())
        .flat_map(|h| [usual(h, "swamp"), usual(h, "swamp-preview")])
        // Only as ancestors: when the run's own store is elsewhere, the
        // usual folder itself is just a folder, but what holds it is not.
        .any(|p| {
            let held = crate::scope::comparable(&t.path);
            own(&p) && crate::scope::comparable(&p) != held
        });
    if store.is_some_and(own) || own(resolved_store.path()) || by_default {
        return Err("this holds swamp's own ledger, which records this move; move it yourself in Finder if you want it gone".to_string());
    }
    if around.trash_root.is_some_and(own) {
        return Err("this holds the Trash this move goes into, and swamp's ledger records it; move it yourself in Finder if you want it gone".to_string());
    }
    let brew = t.path == Path::new("/opt/homebrew");
    let note = if brew {
        ""
    } else {
        installation_note(&t.category)
    };
    let mut warnings = warnings_for(t, note, around);
    if kind == EntryKind::Directory {
        let mounted = submounts(&t.path, device);
        if !mounted.is_empty() {
            let names: Vec<String> = mounted
                .iter()
                .take(3)
                .filter_map(|p| p.file_name().map(|n| plain(&n.to_string_lossy())))
                .collect();
            warnings.push(format!(
                "contains {} mounted volume{} ({}{}): their bytes live on the disk images, not in this folder, so moving it frees about nothing, and whatever uses them breaks; unmount them first",
                mounted.len(),
                if mounted.len() == 1 { "" } else { "s" },
                names.join(", "),
                if mounted.len() > 3 { ", ..." } else { "" }
            ));
        }
    }
    if kind == EntryKind::File && nlink > 1 {
        warnings.push(format!(
            "this file has {nlink} hard links: its bytes stay on disk while another link exists"
        ));
    }
    if kind == EntryKind::Directory
        && let Some(line) = git_checkout_line(&t.path)
    {
        warnings.push(line);
    }
    if let Some(root) = around.trash_root
        && let Some(line) = trash_volume_line(device, trash_device(root))
    {
        warnings.push(line);
    }
    if let Some(why) = unread {
        warnings.insert(
            0,
            format!(
                "could not read your protect list ({why}): your keep marks were not checked for this path"
            ),
        );
    }
    if kind == EntryKind::Symlink {
        warnings.insert(
            0,
            "this is a symlink: only the link goes to Trash, never what it points to".to_string(),
        );
    }
    let occupancy = if kind != EntryKind::Symlink {
        occupancy_line(&t.path)
    } else {
        None
    };
    if let Some(line) = &occupancy {
        warnings.insert(0, line.clone());
    }
    // Each fact once, in order (two sources can word the same coverage gap).
    let mut seen = std::collections::HashSet::new();
    warnings.retain(|w| seen.insert(w.clone()));
    Ok(Review {
        reviewed: Reviewed {
            path: t.path.clone(),
            canonical,
            device,
            inode,
            kind,
            occupancy,
        },
        warnings,
    })
}

/// What the open-file reading says that can make a confirm wrong: held
/// (and on what), or not held. An unanswered reading (the snapshot timed
/// out under load) against a free one is not a change: only a folder that
/// became held, or stopped being held, refuses, so a busy machine does not
/// turn every Enter into "review again".
fn occupancy_key(line: Option<&str>) -> String {
    match line {
        // Held or not: never which file, because a churning cache (journal
        // files) names a different first file every time it is read.
        Some(l) if l.starts_with("in use right now") => "held".to_string(),
        _ => "not held".to_string(),
    }
}

/// The recheck at the move: the same entry is at the same place, and the
/// person's marks still do not cover it. Anything else is `changed since
/// review` and nothing moves.
pub fn recheck(r: &Reviewed, store: Option<&Path>) -> Result<(), String> {
    let changed = |what: String| format!("changed since review: {what}; nothing moved");
    let (kind, device, inode, _) = identify(&r.path).map_err(changed)?;
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
    // The warnings the person read are facts: a process that opened the
    // folder since, or let go of it, is a different confirm (the rule
    // tool-managed removal keeps at `Y`). A correctness check on what was
    // shown, not a veto on the category or on use.
    if kind != EntryKind::Symlink {
        let now = occupancy_line(&r.path);
        if occupancy_key(now.as_deref()) != occupancy_key(r.occupancy.as_deref()) {
            return Err(changed(format!(
                "what holds it open changed (was: {}; now: {}); review again",
                r.occupancy.as_deref().unwrap_or("nothing held it"),
                now.as_deref().unwrap_or("nothing holds it")
            )));
        }
    }
    // A list that could not be read at review was said so on the confirm;
    // a mark that covers the entry now is the person's own decision.
    for p in [&r.path, &canonical] {
        if let ProtectReading::Kept(why) = read_protect(p, store) {
            return Err(why);
        }
    }
    Ok(())
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

    /// Tempting wrong patch: the volume check is left to the move, so the
    /// confirm never says the Trash is elsewhere (and a refusal arrives
    /// after Enter). Another device says so; the same device or a Trash
    /// not yet made says nothing.
    #[test]
    fn mounted_volumes_are_the_entries_on_another_device() {
        let e = vec![
            (PathBuf::from("/v/a"), 7),
            (PathBuf::from("/v/img1"), 9),
            (PathBuf::from("/v/img2"), 10),
        ];
        assert_eq!(
            different_device(7, &e),
            vec![PathBuf::from("/v/img1"), PathBuf::from("/v/img2")]
        );
        assert!(different_device(7, &e[..1]).is_empty());
    }

    #[test]
    fn a_trash_on_another_volume_is_said_before_enter() {
        assert!(
            trash_volume_line(1, Some(2))
                .unwrap()
                .contains("another volume")
        );
        assert_eq!(trash_volume_line(1, Some(1)), None);
        assert_eq!(trash_volume_line(1, None), None);
    }

    /// Tempting wrong patch: control characters are printed as they are.
    #[test]
    fn plain_escapes_controls_and_bidi_and_keeps_text() {
        assert_eq!(plain("a\nb\u{202e}c"), "a\\u{a}b\\u{202e}c");
        assert_eq!(plain("Caches/héllo"), "Caches/héllo");
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
