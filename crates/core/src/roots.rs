//! Declared source roots (#168): the directories the user says hold
//! their source code, kept in `[scan] include` of `config.toml`.
//!
//! swamp never works these out. The only sources of a root are the
//! built-in catalog and what the user declares here, so there is no code
//! in this crate that reads shell history, an editor's recent projects,
//! git configuration or a Spotlight index to propose one
//! (`.oh/guardrails/roots-are-declared-never-inferred.md`).
//!
//! * [`add_root`] / [`remove_root`] edit the config file in place: comments,
//!   ordering and every other key survive, the write is atomic, and edits
//!   are serialized under a lock so two racing callers both land.
//! * [`first_run`] asks once, on an interactive terminal, when nothing has
//!   been observed and the config has no `[scan]` section.
//! * [`declared_roots`] / [`render_declared_roots`] state each declared root
//!   as present, missing or unreadable, with bytes taken from the stored
//!   observation. Nothing here walks a directory.

use crate::coverage::{RegionStatus, RootCoverage};
use crate::fs_gate::StoreDir;
use crate::fs_gate::store::TextFile;
use crate::scope::{EffectiveScope, RootReason, RootStatus, comparable, configured_path};
use std::io::Write;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Value};

/// Where a typed path is read from: `~` is `home`, a relative path is
/// relative to `cwd` (never to home, unlike a hand-written `include`
/// entry, so what the shell would resolve is what is recorded).
#[derive(Debug, Clone)]
pub struct Reach {
    pub home: PathBuf,
    pub cwd: PathBuf,
}

/// Why a root could not be declared or removed. Every message says what
/// was wrong and what to do; none leaves the config file changed.
#[derive(Debug, PartialEq, Eq)]
pub enum RootError {
    /// Nothing exists there. `--allow-missing` records a root that is not
    /// mounted yet.
    Missing(PathBuf),
    NotADirectory(PathBuf),
    /// Exists but the current user cannot read it.
    Unreadable {
        path: PathBuf,
        reason: String,
    },
    /// Inside a root that is already declared.
    NestedUnder {
        typed: String,
        declared: String,
    },
    /// `/` would put the whole machine in scope.
    TooBroad(PathBuf),
    NotDeclared(String),
    /// The path cannot be written to a TOML file as text.
    NotUtf8(PathBuf),
    /// `config.toml` cannot be edited safely (not TOML, or `[scan]` /
    /// `include` has the wrong shape). The file is left as it is.
    Config(String),
    Io(String),
}

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootError::Missing(p) => write!(
                f,
                "{} does not exist. Check the spelling, or pass --allow-missing to record a \
                 root that is not mounted yet (it is then reported as missing, never as an error).",
                p.display()
            ),
            RootError::NotADirectory(p) => write!(
                f,
                "{} is not a directory; a source root is a directory that holds projects.",
                p.display()
            ),
            RootError::Unreadable { path, reason } => write!(
                f,
                "{} exists but this user cannot read it ({reason}); swamp only declares roots \
                 within your reach.",
                path.display()
            ),
            RootError::NestedUnder { typed, declared } => write!(
                f,
                "{typed} is inside {declared}, which is already declared; the declared root \
                 already covers it. Nothing was changed."
            ),
            RootError::TooBroad(p) => write!(
                f,
                "{} would put the whole machine in scope; declare the directories that hold \
                 your projects instead.",
                p.display()
            ),
            RootError::NotDeclared(t) => write!(
                f,
                "{t} is not a declared root; nothing was changed. `swamp config show` lists \
                 the declared roots."
            ),
            RootError::NotUtf8(p) => write!(
                f,
                "{} is not valid UTF-8 and cannot be written to config.toml.",
                p.display()
            ),
            RootError::Config(m) => write!(f, "{m}"),
            RootError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RootError {}

/// What [`add_root`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum AddOutcome {
    /// Recorded. `absorbed` lists the declared roots that were inside the
    /// new one and were removed as redundant.
    Added {
        stored: String,
        absorbed: Vec<String>,
    },
    /// Already declared, possibly spelled differently; the file was not
    /// touched.
    AlreadyDeclared { entry: String },
}

fn config_text(store: &StoreDir) -> Result<String, RootError> {
    let path = TextFile::Config { store }
        .path()
        .map_err(|e| RootError::Io(e.to_string()))?;
    match crate::fs_gate::read::read_owned_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(RootError::Io(format!("reading {}: {e}", path.display()))),
    }
}

fn parse(text: &str) -> Result<DocumentMut, RootError> {
    text.parse::<DocumentMut>().map_err(|e| {
        RootError::Config(format!(
            "config.toml is not valid TOML, so it was not edited: {e}"
        ))
    })
}

/// The `[scan] include` array, created (with `[scan]`) when `create`.
fn include_array(doc: &mut DocumentMut, create: bool) -> Result<Option<&mut Array>, RootError> {
    let shape = |what: &str| {
        RootError::Config(format!(
            "config.toml has {what}, so it was not edited; fix it by hand or remove it"
        ))
    };
    if doc.get("scan").is_none() {
        if !create {
            return Ok(None);
        }
        doc.insert("scan", toml_edit::table());
    }
    let scan = doc
        .get_mut("scan")
        .and_then(|i| {
            if i.is_table_like() {
                i.as_table_like_mut()
            } else {
                None
            }
        })
        .ok_or_else(|| shape("a `scan` key that is not a table"))?;
    if scan.get("include").is_none() {
        if !create {
            return Ok(None);
        }
        scan.insert("include", Item::Value(Value::Array(Array::new())));
    }
    match scan.get_mut("include") {
        Some(Item::Value(Value::Array(a))) => Ok(Some(a)),
        _ => Err(shape("`[scan] include` that is not an array")),
    }
}

fn entries(array: &Array) -> Result<Vec<String>, RootError> {
    array
        .iter()
        .map(|v| {
            v.as_str().map(str::to_string).ok_or_else(|| {
                RootError::Config(
                    "config.toml has a `[scan] include` entry that is not a string, so it \
                     was not edited"
                        .to_string(),
                )
            })
        })
        .collect()
}

/// The declared roots as written, in file order.
pub fn declared_entries(store: &StoreDir) -> Result<Vec<String>, RootError> {
    let mut doc = parse(&config_text(store)?)?;
    match include_array(&mut doc, false)? {
        Some(a) => entries(a),
        None => Ok(Vec::new()),
    }
}

/// What gets written: `~/...` when typed with a tilde, else the absolute
/// path. `.`, `..` and trailing slashes are gone; symlinks are not
/// resolved, so the entry reads the way the user wrote it.
fn stored_form(typed: &str, reach: &Reach) -> Result<String, RootError> {
    let typed = typed.trim();
    let absolute = if typed == "~" || typed.starts_with("~/") {
        configured_path(&reach.home, typed)
    } else {
        let p = Path::new(typed);
        if p.is_absolute() {
            configured_path(&reach.home, typed)
        } else {
            configured_path(&reach.home, &reach.cwd.join(p).to_string_lossy())
        }
    };
    let text = |p: &Path| {
        p.to_str()
            .map(str::to_string)
            .ok_or_else(|| RootError::NotUtf8(p.to_path_buf()))
    };
    if (typed == "~" || typed.starts_with("~/"))
        && let Ok(rest) = absolute.strip_prefix(&reach.home)
    {
        return Ok(if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", text(rest)?)
        });
    }
    text(&absolute)
}

/// The spelling two entries are compared in: symlinks resolved where the
/// path exists.
fn compare_key(entry: &str, reach: &Reach) -> PathBuf {
    comparable(&configured_path(&reach.home, entry))
}

fn raw(text: Option<&toml_edit::RawString>) -> &str {
    text.and_then(toml_edit::RawString::as_str).unwrap_or("")
}

/// Appends `entry` so a hand-written array keeps its shape: a one-line
/// array stays on one line, a multi-line one gets the entry on a line of
/// its own with the same indent, and a comment written after the last
/// entry stays on that entry's line instead of drifting onto the new one.
fn push_entry(array: &mut Array, entry: &str) {
    let multiline = array
        .iter()
        .any(|v| raw(v.decor().prefix()).contains('\n') || raw(v.decor().suffix()).contains('\n'))
        || array.trailing().as_str().is_some_and(|t| t.contains('\n'));
    if !multiline {
        array.push(entry);
        return;
    }
    let indent = array
        .iter()
        .next()
        .map(|v| {
            raw(v.decor().prefix())
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_else(|| "  ".to_string());
    // What sits between the last entry's separator and the closing `]`:
    // its trailing comment, if any, lives in the array's `trailing` (after
    // a trailing comma) or in the last entry's own suffix (without one).
    let mut tail = array.trailing().as_str().unwrap_or("").to_string();
    if !array.trailing_comma() {
        if let Some(last) = array.iter().last() {
            tail = raw(last.decor().suffix()).to_string();
        }
        let n = array.len();
        if let Some(last) = array.get_mut(n.wrapping_sub(1)) {
            last.decor_mut().set_suffix("");
        }
    }
    let carried = tail.trim_end_matches('\n');
    let mut value = Value::from(entry);
    value.decor_mut().set_prefix(format!("{carried}\n{indent}"));
    array.push_formatted(value);
    array.set_trailing("\n");
    array.set_trailing_comma(true);
}

fn write(store: &StoreDir, doc: &DocumentMut) -> Result<(), RootError> {
    crate::fs_gate::store::write_text(TextFile::Config { store }, &doc.to_string())
        .map_err(|e| RootError::Io(format!("writing config.toml: {e}")))
}

/// Declares `typed` as a source root.
///
/// Refused, with the file untouched: a path that does not exist (unless
/// `allow_missing`), is not a directory, cannot be read, is `/`, or is
/// inside a root already declared. A path that is already declared,
/// however spelled (trailing slash, `..`, a symlink, `~`), is a no-op. A
/// path that contains declared roots replaces them.
pub fn add_root(
    store: &StoreDir,
    reach: &Reach,
    typed: &str,
    allow_missing: bool,
) -> Result<AddOutcome, RootError> {
    let stored = stored_form(typed, reach)?;
    let expanded = configured_path(&reach.home, &stored);
    match crate::fs_gate::metadata_following(&expanded) {
        Ok(m) if !m.is_dir() => return Err(RootError::NotADirectory(expanded)),
        Ok(_) => {
            if let Err(e) = crate::fs_gate::probe_listable(&expanded) {
                return Err(RootError::Unreadable {
                    path: expanded,
                    reason: e.to_string(),
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !allow_missing {
                return Err(RootError::Missing(expanded));
            }
        }
        Err(e) => {
            return Err(RootError::Unreadable {
                path: expanded,
                reason: e.to_string(),
            });
        }
    }
    let key = comparable(&expanded);
    if key == Path::new("/") {
        return Err(RootError::TooBroad(key));
    }

    let _lock = store
        .lock_config_edits()
        .map_err(|e| RootError::Io(format!("locking config.toml: {e}")))?;
    let mut doc = parse(&config_text(store)?)?;
    let existing = match include_array(&mut doc, false)? {
        Some(a) => entries(a)?,
        None => Vec::new(),
    };
    let mut absorbed = Vec::new();
    for entry in &existing {
        let other = compare_key(entry, reach);
        if other == key {
            return Ok(AddOutcome::AlreadyDeclared {
                entry: entry.clone(),
            });
        }
        if key.starts_with(&other) {
            return Err(RootError::NestedUnder {
                typed: stored,
                declared: entry.clone(),
            });
        }
        if other.starts_with(&key) {
            absorbed.push(entry.clone());
        }
    }
    let array = include_array(&mut doc, true)?.expect("created");
    array.retain(|v| v.as_str().is_none_or(|s| !absorbed.iter().any(|a| a == s)));
    push_entry(array, &stored);
    write(store, &doc)?;
    Ok(AddOutcome::Added { stored, absorbed })
}

/// Stops declaring `typed`, matched however it is spelled. Returns the
/// entry that was removed. An undeclared root is an error, not a no-op, so
/// a typo does not look like success.
pub fn remove_root(store: &StoreDir, reach: &Reach, typed: &str) -> Result<String, RootError> {
    let stored = stored_form(typed, reach)?;
    let key = compare_key(&stored, reach);
    let _lock = store
        .lock_config_edits()
        .map_err(|e| RootError::Io(format!("locking config.toml: {e}")))?;
    let mut doc = parse(&config_text(store)?)?;
    let Some(array) = include_array(&mut doc, false)? else {
        return Err(RootError::NotDeclared(typed.to_string()));
    };
    let existing = entries(array)?;
    let Some(found) = existing
        .iter()
        .find(|e| **e == stored || compare_key(e, reach) == key)
        .cloned()
    else {
        return Err(RootError::NotDeclared(typed.to_string()));
    };
    array.retain(|v| v.as_str() != Some(found.as_str()));
    // A one-line array whose first entry was removed would start `[ "x"`.
    if let Some(first) = array.get_mut(0)
        && !raw(first.decor().prefix()).contains('\n')
    {
        first.decor_mut().set_prefix("");
    }
    write(store, &doc)?;
    Ok(found)
}

// --- first run ---------------------------------------------------------

/// Whether swamp should ask where the source code is: nothing has ever
/// been observed in this store and the config has no `[scan]` section. A
/// config that cannot be read or parsed never asks (the command's own
/// config error says so).
pub fn first_run_needed(store: &StoreDir) -> bool {
    if store.has_current_format().unwrap_or(true) {
        return false;
    }
    match config_text(store).and_then(|t| parse(&t)) {
        Ok(doc) => doc.get("scan").is_none(),
        Err(_) => false,
    }
}

/// What [`first_run`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum FirstRun {
    NotNeeded,
    /// No terminal: the one-line instruction was printed and the built-in
    /// defaults are used. Never blocks.
    Instructed,
    /// Asked; these entries were recorded (empty when the user skipped).
    Answered(Vec<String>),
}

/// The first-run question. `interactive` is whether both ends are a
/// terminal; without one this prints the instruction and returns at once
/// and `read_line` is never called. `read_line` returns the next line the
/// user typed, `None` at end of input.
///
/// It offers `~/src` only if that directory exists, which is the single
/// filesystem fact it looks at: it does not list, scan or probe anything
/// to propose a candidate. The answer goes through [`add_root`], so it is
/// validated like any other. A skipped answer records an empty `[scan]`
/// section so the question is not asked again.
pub fn first_run(
    store: &StoreDir,
    reach: &Reach,
    interactive: bool,
    read_line: &mut dyn FnMut() -> Option<String>,
    out: &mut dyn Write,
) -> Result<FirstRun, RootError> {
    if !first_run_needed(store) {
        return Ok(FirstRun::NotNeeded);
    }
    let io = |e: std::io::Error| RootError::Io(e.to_string());
    if !interactive {
        writeln!(
            out,
            "No source directories declared; using the built-in defaults. \
             Declare yours with: swamp config add-root <path>"
        )
        .map_err(io)?;
        return Ok(FirstRun::Instructed);
    }
    let offer =
        crate::fs_gate::metadata_following(reach.home.join("src")).is_ok_and(|m| m.is_dir());
    let mut recorded: Vec<String> = Vec::new();
    let mut failures = 0;
    loop {
        let prompt = if !recorded.is_empty() {
            "Another source directory? Press Enter when done: "
        } else if offer {
            "Where is your source code? Press Enter for ~/src, or type a directory: "
        } else {
            "Where is your source code? Type a directory, or press Enter to use the \
             built-in defaults: "
        };
        write!(out, "{prompt}").map_err(io)?;
        out.flush().map_err(io)?;
        let line = read_line();
        let answer = line.as_deref().map_or("", str::trim);
        let answer = if answer.is_empty() {
            if line.is_none() || !recorded.is_empty() || !offer {
                break;
            }
            "~/src"
        } else {
            answer
        };
        match add_root(store, reach, answer, false) {
            Ok(AddOutcome::Added { stored, .. }) => {
                writeln!(out, "  recorded {stored}").map_err(io)?;
                recorded.push(stored);
            }
            Ok(AddOutcome::AlreadyDeclared { entry }) => {
                writeln!(out, "  already declared: {entry}").map_err(io)?;
            }
            Err(e) => {
                writeln!(out, "  {e}").map_err(io)?;
                failures += 1;
                if failures >= 3 {
                    break;
                }
            }
        }
    }
    if recorded.is_empty() {
        // Remember that the question was asked.
        let _lock = store
            .lock_config_edits()
            .map_err(|e| RootError::Io(e.to_string()))?;
        let mut doc = parse(&config_text(store)?)?;
        include_array(&mut doc, true)?;
        write(store, &doc)?;
        writeln!(
            out,
            "No roots recorded; using the built-in defaults. Add one any time with: \
             swamp config add-root <path>"
        )
        .map_err(io)?;
    }
    Ok(FirstRun::Answered(recorded))
}

// --- status ------------------------------------------------------------

/// How one declared root stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredState {
    /// On disk. `bytes` is from the stored observation, `None` when this
    /// scope has none yet (never measured, not zero); `complete` is false
    /// when part of the root could not be read, making it a lower bound.
    Present {
        bytes: Option<u64>,
        complete: bool,
    },
    Missing,
    Unreadable(String),
    /// Folded into a wider root's own walk.
    CoveredBy(PathBuf),
    Excluded(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRoot {
    pub path: PathBuf,
    pub state: DeclaredState,
}

/// Every root the config declares, in scope order, with its state. Bytes
/// come from `coverage` (the stored observation's per-root rows); this
/// function reads nothing else and walks nothing.
pub fn declared_roots(scope: &EffectiveScope, coverage: &[RootCoverage]) -> Vec<DeclaredRoot> {
    scope
        .roots
        .iter()
        .filter(|r| r.reasons.iter().any(|x| matches!(x, RootReason::Included)))
        .map(|r| DeclaredRoot {
            path: r.path.clone(),
            state: match &r.status {
                RootStatus::Present => {
                    let row = coverage.iter().find(|c| c.path == r.path);
                    match row.map(|c| &c.status) {
                        Some(RegionStatus::Complete) => DeclaredState::Present {
                            bytes: row.map(|c| c.walked_total),
                            complete: true,
                        },
                        Some(RegionStatus::Partial { .. }) => DeclaredState::Present {
                            bytes: row.map(|c| c.walked_total),
                            complete: false,
                        },
                        _ => DeclaredState::Present {
                            bytes: None,
                            complete: true,
                        },
                    }
                }
                RootStatus::Missing => DeclaredState::Missing,
                RootStatus::Unreadable { reason } => DeclaredState::Unreadable(reason.clone()),
                RootStatus::SkippedAsNested { parent } => DeclaredState::CoveredBy(parent.clone()),
                RootStatus::Excluded { pattern } => DeclaredState::Excluded(pattern.clone()),
            },
        })
        .collect()
}

/// The lines `swamp scope`, `swamp config show` and `swamp report` print
/// for the declared roots. Empty when none are declared.
pub fn render_declared_roots(roots: &[DeclaredRoot]) -> String {
    use std::fmt::Write as _;
    if roots.is_empty() {
        return String::new();
    }
    let mut out = String::from("declared source roots:\n");
    for r in roots {
        let (label, detail) = match &r.state {
            DeclaredState::Present {
                bytes: Some(b),
                complete: true,
            } => (
                "present",
                format!(
                    "{} (stored observation)",
                    crate::render::human_bytes_pub(*b)
                ),
            ),
            DeclaredState::Present {
                bytes: Some(b),
                complete: false,
            } => (
                "present",
                format!(
                    "at least {} (part could not be read; stored observation)",
                    crate::render::human_bytes_pub(*b)
                ),
            ),
            DeclaredState::Present { bytes: None, .. } => (
                "present",
                "not measured yet; `swamp observe` measures it".to_string(),
            ),
            DeclaredState::Missing => (
                "missing",
                "not measured; not mounted or gone, which is a coverage fact and nothing was \
                 removed"
                    .to_string(),
            ),
            DeclaredState::Unreadable(why) => ("unreadable", format!("not measured ({why})")),
            DeclaredState::CoveredBy(p) => ("covered", format!("by {}", p.display())),
            DeclaredState::Excluded(p) => ("excluded", format!("by {p}")),
        };
        let _ = writeln!(out, "  {label:<10} {}  {detail}", r.path.display());
    }
    out
}
