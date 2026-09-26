//! What delivered strings may say. Exact token rules over every
//! production string literal (including `format!`/`concat!` arguments,
//! constants and `#[serde(rename)]` strings) and every serialized field
//! or variant name.
//!
//! * `no_verdict_literals` -- facts, not verdicts
//!   (`.oh/guardrails/agent-interface-facts-not-verdicts.md`): no
//!   delivered string or serialized key asserts "safe to delete",
//!   "unused", "stale"... unless the same clause negates it ("swamp never
//!   labels a directory unused").
//! * `byte_units_only_in_the_formatter` -- one byte formatter
//!   (`.oh/guardrails/one-byte-formatter.md`): a formatted quantity with a
//!   binary or decimal unit (`"{v:.1} KiB"`) is rendered only by
//!   `render.rs`.
//! * `ids_only_in_their_module` -- adapters and detectors are pluggable
//!   (`agent-adapters-are-pluggable`, `detector-ids-only-in-registry`): a
//!   tool or detector id literal appears only in its own module and the
//!   registries, never in a central `if id == "cline"` chain or wiring
//!   table.

use super::{Rule, verdict};
use crate::model::{Krate, Workspace};

/// Phrases and words that assert a verdict about a path.
const VERDICT_PHRASES: &[&str] = &[
    "safe to delete",
    "safe to remove",
    "safe to clean",
    "can be deleted",
    "can be removed",
    "should delete",
    "should be deleted",
    "should remove",
    "should be removed",
];
const VERDICT_WORDS: &[&str] = &["stale", "unused", "obsolete", "junk", "garbage"];

/// Words that negate what follows in the same clause.
const NEGATORS: &[&str] = &[
    "never", "not", "no", "none", "nothing", "without", "nor", "cannot", "neither",
];

/// Serialized names containing a verdict word that are reviewed facts
/// about a *measurement*, not about a path: `dedup_stale` says the unique
/// byte figure was not recomputed this pass.
const REVIEWED_SERIALIZED: &[&str] = &["dedup_stale"];

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// The clause of `text` that ends at byte `at` (from the previous
/// sentence/clause boundary).
fn clause_before(text: &str, at: usize) -> &str {
    let head = &text[..at];
    let cut = head
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, '.' | ';' | ':' | '!' | '?' | '(' | ')' | '\n' | '—'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let clause = &head[cut..];
    match clause.rfind(" -- ") {
        Some(i) => &clause[i + 4..],
        None => clause,
    }
}

fn negated(text: &str, at: usize) -> bool {
    let clause = clause_before(text, at);
    words(clause)
        .iter()
        .any(|w| NEGATORS.contains(&w.as_str()) || w.ends_with("n't"))
}

/// The verdict a piece of prose asserts, if any.
pub fn verdict_in(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for p in VERDICT_PHRASES {
        let mut from = 0;
        while let Some(i) = lower[from..].find(p) {
            let at = from + i;
            if !negated(&lower, at) {
                return Some((*p).to_string());
            }
            from = at + p.len();
        }
    }
    // Whole words only (`unusedness` is not `unused`, but `unused` and
    // `stale.` are).
    let mut idx = 0;
    for w in lower.split(|c: char| !c.is_alphanumeric()) {
        let at = idx;
        idx += w.len() + 1;
        if VERDICT_WORDS.contains(&w) && !negated(&lower, at.min(lower.len())) {
            return Some(w.to_string());
        }
    }
    if lower.trim() == "safe" {
        return Some("safe".to_string());
    }
    None
}

/// A format string with its `{..}` placeholders blanked: `{stale}` names
/// a local, it is not text anyone reads (`{{` stays a literal brace).
fn without_placeholders(lit: &str) -> String {
    let mut out = String::new();
    let mut chars = lit.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                out.push('{');
                continue;
            }
            for d in chars.by_ref() {
                if d == '}' {
                    break;
                }
            }
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

/// snake_case / CamelCase identifier → words.
fn ident_words(ident: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in ident.chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase();
        out.extend(c.to_lowercase());
    }
    out
}

pub fn no_verdict_literals(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for m in &ws.modules {
        for l in &m.literals {
            if l.test || REVIEWED_SERIALIZED.contains(&l.value.as_str()) {
                continue;
            }
            if let Some(v) = verdict_in(&without_placeholders(&l.value)) {
                problems.push(format!(
                    "{}: \"{}\" asserts the verdict \"{v}\": facts, not verdicts \
                     (agent-interface-facts-not-verdicts)",
                    l.site,
                    l.value.replace('\n', " ")
                ));
            }
        }
        for s in &m.serialized {
            if s.test || REVIEWED_SERIALIZED.contains(&s.name.as_str()) {
                continue;
            }
            let as_prose = ident_words(&s.name);
            if let Some(v) = verdict_in(&as_prose) {
                problems.push(format!(
                    "{}: `{}::{}` is serialized under a name that asserts the verdict \"{v}\": a \
                     key an agent reads is delivered text too",
                    s.site, s.owner, s.name
                ));
            }
        }
    }
    problems
}

const UNITS: &[&str] = &[
    "KB", "MB", "GB", "TB", "PB", "KiB", "MiB", "GiB", "TiB", "PiB",
];

/// Whether a literal renders a formatted quantity with a size unit: a
/// placeholder (`{..}`) followed by optional spaces and a unit token.
fn renders_size(lit: &str) -> Option<&'static str> {
    let b = lit.as_bytes();
    for (i, c) in lit.char_indices() {
        if c != '}' || (i > 0 && b[i - 1] == b'}') {
            continue;
        }
        let rest = lit[i + 1..].trim_start_matches(' ');
        for u in UNITS {
            if let Some(after) = rest.strip_prefix(u)
                && !after.starts_with(|ch: char| ch.is_alphanumeric())
            {
                return Some(u);
            }
        }
    }
    None
}

/// Modules that *parse* sizes written with units (Docker's CLI output,
/// the filter and budget flags). Reviewed: they read units, never render
/// them.
const UNIT_PARSERS: &[&[&str]] = &[&["docker"], &["filter"]];

pub fn byte_units_only_in_the_formatter(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for m in &ws.modules {
        if m.is_within(Krate::Core, &["render"]) {
            continue;
        }
        let parser = UNIT_PARSERS.iter().any(|p| m.is_within(Krate::Core, p));
        for l in &m.literals {
            if l.test {
                continue;
            }
            // A bare unit label (a unit table, a label chosen at run time)
            // is a formatter's vocabulary.
            if !parser && (UNITS.contains(&l.value.as_str()) || l.value == "kB") {
                problems.push(format!(
                    "{}: the unit label \"{}\" outside render.rs (and the reviewed unit parsers): \
                     a table of byte units is a second formatter (one-byte-formatter)",
                    l.site, l.value
                ));
                continue;
            }
            if let Some(u) = renders_size(&l.value) {
                problems.push(format!(
                    "{}: \"{}\" formats a quantity in {u} outside render.rs: every size a person \
                     reads goes through `render::human_bytes*` (one-byte-formatter)",
                    l.site, l.value
                ));
            }
        }
    }
    problems
}

/// Detector ids that are also the everyday name of a package ecosystem,
/// a toolchain manager or a directory (`"npm"` is the dependency
/// ecosystem tag of a `package-lock.json` identity and pi's `npm/` cache
/// directory as well as the npm-cache detector's id). Reviewed: these
/// strings in other modules name the ecosystem, not the detector. Any id
/// not listed here -- every tool id, and `cargo-home`, `docker-desktop`,
/// `homebrew`... -- is still owned by its module alone.
const SHARED_VOCABULARY: &[&str] = &[
    "npm", "pnpm", "go", "maven", "gradle", "mise", "pyenv", "nvm", "rustup",
    // `pyvenv.cfg`'s own field name (`uv = <version>`), read as a property
    // key by the build-adapter's Python environment parser -- the same
    // string as the uv detector's id, but naming the config file's field,
    // never the detector.
    "uv",
];

pub fn ids_only_in_their_module(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    // Every id, with the module that owns it.
    let mut ids: Vec<(String, Vec<String>, &'static str)> = Vec::new();
    for m in &ws.modules {
        if m.test || m.krate != Krate::Core {
            continue;
        }
        let family = match m.path.first().map(String::as_str) {
            Some("agents") if m.path.len() == 2 => "tool",
            Some("locations") if m.path.len() == 2 => "detector",
            _ => continue,
        };
        for (name, value, _) in &m.str_consts {
            let is_id = (family == "tool" && name.ends_with("_TOOL_ID"))
                || (family == "detector" && name.ends_with("_DETECTOR_ID"));
            if is_id {
                ids.push((value.clone(), m.path.clone(), family));
            }
        }
    }
    // Id constants, by name: a path to another module's `*_TOOL_ID` is
    // the same central dispatch as its literal.
    for (mi, m) in ws.modules.iter().enumerate() {
        let registry = m.is_within(Krate::Core, &["agents", "registry"])
            || m.is_within(Krate::Core, &["agents", "matrix"])
            || m.is_within(Krate::Core, &["build_adapters", "registry"])
            || m.is_within(Krate::Core, &["build_adapters", "matrix"])
            || m.is_within(Krate::Core, &["locations"]) && m.path.len() == 1;
        for r in &m.refs {
            if r.test {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments);
            let Some(last) = abs.last() else { continue };
            let (family, reviewed): (&str, &[&[&str]]) = if last.ends_with("_TOOL_ID") {
                ("tool", &[])
            } else if last.ends_with("_DETECTOR_ID") {
                // Reviewed wiring that names detector ids by constant: scope
                // interpretation and the permitted-detector set.
                ("detector", &[&["scope"], &["locations", "permitted"]])
            } else {
                continue;
            };
            if abs.len() < 4 || abs[0] != "@core" {
                continue;
            }
            let owner: Vec<&str> = abs[1..abs.len() - 1].iter().map(String::as_str).collect();
            let own = m.krate == Krate::Core
                && m.path
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .starts_with(&owner);
            // A build adapter's own module (`build_adapters::<x>`) may name
            // the id constant of its same-named detector (`locations::<x>`):
            // the same twin relationship a tool adapter has with its
            // detector (`every_adapter_id_is_also_a_detector_id`), one
            // family over.
            let build_twin = m.krate == Krate::Core
                && m.path.len() == 2
                && m.path.first().map(String::as_str) == Some("build_adapters")
                && m.path.last().map(String::as_str) == owner.last().copied();
            let twin = m.krate == Krate::Core
                && ids.iter().any(|(_, o2, _)| {
                    m.path.starts_with(o2) && o2.last().map(String::as_str) == owner.last().copied()
                });
            if !own
                && !twin
                && !build_twin
                && !registry
                && !reviewed.iter().any(|p| m.is_within(Krate::Core, p))
            {
                problems.push(format!(
                    "{}: {} names the {family} id constant `{}`: an id is used only in its own \
                     module and the registries, so adding a {family} never means editing a \
                     central list or chain",
                    r.site,
                    m.display(),
                    r.segments.join("::")
                ));
            }
        }
    }
    for m in &ws.modules {
        let registry = m.is_within(Krate::Core, &["agents", "registry"])
            || m.is_within(Krate::Core, &["agents", "matrix"])
            || m.is_within(Krate::Core, &["build_adapters", "registry"])
            || m.is_within(Krate::Core, &["build_adapters", "matrix"])
            || m.is_within(Krate::Core, &["locations"]) && m.path.len() == 1;
        for l in &m.literals {
            if l.test {
                continue;
            }
            for (id, owner, family) in &ids {
                if &l.value != id || SHARED_VOCABULARY.contains(&id.as_str()) {
                    continue;
                }
                let own = m.krate == Krate::Core && m.path.starts_with(owner);
                // A tool id may also be its own detector's id (the pinned
                // equality `every_adapter_id_is_also_a_detector_id`), and a
                // build adapter's own module may equally be its same-named
                // detector's twin (`build_adapters::<x>` / `locations::<x>`).
                let twin = m.krate == Krate::Core
                    && ids
                        .iter()
                        .any(|(i2, o2, _)| i2 == id && m.path.starts_with(o2));
                let build_twin = m.krate == Krate::Core
                    && m.path.len() == 2
                    && m.path.first().map(String::as_str) == Some("build_adapters")
                    && m.path.last().map(String::as_str) == owner.last().map(String::as_str);
                if !own && !twin && !build_twin && !registry {
                    problems.push(format!(
                        "{}: the {family} id \"{id}\" is written in {} instead of only in \
                         `{}` and the registry: adding a {family} must never mean editing a \
                         central list or chain",
                        l.site,
                        m.display(),
                        owner.join("::")
                    ));
                }
            }
        }
    }
    problems
}

pub const RULES: &[Rule] = &[
    ("no_verdict_literals", |ws| {
        verdict(
            "facts, not verdicts: no delivered string or serialized key asserts a verdict",
            no_verdict_literals(ws),
        )
    }),
    ("byte_units_only_in_the_formatter", |ws| {
        verdict("one byte formatter", byte_units_only_in_the_formatter(ws))
    }),
    ("ids_only_in_their_module", |ws| {
        verdict(
            "tool and detector ids appear only in their own module and the registries",
            ids_only_in_their_module(ws),
        )
    }),
];
