//! Mutation evidence for the capability gates (`docs/architecture.md`,
//! "Capability gates").
//!
//! Every mutation under `tests/mutations/` -- the 185-fixture corpus
//! (which includes re-review 3's 45 sweep mutations as `*-sweep3.rs`),
//! re-review 4's 46 + 12 + 7 sweep mutations (`sweep4-M*`, `sweep4-W*`,
//! `sweep4-U*`, converted from `reviewer_counterexamples_stack4_sweep.rs`)
//! and the legitimate shapes (`expect: accept`) -- is applied to a copy of
//! the real workspace, and judged by **what actually rejected it**:
//!
//! * `parse` -- a touched file no longer parses. Never a rejection: a
//!   mutation that does not parse proves nothing about any rule.
//! * `audit:<rule>` -- the named source audit fails on the mutated tree
//!   (the unmutated tree passes every audit, so the failure is the
//!   mutation's).
//! * `compile:<code>` -- `cargo clippy --lib --bins` of the production
//!   crates reports an **error** with that code (a rustc code such as
//!   `E0616`, or a denied lint such as `clippy::disallowed_methods`)
//!   whose primary span lies inside the lines the mutation wrote.
//!
//! A rejecting fixture names the kinds that count for it on a
//! `//! by:` line (any of them); any other failure -- a stale name, a
//! dead-code rule tripping over a new helper, an error somewhere else in
//! the file -- does not count. An accepted fixture must pass every audit
//! and compile with no error at all.
//!
//! `mutation_operators_keep_the_rejection_kind` then derives variants
//! mechanically from every rejected seed (the old harness's `alias`,
//! `pub_use`, `helper`, `child_module`, `macro_wrap`, `via_constant`,
//! and re-review 4's `fn_item_binding` and `doc_cfg_test`) and requires
//! each variant to be rejected by one of its seed's own kinds; and from
//! every accepted seed, requiring the variants to stay accepted.
//!
//! Compiles run in a separate target directory under the shared one
//! (`<target>/tests/mutation-sweep`, as trybuild uses
//! `<target>/tests/trybuild`), batched: every pending fixture is applied
//! at once, errors are attributed by span, and only what a batch could
//! not decide is compiled again (clippy's lints run only when rustc found
//! no error, and a core error hides the TUI and CLI).
//!
//! `MUTATION_SURVEY=1` prints every fixture's full verdict (all audits,
//! all in-region compile codes) without asserting.

use quote::{ToTokens, quote};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use syn::visit_mut::VisitMut;

/// One sweep at a time: both tests copy the workspace and run cargo in
/// the same target directory.
static SERIAL: Mutex<()> = Mutex::new(());

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// The shared target directory this test binary was built into
/// (`<target>/debug/deps/<bin>`).
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.ancestors().nth(3).unwrap().to_path_buf()
}

fn survey() -> bool {
    std::env::var_os("MUTATION_SURVEY").is_some()
}

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Append,
    Create,
    Replace,
    Substitute(String),
}

#[derive(Clone, Debug)]
struct Section {
    target: String,
    mode: Mode,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Audit(String),
    Compile(String),
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Audit(r) => write!(f, "audit:{r}"),
            Kind::Compile(c) => write!(f, "compile:{c}"),
        }
    }
}

fn parse_kind(s: &str) -> Kind {
    let s = s.trim();
    if let Some(r) = s.strip_prefix("audit:") {
        Kind::Audit(r.trim().to_string())
    } else if let Some(c) = s.strip_prefix("compile:") {
        Kind::Compile(c.trim().to_string())
    } else {
        panic!("unknown rejection kind `{s}` (audit:<rule> or compile:<code>)")
    }
}

#[derive(Clone, Debug)]
struct Fixture {
    group: String,
    name: String,
    accept: bool,
    /// `expect: retired`: the mutation no longer says anything about this
    /// tree (it targets an API the gates removed, or only defines a
    /// function nothing can reach); `retired:` says why. Listed, never
    /// applied, never counted as a rejection.
    retired: Option<String>,
    by: Vec<Kind>,
    source: String,
    files: Vec<Section>,
}

impl Fixture {
    fn id(&self) -> String {
        format!("{}/{}", self.group, self.name)
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn mode_of(v: &str, find: Option<String>) -> Mode {
    match v.trim() {
        "append" => Mode::Append,
        "create" => Mode::Create,
        "replace" => Mode::Replace,
        "substitute" => Mode::Substitute(find.expect("`mode: substitute` needs a `find:` line")),
        other => panic!("unknown mode `{other}`"),
    }
}

/// The fixture format: a header of `//! key: value` lines (`target`,
/// `mode`, `find`, `expect`, `by`, `source`, `why`, ...), then the body;
/// a `//! file: <path>` line (with its own `//! mode:`/`//! find:` right
/// after) starts another section.
fn parse_fixture(group: &str, path: &Path) -> Fixture {
    let text = std::fs::read_to_string(path).unwrap();
    let mut header: BTreeMap<String, String> = BTreeMap::new();
    let mut by: Vec<Kind> = Vec::new();
    let mut sections: Vec<(String, String, Option<String>, String)> = Vec::new();
    let mut in_header = true;
    let mut after_file = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("//! ")
            && let Some((k, v)) = rest.split_once(": ")
        {
            let k = k.trim();
            if in_header
                && k != "file"
                && (!k.contains(char::is_whitespace) || k.starts_with("blind-spot"))
            {
                if k == "by" {
                    by.extend(v.split(',').map(parse_kind));
                } else {
                    header.insert(k.to_string(), v.to_string());
                }
                continue;
            }
            if k == "file" {
                if sections.is_empty() {
                    sections.push((
                        header.get("target").cloned().unwrap_or_default(),
                        header
                            .get("mode")
                            .cloned()
                            .unwrap_or_else(|| "append".into()),
                        header.get("find").map(|f| unescape(f)),
                        String::new(),
                    ));
                }
                sections.push((v.trim().to_string(), "append".into(), None, String::new()));
                in_header = false;
                after_file = true;
                continue;
            }
            if after_file && (k == "mode" || k == "find") {
                let last = sections.last_mut().unwrap();
                if k == "mode" {
                    last.1 = v.trim().to_string();
                } else {
                    last.2 = Some(unescape(v));
                }
                continue;
            }
        }
        if in_header {
            in_header = false;
            if sections.is_empty() {
                sections.push((
                    header
                        .get("target")
                        .cloned()
                        .unwrap_or_else(|| panic!("{} has no `//! target:`", path.display())),
                    header
                        .get("mode")
                        .cloned()
                        .unwrap_or_else(|| "append".into()),
                    header.get("find").map(|f| unescape(f)),
                    String::new(),
                ));
            }
        }
        after_file = false;
        let last = sections.last_mut().unwrap();
        last.3.push_str(line);
        last.3.push('\n');
    }
    if sections.is_empty() {
        sections.push((
            header.get("target").cloned().unwrap_or_default(),
            header
                .get("mode")
                .cloned()
                .unwrap_or_else(|| "append".into()),
            header.get("find").map(|f| unescape(f)),
            String::new(),
        ));
    }
    let files = sections
        .into_iter()
        .map(|(target, mode, find, body)| {
            let mode = mode_of(&mode, find);
            // A substitution's replacement is the text itself, not a line.
            let body = if matches!(mode, Mode::Substitute(_)) {
                body.strip_suffix('\n').unwrap_or(&body).to_string()
            } else {
                body
            };
            Section { target, mode, body }
        })
        .collect();
    Fixture {
        group: group.to_string(),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        accept: header.get("expect").map(String::as_str) == Some("accept"),
        retired: (header.get("expect").map(String::as_str) == Some("retired")).then(|| {
            header.get("retired").cloned().unwrap_or_else(|| {
                panic!(
                    "{}: `expect: retired` needs a `retired:` reason",
                    path.display()
                )
            })
        }),
        by,
        source: header.get("source").cloned().unwrap_or_else(|| {
            if path.to_string_lossy().ends_with("-sweep3.rs") {
                "review-3 sweep".into()
            } else {
                "corpus".into()
            }
        }),
        files,
    }
}

fn fixtures() -> Vec<Fixture> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mutations");
    let mut out = Vec::new();
    let mut groups: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    groups.sort();
    for g in groups {
        let group = g.file_name().unwrap().to_string_lossy().into_owned();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&g)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("rs"))
            .collect();
        files.sort();
        for f in files {
            out.push(parse_fixture(&group, &f));
        }
    }
    out
}

// ---------------------------------------------------------------------
// The workspace copy
// ---------------------------------------------------------------------

const COPIED: &[&str] = &[
    "crates",
    ".oh",
    "docs",
    "scripts",
    "Cargo.toml",
    "Cargo.lock",
    "CHANGELOG.md",
];

fn copy_tree(from: &Path, to: &Path) {
    if from.is_file() {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(from, to).unwrap();
        return;
    }
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let name = e.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        copy_tree(&e.path(), &to.join(&name));
    }
}

fn fresh_copy(name: &str) -> PathBuf {
    let base = target_dir().join("tests/mutation-sweep");
    let work = base.join(name);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let root = repo_root();
    for c in COPIED {
        if root.join(c).exists() {
            copy_tree(&root.join(c), &work.join(c));
        }
    }
    work
}

/// What a set of applied fixtures wrote, per fixture: `(file, lines)`.
type Regions = Vec<(String, RangeInclusive<usize>)>;

struct Applied {
    originals: Vec<(PathBuf, Option<String>)>,
    regions: Vec<Regions>,
    touched: BTreeSet<String>,
}

fn line_count(s: &str) -> usize {
    s.lines().count()
}

/// Applies fixtures in order. `Err` names a fixture that cannot be
/// applied to this tree (a substitution whose text is not there once).
fn apply_all(work: &Path, fs: &[&Fixture]) -> Result<Applied, (usize, String)> {
    let mut applied = Applied {
        originals: Vec::new(),
        regions: Vec::new(),
        touched: BTreeSet::new(),
    };
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for (i, f) in fs.iter().enumerate() {
        let mut mine: Regions = Vec::new();
        for s in &f.files {
            let path = work.join(&s.target);
            let original = std::fs::read_to_string(&path).ok();
            if seen.insert(path.clone()) {
                applied.originals.push((path.clone(), original.clone()));
            }
            let current = original.clone().unwrap_or_default();
            let (new, region) = match &s.mode {
                Mode::Append => {
                    let base = if current.ends_with('\n') || current.is_empty() {
                        current.clone()
                    } else {
                        format!("{current}\n")
                    };
                    let start = line_count(&base) + 1;
                    let new = format!("{base}{}", s.body);
                    let end = line_count(&new).max(start);
                    (new, start..=end)
                }
                Mode::Create | Mode::Replace => {
                    let end = line_count(&s.body).max(1);
                    (s.body.clone(), 1..=end)
                }
                Mode::Substitute(find) => {
                    let hits = current.matches(find.as_str()).count();
                    if hits != 1 {
                        restore(std::mem::take(&mut applied.originals));
                        return Err((i, format!("`{}` occurs {hits} times in {}", find, s.target)));
                    }
                    let at = current.find(find.as_str()).unwrap();
                    let start = line_count(&current[..at]) + 1;
                    let delta = line_count(&s.body) as isize - line_count(find) as isize;
                    // Regions already recorded below this point move.
                    for rs in applied.regions.iter_mut().chain(std::iter::once(&mut mine)) {
                        for (file, r) in rs.iter_mut() {
                            if *file == s.target && *r.start() > start {
                                let (a, b) =
                                    (*r.start() as isize + delta, *r.end() as isize + delta);
                                *r = (a.max(1) as usize)..=(b.max(1) as usize);
                            }
                        }
                    }
                    let new = current.replacen(find.as_str(), &s.body, 1);
                    let end = start + line_count(&s.body).saturating_sub(1);
                    (new, start..=end)
                }
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, new).unwrap();
            applied.touched.insert(s.target.clone());
            mine.push((s.target.clone(), region));
        }
        applied.regions.push(mine);
    }
    Ok(applied)
}

fn restore(originals: Vec<(PathBuf, Option<String>)>) {
    for (path, original) in originals.into_iter().rev() {
        match original {
            Some(t) => std::fs::write(&path, t).unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ---------------------------------------------------------------------
// Verdicts
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Verdict {
    /// A touched `.rs` file failed to parse, or the fixture could not be
    /// applied.
    broken: Option<String>,
    /// Failing audits, with their first problem line.
    audits: BTreeMap<String, String>,
    /// Error codes with a primary span inside the fixture's lines.
    compile_in: BTreeMap<String, String>,
    /// Error diagnostics anywhere else (the mutation broke other code).
    compile_elsewhere: usize,
    compiled: bool,
}

impl Verdict {
    fn kinds(&self) -> BTreeSet<Kind> {
        let mut k: BTreeSet<Kind> = self.audits.keys().map(|r| Kind::Audit(r.clone())).collect();
        k.extend(self.compile_in.keys().map(|c| Kind::Compile(c.clone())));
        k
    }
    fn rejected_by(&self, wanted: &[Kind]) -> Option<Kind> {
        if self.broken.is_some() {
            return None;
        }
        let have = self.kinds();
        wanted.iter().find(|k| have.contains(k)).cloned()
    }

    /// For an operator variant: rejected by one of `wanted`, or by the
    /// same *class* of compile error -- a name that does not exist in the
    /// production API is `E0425` called directly and `E0432` imported
    /// under an alias.
    fn rejected_like(&self, wanted: &[Kind]) -> Option<Kind> {
        if let Some(k) = self.rejected_by(wanted) {
            return Some(k);
        }
        if self.broken.is_some() {
            return None;
        }
        let class = |k: &Kind| match k {
            Kind::Compile(c) => UNRESOLVED.contains(&c.as_str()).then_some("unresolved"),
            Kind::Audit(_) => None,
        };
        let want: HashSet<&str> = wanted.iter().filter_map(class).collect();
        self.kinds()
            .into_iter()
            .find(|k| class(k).is_some_and(|c| want.contains(c)))
    }
    fn clean(&self) -> bool {
        self.broken.is_none()
            && self.audits.is_empty()
            && self.compiled
            && self.compile_in.is_empty()
            && self.compile_elsewhere == 0
    }
}

/// rustc's "this name does not exist here" codes.
const UNRESOLVED: &[&str] = &["E0425", "E0432", "E0433", "E0412", "E0531"];

fn parse_check(work: &Path, touched: &BTreeSet<String>) -> Option<String> {
    for t in touched {
        if !t.ends_with(".rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(work.join(t)) else {
            continue;
        };
        if let Err(e) = syn::parse_file(&text) {
            return Some(format!("{t} does not parse: {e}"));
        }
    }
    None
}

fn run_audits(work: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, r) in swamp_source_audit::audits::run_all(work) {
        if let Err(e) = r {
            let first = e.lines().nth(1).unwrap_or("").trim().to_string();
            out.insert(name.to_string(), first);
        }
    }
    out
}

fn audit_verdict(work: &Path, f: &Fixture) -> Verdict {
    let mut v = Verdict::default();
    let applied = match apply_all(work, &[f]) {
        Ok(a) => a,
        Err((_, why)) => {
            v.broken = Some(format!("not applicable: {why}"));
            return v;
        }
    };
    v.broken = parse_check(work, &applied.touched);
    if v.broken.is_none() {
        v.audits = run_audits(work);
    }
    restore(applied.originals);
    v
}

#[derive(Debug, Clone)]
struct Diag {
    code: String,
    file: String,
    line: usize,
    line_end: usize,
    message: String,
}

/// `cargo clippy` of the production targets of the copy: every
/// error-level diagnostic.
fn compile(work: &Path) -> Vec<Diag> {
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(work)
        .args([
            "clippy",
            "--workspace",
            "--exclude",
            "swamp-source-audit",
            "--lib",
            "--bins",
            "--locked",
            "--offline",
            "--message-format=json",
            "--target-dir",
        ])
        .arg(target_dir().join("tests/mutation-sweep/target"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .output()
        .expect("run cargo clippy on the mutated copy");
    let mut diags = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let m = &v["message"];
        if m["level"] != "error" {
            continue;
        }
        let code = m["code"]["code"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Uncoded errors: name them by their message's shape.
                let msg = m["message"].as_str().unwrap_or("");
                if msg.contains("with struct literal syntax due to private fields") {
                    "private-fields-literal".into()
                } else if msg.starts_with("aborting due to") || msg.starts_with("could not compile")
                {
                    String::new()
                } else {
                    "uncoded".into()
                }
            });
        if code.is_empty() {
            continue;
        }
        let spans = m["spans"].as_array().cloned().unwrap_or_default();
        let primary = spans
            .iter()
            .find(|s| s["is_primary"] == true)
            .or(spans.first());
        let (file, line, line_end) = match primary {
            Some(s) => (
                s["file_name"].as_str().unwrap_or("").to_string(),
                s["line_start"].as_u64().unwrap_or(0) as usize,
                s["line_end"].as_u64().unwrap_or(0) as usize,
            ),
            None => (String::new(), 0, 0),
        };
        diags.push(Diag {
            code,
            file,
            line,
            line_end,
            message: m["message"].as_str().unwrap_or("").to_string(),
        });
    }
    if diags.is_empty() && !out.status.success() {
        diags.push(Diag {
            code: "cargo-failed".into(),
            file: String::new(),
            line: 0,
            line_end: 0,
            message: String::from_utf8_lossy(&out.stderr)
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .join(" | "),
        });
    }
    diags
}

fn in_regions(d: &Diag, regions: &Regions) -> bool {
    regions
        .iter()
        .any(|(f, r)| *f == d.file && d.line <= *r.end() && d.line_end.max(d.line) >= *r.start())
}

fn crate_of(target: &str) -> Option<&'static str> {
    for (prefix, k) in [
        ("crates/core/src/", "core"),
        ("crates/tui/src/", "tui"),
        ("crates/cli/src/", "cli"),
    ] {
        if target.starts_with(prefix) {
            return Some(k);
        }
    }
    None
}

fn compiles_something(f: &Fixture) -> bool {
    f.files.iter().any(|s| crate_of(&s.target).is_some())
}

/// Whether `f` can join a batch that already writes `claimed`.
fn batchable(f: &Fixture, claimed: &HashMap<String, bool>) -> bool {
    f.files.iter().all(|s| {
        let exclusive = !matches!(s.mode, Mode::Append);
        match claimed.get(&s.target) {
            None => true,
            Some(other_exclusive) => !exclusive && !other_exclusive,
        }
    })
}

/// Compiles each fixture's mutation (batched) and fills in the compile
/// half of its verdict.
fn compile_verdicts(work: &Path, fs: &[&Fixture], verdicts: &mut HashMap<String, Verdict>) {
    let mut pending: Vec<&Fixture> = fs.to_vec();
    let mut individually = false;
    while !pending.is_empty() {
        let mut batch: Vec<&Fixture> = Vec::new();
        let mut claimed: HashMap<String, bool> = HashMap::new();
        let mut rest: Vec<&Fixture> = Vec::new();
        for f in pending.drain(..) {
            if (individually && !batch.is_empty()) || !batchable(f, &claimed) {
                rest.push(f);
                continue;
            }
            for s in &f.files {
                let exclusive = !matches!(s.mode, Mode::Append);
                let e = claimed.entry(s.target.clone()).or_insert(false);
                *e |= exclusive;
            }
            batch.push(f);
        }
        // Fixtures written in one style reuse helper names (`sweep_slurp`);
        // side by side in one file they would collide, so each gets its
        // own suffix for the batch.
        let renamed: Vec<Fixture> = batch
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if batch.len() > 1 {
                    suffixed(f, &format!("b{i}"))
                } else {
                    (*f).clone()
                }
            })
            .collect();
        let renamed_refs: Vec<&Fixture> = renamed.iter().collect();
        let applied = match apply_all(work, &renamed_refs) {
            Ok(a) => a,
            Err((i, why)) => {
                let bad = batch.remove(i);
                verdicts.get_mut(&bad.id()).unwrap().broken =
                    Some(format!("not applicable: {why}"));
                pending = batch.into_iter().chain(rest).collect();
                continue;
            }
        };
        let diags = compile(work);
        restore(applied.originals);
        let mut decided = 0;
        let mut undecided: Vec<&Fixture> = Vec::new();
        for (f, regions) in batch.iter().zip(applied.regions.iter()) {
            let v = verdicts.get_mut(&f.id()).unwrap();
            let mine: Vec<&Diag> = diags.iter().filter(|d| in_regions(d, regions)).collect();
            if !mine.is_empty() {
                for d in mine {
                    v.compile_in
                        .entry(d.code.clone())
                        .or_insert_with(|| format!("{}:{}: {}", d.file, d.line, d.message));
                }
                v.compiled = true;
                decided += 1;
            } else if diags.is_empty() {
                v.compiled = true;
                decided += 1;
            } else if batch.len() == 1 {
                // Alone, and errors only elsewhere: the mutation broke
                // other code (or cargo itself failed).
                v.compiled = true;
                v.compile_elsewhere = diags.len();
                v.compile_in.retain(|_, _| false);
                decided += 1;
            } else {
                undecided.push(f);
            }
        }
        // Fixtures with an in-region error are out of the next batch, so
        // it can reach the crates (and the lints) this one could not.
        individually = decided == 0;
        pending = undecided.into_iter().chain(rest).collect();
    }
}

// ---------------------------------------------------------------------
// The sweep
// ---------------------------------------------------------------------

fn needs_compile(f: &Fixture, v: &Verdict) -> bool {
    if v.broken.is_some() || !compiles_something(f) {
        return false;
    }
    if survey() || f.accept {
        return true;
    }
    v.rejected_by(&f.by).is_none() && f.by.iter().any(|k| matches!(k, Kind::Compile(_)))
}

fn sweep(work: &Path, fs: &[Fixture]) -> HashMap<String, Verdict> {
    let mut verdicts: HashMap<String, Verdict> = HashMap::new();
    for f in fs {
        if f.retired.is_some() {
            verdicts.insert(f.id(), Verdict::default());
            continue;
        }
        verdicts.insert(f.id(), audit_verdict(work, f));
    }
    let to_compile: Vec<&Fixture> = fs
        .iter()
        .filter(|f| f.retired.is_none() && needs_compile(f, &verdicts[&f.id()]))
        .collect();
    compile_verdicts(work, &to_compile, &mut verdicts);
    verdicts
}

fn describe(v: &Verdict) -> String {
    if let Some(b) = &v.broken {
        return format!("BROKEN ({b})");
    }
    let mut parts: Vec<String> = v.audits.keys().map(|a| format!("audit:{a}")).collect();
    parts.extend(v.compile_in.keys().map(|c| format!("compile:{c}")));
    if v.compile_elsewhere > 0 {
        parts.push(format!(
            "({} errors outside its lines)",
            v.compile_elsewhere
        ));
    }
    if !v.compiled {
        parts.push("(not compiled)".into());
    }
    if parts.is_empty() {
        "accepted".into()
    } else {
        parts.join(", ")
    }
}

/// Every fixture's verdict, computed once per test binary (both tests
/// need them: the operators are held to what rejects each seed).
fn fixture_verdicts() -> &'static (Vec<Fixture>, HashMap<String, Verdict>) {
    static CELL: std::sync::OnceLock<(Vec<Fixture>, HashMap<String, Verdict>)> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let all = fixtures();
        let work = fresh_copy("fixtures");
        // The unmutated copy passes every audit: any failure below is the
        // mutation's.
        let baseline = run_audits(&work);
        assert!(
            baseline.is_empty(),
            "the unmutated tree fails audits: {baseline:?}"
        );
        let verdicts = sweep(&work, &all);
        (all, verdicts)
    })
}

#[test]
#[ignore = "heavy harness: scripts/check-full.sh runs it once, with --ignored"]
fn every_mutation_is_rejected_by_its_intended_mechanism_and_legitimate_shapes_are_accepted() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (all, verdicts) = fixture_verdicts();

    let mut problems: Vec<String> = Vec::new();
    let mut by_source: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut accepted = 0;
    let mut retired: Vec<String> = Vec::new();
    for f in all {
        let v = &verdicts[&f.id()];
        if survey() {
            println!(
                "{:<70} {:<7} by[{}] => {}",
                f.id(),
                if f.accept { "accept" } else { "reject" },
                f.by.iter()
                    .map(Kind::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                describe(v)
            );
            for (c, why) in &v.compile_in {
                println!("      {c}: {why}");
            }
            for (a, why) in &v.audits {
                println!("      {a}: {why}");
            }
        }
        let source = f.source.split(',').next().unwrap_or("").trim().to_string();
        if let Some(why) = &f.retired {
            retired.push(format!("{}: {why}", f.id()));
            if !f.source.starts_with("corpus") {
                problems.push(format!(
                    "{}: a {} mutation cannot be retired -- every review sweep mutation must fail \
                     compilation or a gate audit",
                    f.id(),
                    f.source
                ));
            }
            continue;
        }
        let e = by_source.entry(source).or_default();
        e.0 += 1;
        if f.accept {
            if v.clean() {
                accepted += 1;
                e.1 += 1;
            } else {
                problems.push(format!(
                    "{}: a legitimate shape was rejected: {}",
                    f.id(),
                    describe(v)
                ));
            }
            continue;
        }
        if f.by.is_empty() {
            problems.push(format!("{}: names no `//! by:` rejection kind", f.id()));
            continue;
        }
        match v.rejected_by(&f.by) {
            Some(k) => {
                e.1 += 1;
                *by_kind.entry(k.to_string()).or_default() += 1;
            }
            None => problems.push(format!(
                "{}: not rejected by {} -- got {}",
                f.id(),
                f.by.iter()
                    .map(Kind::to_string)
                    .collect::<Vec<_>>()
                    .join(" or "),
                describe(v)
            )),
        }
    }
    println!("fixtures by source (total, as intended):");
    for (s, (n, ok)) in &by_source {
        println!("  {s:<80} {ok}/{n}");
    }
    println!("rejections by mechanism:");
    for (k, n) in &by_kind {
        println!("  {k:<60} {n}");
    }
    println!("legitimate shapes accepted: {accepted}");
    println!("retired corpus fixtures ({}):", retired.len());
    for r in &retired {
        println!("  {r}");
    }
    if survey() {
        return;
    }
    let accepts = all.iter().filter(|f| f.accept).count();
    assert!(
        accepts >= 20,
        "only {accepts} accept fixtures; the precision side needs at least 20"
    );
    assert!(
        problems.is_empty(),
        "{} fixture(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
    // Re-review 4's sweep, by group: every one rejected.
    //
    // 2026-09-23: M is 42 (not 46), W is 6 (not 12), U is 3 (not 7) --
    // fixtures naming `authority.rs`/`recheck.rs` (four M, six W, four U)
    // are deleted with the rest of the CLI/agent action path ("swamp
    // reports; the human removes"). They are gone, not retired-in-place,
    // because the mechanism they tested no longer exists to have a blind
    // spot in.
    for (group, want) in [("M", 42), ("W", 6), ("U", 3)] {
        let n = all
            .iter()
            .filter(|f| {
                f.source
                    .starts_with(&format!("review-4 sweep ({group} group"))
            })
            .count();
        assert_eq!(
            n, want,
            "review-4 {group} group: {n} fixtures, expected {want}"
        );
    }
}

// ---------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------

/// The product crates' dependency names: a path starting with one is
/// absolute from any module.
fn dependency_crates() -> Vec<String> {
    let mut out = Vec::new();
    for k in ["core", "cli", "tui"] {
        let text = std::fs::read_to_string(repo_root().join(format!("crates/{k}/Cargo.toml")))
            .unwrap_or_default();
        let mut on = false;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                on = t == "[dependencies]";
                continue;
            }
            if on && let Some((name, _)) = t.split_once(['=', '.']) {
                out.push(name.trim().replace('-', "_"));
            }
        }
    }
    out
}

fn renameable(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.len() >= 2
        && segs
            .last()
            .is_some_and(|l| l.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
        && !matches!(
            segs[0].as_str(),
            "Self" | "Some" | "Ok" | "Err" | "Box" | "Vec" | "String"
        )
        && !segs[segs.len() - 2]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        && path.segments.iter().all(|s| s.arguments.is_none())
}

fn first_call(block: &syn::Block) -> Option<syn::Path> {
    struct F(Option<syn::Path>);
    impl syn::visit::Visit<'_> for F {
        fn visit_expr_call(&mut self, c: &syn::ExprCall) {
            if self.0.is_none()
                && let syn::Expr::Path(p) = &*c.func
                && p.qself.is_none()
                && renameable(&p.path)
            {
                self.0 = Some(p.path.clone());
            }
            syn::visit::visit_expr_call(self, c);
        }
    }
    let mut f = F(None);
    syn::visit::visit_block(&mut f, block);
    f.0
}

struct Retarget {
    from: String,
    to: syn::Path,
}
impl VisitMut for Retarget {
    fn visit_expr_call_mut(&mut self, c: &mut syn::ExprCall) {
        if let syn::Expr::Path(p) = &mut *c.func
            && p.path.to_token_stream().to_string() == self.from
        {
            p.path = self.to.clone();
        }
        syn::visit_mut::visit_expr_call_mut(self, c);
    }
}

fn each_scope(
    items: &mut Vec<syn::Item>,
    edit: &mut dyn FnMut(&mut Vec<syn::Item>) -> bool,
) -> bool {
    let mut changed = edit(items);
    for it in items.iter_mut() {
        if let syn::Item::Mod(m) = it
            && let Some((_, inner)) = &mut m.content
        {
            changed |= each_scope(inner, edit);
        }
    }
    changed
}

fn fns_in(items: &mut [syn::Item]) -> Vec<&mut syn::ItemFn> {
    items
        .iter_mut()
        .filter_map(|i| match i {
            syn::Item::Fn(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn op_alias(file: &mut syn::File, via_shim: bool) -> bool {
    let deps = dependency_crates();
    let mut done = false;
    each_scope(&mut file.items, &mut |items| {
        if done {
            return false;
        }
        let mut target: Option<syn::Path> = None;
        for f in fns_in(items) {
            if let Some(p) = first_call(&f.block) {
                target = Some(p);
                break;
            }
        }
        let Some(path) = target else { return false };
        let from = path.to_token_stream().to_string();
        let (to, added): (syn::Path, syn::Item) = if via_shim {
            let written = from.replace(' ', "");
            let first = written.split("::").next().unwrap_or("").to_string();
            let inner = match first.as_str() {
                _ if deps.contains(&first) => written.clone(),
                "crate" | "std" | "swamp_core" | "swamp_tui" => written.clone(),
                "self" => written.replacen("self", "super", 1),
                _ => format!("super::{written}"),
            };
            let inner: syn::Path = syn::parse_str(&inner).unwrap();
            (
                syn::parse_quote!(sweep_op_shim::sweep_op_g),
                syn::parse_quote!(mod sweep_op_shim { pub use #inner as sweep_op_g; }),
            )
        } else {
            (
                syn::parse_quote!(sweep_op_alias),
                syn::parse_quote!(use #path as sweep_op_alias;),
            )
        };
        let mut r = Retarget { from, to };
        for f in fns_in(items) {
            r.visit_block_mut(&mut f.block);
        }
        items.insert(0, added);
        done = true;
        true
    })
}

fn op_helper(file: &mut syn::File) -> bool {
    each_scope(&mut file.items, &mut |items| {
        let mut added: Vec<syn::Item> = Vec::new();
        for f in fns_in(items) {
            let simple = f.sig.inputs.iter().all(
                |a| matches!(a, syn::FnArg::Typed(t) if matches!(&*t.pat, syn::Pat::Ident(_))),
            );
            if !simple || f.block.stmts.is_empty() || f.sig.asyncness.is_some() {
                continue;
            }
            let helper = syn::Ident::new(
                &format!("sweep_op_helper_{}", f.sig.ident),
                f.sig.ident.span(),
            );
            let mut sig = f.sig.clone();
            sig.ident = helper.clone();
            added.push(syn::Item::Fn(syn::ItemFn {
                attrs: Vec::new(),
                vis: syn::Visibility::Inherited,
                sig,
                block: f.block.clone(),
            }));
            let args: Vec<syn::Ident> = f
                .sig
                .inputs
                .iter()
                .filter_map(|a| match a {
                    syn::FnArg::Typed(t) => match &*t.pat {
                        syn::Pat::Ident(i) => Some(i.ident.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            *f.block = syn::parse_quote!({ #helper(#(#args),*) });
        }
        let changed = !added.is_empty();
        items.extend(added);
        changed
    })
}

fn op_macro_wrap(file: &mut syn::File) -> bool {
    struct W(bool);
    impl VisitMut for W {
        fn visit_block_mut(&mut self, b: &mut syn::Block) {
            for s in b.stmts.iter_mut() {
                let replacement: Option<syn::Stmt> = match s {
                    syn::Stmt::Expr(
                        e @ (syn::Expr::Call(_) | syn::Expr::MethodCall(_) | syn::Expr::Try(_)),
                        Some(_),
                    ) => Some(syn::parse_quote!(let _ = vec![#e];)),
                    syn::Stmt::Local(l)
                        if matches!(l.pat, syn::Pat::Wild(_))
                            && l.init.as_ref().is_some_and(|i| i.diverge.is_none()) =>
                    {
                        let e = &l.init.as_ref().unwrap().expr;
                        Some(syn::parse_quote!(let _ = vec![#e];))
                    }
                    _ => None,
                };
                if let Some(r) = replacement {
                    *s = r;
                    self.0 = true;
                }
            }
            syn::visit_mut::visit_block_mut(self, b);
        }
    }
    let mut w = W(false);
    w.visit_file_mut(file);
    w.0
}

fn op_via_constant(file: &mut syn::File) -> bool {
    struct H {
        n: usize,
        consts: Vec<(syn::Ident, syn::LitStr)>,
    }
    impl VisitMut for H {
        fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
            if let syn::Expr::Lit(l) = e
                && let syn::Lit::Str(s) = &l.lit
            {
                let id = syn::Ident::new(&format!("SWEEP_OP_C{}", self.n), s.span());
                self.n += 1;
                self.consts.push((id.clone(), s.clone()));
                *e = syn::parse_quote!(#id);
                return;
            }
            // Format strings and macro arguments are tokens, not
            // expressions: `format!` needs its literal in place.
            if matches!(e, syn::Expr::Macro(_)) {
                return;
            }
            syn::visit_mut::visit_expr_mut(self, e);
        }
    }
    let mut n = 0usize;
    each_scope(&mut file.items, &mut |items| {
        let mut h = H {
            n,
            consts: Vec::new(),
        };
        for f in fns_in(items) {
            h.visit_block_mut(&mut f.block);
        }
        n = h.n;
        let changed = !h.consts.is_empty();
        for (id, lit) in h.consts {
            items.insert(0, syn::parse_quote!(const #id: &str = #lit;));
        }
        changed
    })
}

/// Re-review 4: every call `P(args)` with a multi-segment path callee
/// becomes `{ let sweep4_fb = P; sweep4_fb(args) }`.
fn op_fn_item_binding(file: &mut syn::File) -> bool {
    struct B(bool);
    impl VisitMut for B {
        fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
            syn::visit_mut::visit_expr_mut(self, e);
            if let syn::Expr::Call(c) = e
                && let syn::Expr::Path(p) = &*c.func
                && p.qself.is_none()
                && p.path.segments.len() >= 2
                && p.path.segments.iter().all(|s| s.arguments.is_none())
                && p.path.segments.last().is_some_and(|l| {
                    l.ident
                        .to_string()
                        .starts_with(|c: char| c.is_ascii_lowercase())
                })
            {
                let path = p.path.clone();
                let args = c.args.clone();
                *e = syn::parse_quote!({ let sweep4_fb = #path; sweep4_fb(#args) });
                self.0 = true;
            }
        }
    }
    let mut b = B(false);
    b.visit_file_mut(file);
    b.0
}

/// Re-review 4: every item gets a doc line that mentions `cfg(test)`.
fn op_doc_cfg_test(file: &mut syn::File) -> bool {
    fn doc(attrs: &mut Vec<syn::Attribute>) {
        attrs.push(syn::parse_quote!(#[doc = " Production code: not behind `#[cfg(test)]`."]));
    }
    fn items(list: &mut [syn::Item]) -> bool {
        let mut changed = false;
        for it in list.iter_mut() {
            match it {
                syn::Item::Fn(f) => doc(&mut f.attrs),
                syn::Item::Impl(i) => doc(&mut i.attrs),
                syn::Item::Struct(s) => doc(&mut s.attrs),
                syn::Item::Const(c) => doc(&mut c.attrs),
                syn::Item::Static(s) => doc(&mut s.attrs),
                syn::Item::Mod(m) => {
                    doc(&mut m.attrs);
                    if let Some((_, inner)) = &mut m.content {
                        items(inner);
                    }
                }
                _ => continue,
            }
            changed = true;
        }
        changed
    }
    items(&mut file.items)
}

const REJECT_OPERATORS: &[&str] = &[
    "alias",
    "pub_use",
    "helper",
    "child_module",
    "macro_wrap",
    "via_constant",
    "fn_item_binding",
    "doc_cfg_test",
];

/// Spelling changes that must keep a legitimate shape legitimate.
const ACCEPT_OPERATORS: &[&str] = &[
    "alias",
    "helper",
    "via_constant",
    "fn_item_binding",
    "doc_cfg_test",
];

fn variant(seed: &Fixture, op: &str) -> Option<Fixture> {
    let first = seed.files.first()?;
    if !first.target.ends_with(".rs") || crate_of(&first.target).is_none() {
        return None;
    }
    let mut out = seed.clone();
    out.name = format!("{} [{op}]", seed.name);
    if op == "child_module" {
        if first.mode != Mode::Append || seed.files.len() != 1 {
            return None;
        }
        let (dir, stem) = first.target.rsplit_once('/')?;
        let stem = stem.trim_end_matches(".rs");
        let child = if ["mod", "lib", "main"].contains(&stem) {
            format!("{dir}/sweep_op_moved.rs")
        } else {
            format!("{dir}/{stem}/sweep_op_moved.rs")
        };
        out.files = vec![
            Section {
                target: first.target.clone(),
                mode: Mode::Append,
                body: "pub mod sweep_op_moved;\n".into(),
            },
            Section {
                target: child,
                mode: Mode::Create,
                // One level down: an explicit `super::` path now needs one
                // more hop to mean what it meant.
                body: format!(
                    "#[allow(unused_imports)]\nuse super::*;\n{}",
                    first.body.replace("super::", "super::super::")
                ),
            },
        ];
        return Some(out);
    }
    if matches!(first.mode, Mode::Substitute(_)) {
        return None;
    }
    let mut parsed: syn::File = syn::parse_str(&first.body).ok()?;
    let changed = match op {
        "alias" => op_alias(&mut parsed, false),
        "pub_use" => op_alias(&mut parsed, true),
        "helper" => op_helper(&mut parsed),
        "macro_wrap" => op_macro_wrap(&mut parsed),
        "via_constant" => op_via_constant(&mut parsed),
        "fn_item_binding" => op_fn_item_binding(&mut parsed),
        "doc_cfg_test" => op_doc_cfg_test(&mut parsed),
        _ => false,
    };
    if !changed {
        return None;
    }
    out.files[0].body = quote!(#parsed).to_string();
    Some(out)
}

/// The names a fixture body defines at item level (and inherent
/// methods), so several variants of one seed can share a compile.
fn defined_names(body: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(file) = syn::parse_file(body) else {
        return out;
    };
    fn walk(items: &[syn::Item], out: &mut HashSet<String>) {
        for it in items {
            match it {
                syn::Item::Fn(f) => {
                    out.insert(f.sig.ident.to_string());
                }
                syn::Item::Struct(s) => {
                    out.insert(s.ident.to_string());
                }
                syn::Item::Enum(e) => {
                    out.insert(e.ident.to_string());
                }
                syn::Item::Const(c) => {
                    out.insert(c.ident.to_string());
                }
                syn::Item::Static(s) => {
                    out.insert(s.ident.to_string());
                }
                syn::Item::Trait(t) => {
                    out.insert(t.ident.to_string());
                }
                syn::Item::Type(t) => {
                    out.insert(t.ident.to_string());
                }
                syn::Item::Macro(m) => {
                    if let Some(i) = &m.ident {
                        out.insert(i.to_string());
                    }
                }
                // A file module's name is its file's name: renaming it
                // would orphan the file. Inline modules are renamed.
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        out.insert(m.ident.to_string());
                        walk(inner, out);
                    }
                }
                syn::Item::Use(u) => {
                    fn renames(t: &syn::UseTree, out: &mut HashSet<String>) {
                        match t {
                            syn::UseTree::Path(p) => renames(&p.tree, out),
                            syn::UseTree::Rename(r) => {
                                out.insert(r.rename.to_string());
                            }
                            syn::UseTree::Group(g) => g.items.iter().for_each(|i| renames(i, out)),
                            _ => {}
                        }
                    }
                    renames(&u.tree, out);
                }
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    for ii in &i.items {
                        match ii {
                            syn::ImplItem::Fn(f) => {
                                out.insert(f.sig.ident.to_string());
                            }
                            syn::ImplItem::Const(c) => {
                                out.insert(c.ident.to_string());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    walk(&file.items, &mut out);
    out
}

/// Suffixes every name the variant defines (and every use of it), token
/// by token, so it can sit in one file beside its siblings.
fn suffixed(f: &Fixture, suffix: &str) -> Fixture {
    let mut names: HashSet<String> = HashSet::new();
    for s in &f.files {
        names.extend(defined_names(&s.body));
    }
    // Only the fixture's own helper names (the `sweep`-prefixed
    // convention): a fixture that defines an item named like a real one is
    // *about* that name, and renaming it would change the mutation.
    names.retain(|n| n.to_ascii_lowercase().starts_with("sweep"));
    // A child module's own name is its file name; leave modules alone.
    names.remove("sweep_op_moved");
    let mut out = f.clone();
    for s in out.files.iter_mut() {
        if !s.target.ends_with(".rs") || matches!(s.mode, Mode::Substitute(_)) {
            continue;
        }
        let Ok(ts) = s.body.parse::<proc_macro2::TokenStream>() else {
            continue;
        };
        fn rename(
            ts: proc_macro2::TokenStream,
            names: &HashSet<String>,
            suffix: &str,
        ) -> proc_macro2::TokenStream {
            ts.into_iter()
                .map(|t| match t {
                    // The tag goes right after the `sweep` prefix, so a
                    // meaningful suffix (`SWEEP_TOOL_ID` is an adapter's
                    // `*_TOOL_ID`) keeps its meaning.
                    proc_macro2::TokenTree::Ident(i) if names.contains(&i.to_string()) => {
                        let text = i.to_string();
                        let (head, tail) = text.split_at(5);
                        let tag = if head == "SWEEP" {
                            suffix.to_ascii_uppercase()
                        } else {
                            suffix.to_string()
                        };
                        proc_macro2::TokenTree::Ident(proc_macro2::Ident::new(
                            &format!("{head}{tag}{tail}"),
                            i.span(),
                        ))
                    }
                    proc_macro2::TokenTree::Group(g) => {
                        let mut ng = proc_macro2::Group::new(
                            g.delimiter(),
                            rename(g.stream(), names, suffix),
                        );
                        ng.set_span(g.span());
                        proc_macro2::TokenTree::Group(ng)
                    }
                    other => other,
                })
                .collect()
        }
        s.body = rename(ts, &names, suffix).to_string();
    }
    out
}

#[test]
#[ignore = "heavy harness: scripts/check-full.sh runs it once, with --ignored"]
fn mutation_operators_keep_the_rejection_kind() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (all, seed_verdicts) = fixture_verdicts();
    let work = fresh_copy("operators");
    // Seeds: a variant is held to the kinds that actually reject its seed
    // (within the seed's own `by:` list).
    let seeds: Vec<&Fixture> = all
        .iter()
        .filter(|f| f.retired.is_none())
        .filter(|f| {
            f.files
                .first()
                .is_some_and(|s| crate_of(&s.target).is_some())
        })
        .collect();

    let mut variants: Vec<(Fixture, Vec<Kind>, bool)> = Vec::new();
    let mut per_op: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (si, seed) in seeds.iter().enumerate() {
        let v = &seed_verdicts[&seed.id()];
        let (ops, wanted): (&[&str], Vec<Kind>) = if seed.accept {
            if !v.clean() {
                continue;
            }
            (ACCEPT_OPERATORS, Vec::new())
        } else {
            let fired: Vec<Kind> = seed
                .by
                .iter()
                .filter(|k| v.kinds().contains(k))
                .cloned()
                .collect();
            if fired.is_empty() {
                // The seed itself is not rejected as intended; the fixture
                // test reports that.
                continue;
            }
            (REJECT_OPERATORS, seed.by.clone())
        };
        for (oi, op) in ops.iter().enumerate() {
            if let Some(var) = variant(seed, op) {
                let var = suffixed(&var, &format!("v{si}o{oi}"));
                per_op.entry(op).or_default().0 += 1;
                variants.push((var, wanted.clone(), seed.accept));
            }
        }
    }
    let owned: Vec<Fixture> = variants.iter().map(|(f, _, _)| f.clone()).collect();
    let verdicts = sweep(&work, &owned);
    let mut problems = Vec::new();
    for (f, wanted, accept) in &variants {
        let v = &verdicts[&f.id()];
        let op = f
            .name
            .rsplit_once('[')
            .map(|(_, o)| o.trim_end_matches(']'))
            .unwrap_or("");
        let ok = if *accept {
            v.clean()
        } else {
            v.rejected_like(wanted).is_some()
        };
        if ok {
            per_op.get_mut(op).unwrap().1 += 1;
        } else {
            problems.push(format!(
                "{}: {} -- got {}",
                f.id(),
                if *accept {
                    "a legitimate shape was rejected".to_string()
                } else {
                    format!(
                        "not rejected by {}",
                        wanted
                            .iter()
                            .map(Kind::to_string)
                            .collect::<Vec<_>>()
                            .join(" or ")
                    )
                },
                describe(v)
            ));
        }
    }
    println!("operator variants (generated, held):");
    for (op, (n, ok)) in &per_op {
        println!("  {op:<16} {ok}/{n}");
    }
    if survey() {
        for p in &problems {
            println!("  {p}");
        }
        return;
    }
    assert!(
        problems.is_empty(),
        "{} variant(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
