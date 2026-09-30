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
use crate::scope::{EffectiveScope, RootReason, RootStatus, comparable, configured_path, lexical};
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
    /// A directory that holds far more than one person's projects (`/`,
    /// `/Users`, the home directory itself, ...).
    TooBroad(PathBuf),
    /// `~user/...`: swamp cannot resolve another account's home.
    UnsupportedTilde(String),
    /// `~` was typed but there is no usable home directory.
    NoHome,
    NotDeclared(String),
    /// The path cannot be written to a TOML file as text.
    NotUtf8(PathBuf),
    /// `config.toml` cannot be edited safely (not TOML, or `[scan]` /
    /// `include` has the wrong shape). The file is left as it is.
    Config(String),
    Io(String),
}

/// Text safe to echo to a terminal: control characters, escape sequences
/// included, become `?`.
pub fn sanitized(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn shown(p: &Path) -> String {
    sanitized(&p.display().to_string())
}

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootError::Missing(p) => write!(
                f,
                "{} does not exist. Check the spelling, or pass --allow-missing to record a \
                 root that is not mounted yet (it is then reported as missing, never as an error).",
                shown(p)
            ),
            RootError::NotADirectory(p) => write!(
                f,
                "{} is not a directory; a source root is a directory that holds projects.",
                shown(p)
            ),
            RootError::Unreadable { path, reason } => write!(
                f,
                "{} exists but this user cannot read it ({reason}); swamp only declares roots \
                 within your reach.",
                shown(path)
            ),
            RootError::NestedUnder { typed, declared } => write!(
                f,
                "{} is inside {}, which is already declared; the declared root \
                 already covers it. Nothing was changed.",
                sanitized(typed),
                sanitized(declared)
            ),
            RootError::TooBroad(p) => write!(
                f,
                "{} holds far more than your projects, so declaring it would put a whole \
                 machine area in scope; declare a narrower directory that holds your \
                 projects instead.",
                shown(p)
            ),
            RootError::UnsupportedTilde(t) => write!(
                f,
                "{} uses another account's home (`~name`), which is not supported; type the \
                 full path instead.",
                sanitized(t)
            ),
            RootError::NoHome => write!(
                f,
                "`~` needs a home directory and none is set (HOME is empty); type the full \
                 path instead."
            ),
            RootError::NotDeclared(t) => write!(
                f,
                "{} is not a declared root; nothing was changed. `swamp config show` lists \
                 the declared roots.",
                sanitized(t)
            ),
            RootError::NotUtf8(p) => write!(
                f,
                "{} is not valid UTF-8 and cannot be written to config.toml.",
                shown(p)
            ),
            RootError::Config(m) => write!(f, "{}", sanitized(m)),
            RootError::Io(m) => write!(f, "{}", sanitized(m)),
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

/// A typed path as an absolute path and the text that gets written:
/// `~/...` when typed with a tilde, else the absolute path. `.`, `..`,
/// repeated and trailing slashes are gone; symlinks are not resolved, so
/// the entry reads the way the user wrote it. `~user` forms, `~` without a
/// home directory and a relative path under a non-UTF-8 working directory
/// are refused, never guessed.
fn stored_form(typed: &str, reach: &Reach) -> Result<(String, PathBuf), RootError> {
    let typed = typed.trim();
    let text = |p: &Path| {
        p.to_str()
            .map(str::to_string)
            .ok_or_else(|| RootError::NotUtf8(p.to_path_buf()))
    };
    if let Some(rest) = typed.strip_prefix('~') {
        if !(rest.is_empty() || rest.starts_with('/')) {
            return Err(RootError::UnsupportedTilde(typed.to_string()));
        }
        if !reach.home.is_absolute() {
            return Err(RootError::NoHome);
        }
        let below = rest.trim_start_matches('/');
        let absolute = lexical(&reach.home.join(below));
        if let Ok(inside) = absolute.strip_prefix(&reach.home) {
            let stored = if inside.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", text(inside)?)
            };
            return Ok((stored, absolute));
        }
        return Ok((text(&absolute)?, absolute));
    }
    let p = Path::new(typed);
    let absolute = if p.is_absolute() {
        lexical(p)
    } else {
        lexical(&reach.cwd.join(p))
    };
    Ok((text(&absolute)?, absolute))
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

/// Writes the edited document. A file that used CRLF stays CRLF.
fn write(store: &StoreDir, doc: &DocumentMut, crlf: bool) -> Result<(), RootError> {
    let mut text = doc.to_string();
    if crlf {
        text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    crate::fs_gate::store::write_config_in_place(store, &text).map_err(|e| {
        RootError::Io(format!(
            "config.toml was not changed: a temporary file could not be written next to it ({e})"
        ))
    })
}

/// Whether the config file text uses CRLF line endings.
fn uses_crlf(text: &str) -> bool {
    text.contains("\r\n")
}

/// Declares `typed` as a source root.
///
/// Refused, with the file untouched: a path that does not exist (unless
/// `allow_missing`), is not a directory, cannot be read, is too broad (`/`, the home
/// directory, ...), or is
/// inside a root already declared. A path that is already declared,
/// however spelled (trailing slash, `..`, a symlink, `~`), is a no-op. A
/// path that contains declared roots leaves them declared (they show as
/// covered) and says so.
pub fn add_root(
    store: &StoreDir,
    reach: &Reach,
    typed: &str,
    allow_missing: bool,
) -> Result<AddOutcome, RootError> {
    let (stored, expanded) = stored_form(typed, reach)?;
    // Everything below, the checks included, happens under the lock, so
    // what was validated is what is recorded.
    let _lock = store
        .lock_config_edits()
        .map_err(|e| RootError::Io(format!("locking config.toml: {e}")))?;
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
    if too_broad(&key, reach) {
        return Err(RootError::TooBroad(key));
    }
    let original = config_text(store)?;
    let mut doc = parse(&original)?;
    let existing = match include_array(&mut doc, false)? {
        Some(a) => entries(a)?,
        None => Vec::new(),
    };
    let mut covered = Vec::new();
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
            covered.push(entry.clone());
        }
    }
    let array = include_array(&mut doc, true)?.expect("created");
    push_entry(array, &stored);
    write(store, &doc, uses_crlf(&original))?;
    Ok(AddOutcome::Added {
        stored,
        absorbed: covered,
    })
}

/// Directories that hold far more than one person's projects: the
/// filesystem root, the system and user areas, and the home directory
/// itself.
fn too_broad(key: &Path, reach: &Reach) -> bool {
    const AREAS: &[&str] = &[
        "/",
        "/Users",
        "/Volumes",
        "/private",
        "/System",
        "/opt",
        "/usr",
        "/home",
        "/var",
        "/Library",
        "/Applications",
    ];
    AREAS.iter().any(|a| key == comparable(Path::new(a))) || key == comparable(&reach.home)
}

/// Stops declaring `typed`, matched however it is spelled. Returns the
/// entry that was removed. An undeclared root is an error, not a no-op, so
/// a typo does not look like success.
pub fn remove_root(store: &StoreDir, reach: &Reach, typed: &str) -> Result<String, RootError> {
    let (stored, _) = stored_form(typed, reach)?;
    let key = compare_key(&stored, reach);
    let _lock = store
        .lock_config_edits()
        .map_err(|e| RootError::Io(format!("locking config.toml: {e}")))?;
    let original = config_text(store)?;
    let mut doc = parse(&original)?;
    let Some(array) = include_array(&mut doc, false)? else {
        return Err(RootError::NotDeclared(typed.to_string()));
    };
    // Every entry that spells this root, however it is spelled.
    let found: Vec<String> = entries(array)?
        .into_iter()
        .filter(|e| *e == stored || compare_key(e, reach) == key)
        .collect();
    if found.is_empty() {
        return Err(RootError::NotDeclared(typed.to_string()));
    }
    array.retain(|v| v.as_str().is_none_or(|s| !found.iter().any(|f| f == s)));
    // A one-line array whose first entry was removed would start `[ "x"`.
    if let Some(first) = array.get_mut(0)
        && !raw(first.decor().prefix()).contains('\n')
    {
        first.decor_mut().set_prefix("");
    }
    write(store, &doc, uses_crlf(&original))?;
    Ok(found.join(", "))
}

// --- first run ---------------------------------------------------------

/// Whether swamp should ask where the source code is: the store holds no
/// observation, the config has no `[scan]` section, and the question has
/// not been asked before. Deliberately not tied to the store-format
/// marker: a later store reset must not re-ask anyone. A store that
/// already holds an index, or a config that cannot be read or parsed
/// (the command's own error says so), never asks. When an existing index
/// is what settles it, that is recorded, so a reset later cannot make it
/// look like a first run.
pub fn first_run_needed(store: &StoreDir) -> bool {
    if crate::fs_gate::store::onboarded_recorded(store) {
        return false;
    }
    let has_index = store.has_current_format().unwrap_or(true)
        || store.has_incompatible_marker().unwrap_or(true);
    let scan = match config_text(store).and_then(|t| parse(&t)) {
        Ok(doc) => doc.get("scan").is_some(),
        Err(_) => return false,
    };
    if has_index || scan {
        if has_index {
            record_onboarded(store);
        }
        return false;
    }
    true
}

/// Best effort: remember that this store is past its first run.
fn record_onboarded(store: &StoreDir) {
    let _ = crate::fs_gate::store::write_text(TextFile::Onboarded { store }, "asked\n");
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
            "Where is your source code? Press Enter for ~/src, or type a directory \
             (~/src stays a built-in default either way): "
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
                writeln!(out, "  recorded {}", sanitized(&stored)).map_err(io)?;
                recorded.push(stored);
            }
            Ok(AddOutcome::AlreadyDeclared { entry }) => {
                writeln!(out, "  already declared: {}", sanitized(&entry)).map_err(io)?;
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
    record_onboarded(store);
    if recorded.is_empty() {
        // Remember that the question was asked.
        let _lock = store
            .lock_config_edits()
            .map_err(|e| RootError::Io(e.to_string()))?;
        let original = config_text(store)?;
        let mut doc = parse(&original)?;
        include_array(&mut doc, true)?;
        write(store, &doc, uses_crlf(&original))?;
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
                // Present by `stat`, but a directory this user cannot open
                // is unreadable, however it got declared. The probe opens
                // the directory and reads no entry.
                RootStatus::Present => {
                    let row = coverage.iter().find(|c| c.path == r.path);
                    if let Err(e) = crate::fs_gate::probe_listable(&r.path) {
                        return DeclaredRoot {
                            path: r.path.clone(),
                            state: DeclaredState::Unreadable(e.to_string()),
                        };
                    }
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
