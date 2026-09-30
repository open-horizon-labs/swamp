//! mise: `mise -C / uninstall <tool>@<version>` and `mise -C / prune
//! --tools`, each previewed by the same argv plus `--dry-run`.
//!
//! What mise itself does not check, and swamp does (verified 2026-09-30,
//! mise 2026.9.15): `mise uninstall --dry-run` exits 0 for a version the
//! global config requests; bare `mise prune` also prunes tracked config
//! links (hence `--tools`, always); `mise ls --json` answers relative to
//! its cwd (hence `-C /` and a cwd of `/`), so a `source: null` there is
//! never read as "nothing requests it": a per-version removal is offered
//! only for a version mise's own prune reports.

use super::{
    Candidate, Host, Manager, Preview, Refusal, Size, Target, clean_block, digest, expand_home,
    read_ok, tilde,
};
use crate::fs_gate::spawn::{RunOutput, ToolBin, is_mise_tool_version};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Always on a mise confirm: a `MISE_<TOOL>_VERSION` in the user's shell
/// never reaches swamp's child, so mise's prune under swamp cannot see it.
pub(super) const PINNED_BY_ENV: &str =
    "Versions pinned by environment variables in your shell are not visible to swamp.";

/// The mise version the dry-run formats below were read from.
pub(super) const VERIFIED_VERSION: &str = "2026.9.15";

const LS: &[&str] = &["-C", "/", "ls", "--json", "--installed"];
const PRUNE_DRY: &[&str] = &["-C", "/", "prune", "--tools", "--dry-run"];

/// One installed version, as `mise ls --json --installed` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Install {
    tool: String,
    version: String,
    install_path: PathBuf,
    source: Option<PathBuf>,
    requested: Option<String>,
    symlinked_to: Option<PathBuf>,
    active: bool,
}

impl Install {
    fn tv(&self) -> String {
        format!("{}@{}", self.tool, self.version)
    }

    /// Everything about this entry a review decides on: compared again
    /// at Enter.
    fn fact(&self, home: &Path) -> String {
        format!(
            "path {}, requested by {}{}, {}{}",
            tilde(&self.install_path, home),
            self.source
                .as_deref()
                .map_or_else(|| "nothing listed".to_string(), |p| tilde(p, home)),
            self.requested
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default(),
            if self.active { "active" } else { "not active" },
            self.symlinked_to
                .as_deref()
                .map(|p| format!(", symlink to {}", p.display()))
                .unwrap_or_default()
        )
    }
}

pub(super) fn parse_version(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.split_whitespace().next())
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

fn parse_ls(text: &str) -> Result<Vec<Install>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("its JSON did not parse ({e})"))?;
    let obj = v.as_object().ok_or("its JSON is not a map of tools")?;
    let mut out = Vec::new();
    for (tool, entries) in obj {
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("{tool} is not a list"))?;
        for e in entries {
            if e.get("installed").and_then(|i| i.as_bool()) == Some(false) {
                continue;
            }
            let version = e
                .get("version")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("a {tool} entry has no version"))?;
            let install_path = e
                .get("install_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{tool}@{version} has no install_path"))?;
            let tv = format!("{tool}@{version}");
            // A field swamp cannot read is not "absent": an unreadable
            // `source` must never pass the consumer guard.
            let source = match e.get("source") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::Object(o)) => match o.get("path") {
                    Some(serde_json::Value::String(p)) => Some(PathBuf::from(p)),
                    _ => return Err(format!("{tv} has a source without a readable path")),
                },
                Some(_) => return Err(format!("{tv} has a source swamp cannot read")),
            };
            let opt_str = |k: &str| -> Result<Option<String>, String> {
                match e.get(k) {
                    None | Some(serde_json::Value::Null) => Ok(None),
                    Some(serde_json::Value::String(v)) => Ok(Some(v.clone())),
                    Some(_) => Err(format!("{tv} has a {k} swamp cannot read")),
                }
            };
            let active = match e.get("active") {
                None => false,
                Some(serde_json::Value::Bool(b)) => *b,
                Some(_) => return Err(format!("{tv} has an active flag swamp cannot read")),
            };
            out.push(Install {
                tool: tool.clone(),
                version: version.to_string(),
                install_path: PathBuf::from(install_path),
                source,
                requested: opt_str("requested_version")?,
                symlinked_to: opt_str("symlinked_to")?.map(PathBuf::from),
                active,
            });
        }
    }
    out.sort_by(|a, b| a.tool.cmp(&b.tool).then(a.version.cmp(&b.version)));
    Ok(out)
}

fn list_installs(bin: &ToolBin) -> Result<Vec<Install>, Refusal> {
    let out = read_ok(bin, LS, Manager::Mise, "ls")?;
    if !out.success() {
        return Err(Refusal::new(
            format!("mise ls exited {}.", code_text(&out)),
            "Run `mise ls` yourself to see why.",
        )
        .with_output(clean_block(&out.stderr)));
    }
    parse_ls(&out.stdout_lossy()).map_err(|why| {
        Refusal::new(
            format!("mise's list could not be read: {why}."),
            "Check the mise version; swamp runs nothing it cannot read.",
        )
    })
}

fn code_text(out: &RunOutput) -> String {
    out.code
        .map_or_else(|| "on a signal".to_string(), |c| c.to_string())
}

/// Which dry run a text came from: the formats differ, and each must
/// carry its own marker (the real run prints the same lines without it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `mise uninstall --dry-run`: untagged lines, ends
    /// `✓ uninstalled (dry-run)`.
    Uninstall,
    /// `mise prune --tools --dry-run`: every action line tagged
    /// `[dryrun]`, ends `[dryrun] ✓ done`.
    Prune,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DryVersion {
    removes: Vec<PathBuf>,
    done: bool,
}

/// What a mise dry run says it would do: per `<tool>@<version>`, the
/// paths it would remove, and mise's own reason lines (`... is
/// prunable: ...`) verbatim. Anything swamp does not recognize refuses:
/// a format it cannot read is not a preview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Dry {
    versions: BTreeMap<String, DryVersion>,
    reasons: Vec<String>,
}

fn parse_dry(text: &str, form: Form, home: &Path) -> Result<Dry, String> {
    let mut dry = Dry::default();
    let unrecognized = |l: &str| format!("mise printed a line swamp does not recognize: `{l}`");
    for raw in text.lines() {
        let l = super::clean_line(raw);
        let l = l.trim_end();
        if l.trim().is_empty() || l.starts_with("mise WARN") {
            continue;
        }
        if l.contains("ERROR") {
            return Err(format!("mise printed an error: `{l}`"));
        }
        if l.contains("configuration links") {
            return Err(
                "mise's dry run also names configuration links; swamp runs only `mise prune \
                 --tools`"
                    .to_string(),
            );
        }
        let Some(rest) = l.strip_prefix("mise ") else {
            return Err(unrecognized(l));
        };
        let Some((tv, tail)) = rest.split_once(' ') else {
            return Err(unrecognized(l));
        };
        if !is_mise_tool_version(tv) {
            return Err(unrecognized(l));
        }
        let tail = tail.trim_start();
        if tail.starts_with("is prunable: ") {
            if form != Form::Prune {
                return Err(unrecognized(l));
            }
            dry.reasons.push(l.to_string());
            continue;
        }
        let (tagged, body) = match tail.strip_prefix("[dryrun]") {
            Some(b) => (true, b.trim_start()),
            None => (false, tail),
        };
        if tagged != (form == Form::Prune) {
            return Err(format!(
                "mise's dry run did not mark a line the way its dry run does: `{l}`"
            ));
        }
        let entry = dry.versions.entry(tv.to_string()).or_default();
        let done = match form {
            Form::Uninstall => "✓ uninstalled (dry-run)",
            Form::Prune => "✓ done",
        };
        if body == "uninstall" {
        } else if let Some(p) = body.strip_prefix("remove ") {
            entry.removes.push(expand_home(p.trim(), home));
        } else if body == done {
            entry.done = true;
        } else {
            return Err(unrecognized(l));
        }
    }
    if let Some((tv, _)) = dry.versions.iter().find(|(_, v)| !v.done) {
        return Err(format!(
            "mise's dry run did not end with its dry-run marker for {tv}; swamp shows it and runs \
             nothing"
        ));
    }
    Ok(dry)
}

/// mise's directories as the child sees them: its own variables, then
/// XDG, then the defaults under `home` (the same order mise uses).
struct Dirs {
    data: PathBuf,
    caches: Vec<PathBuf>,
    config: PathBuf,
}

fn dirs(bin: &ToolBin, home: &Path) -> Dirs {
    let var = |k: &str| {
        bin.env_value(k)
            .filter(|v| v.starts_with('/'))
            .map(PathBuf::from)
    };
    let data = var("MISE_DATA_DIR")
        .or_else(|| var("XDG_DATA_HOME").map(|d| d.join("mise")))
        .unwrap_or_else(|| home.join(".local/share/mise"));
    let caches =
        match var("MISE_CACHE_DIR").or_else(|| var("XDG_CACHE_HOME").map(|d| d.join("mise"))) {
            Some(c) => vec![c],
            None => vec![home.join("Library/Caches/mise"), home.join(".cache/mise")],
        };
    let config = var("MISE_CONFIG_DIR")
        .or_else(|| var("XDG_CONFIG_HOME").map(|d| d.join("mise")))
        .unwrap_or_else(|| home.join(".config/mise"));
    Dirs {
        data,
        caches,
        config,
    }
}

/// A path read from manager output, lexically: absolute, and no `.` or
/// `..` component (so `starts_with` means what it says).
fn plain_abs(p: &Path) -> bool {
    use std::path::Component;
    p.is_absolute()
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// Exactly `<base>/<folder>/<version>`, component by component.
fn is_exactly(p: &Path, base: &Path, folder: &str, version: &str) -> bool {
    p.strip_prefix(base).is_ok_and(|rest| {
        let parts: Vec<_> = rest.components().map(|c| c.as_os_str()).collect();
        parts.len() == 2 && parts[0] == folder && parts[1] == version
    })
}

/// The install dir mise uses for `inst`: `<data>/installs/<folder>/<version>`.
fn expected_install(inst: &Install, d: &Dirs) -> PathBuf {
    d.data
        .join("installs")
        .join(crate::manager_facts::mise_folder_name(&inst.tool))
        .join(&inst.version)
}

fn check_removes(inst: &Install, removes: &[PathBuf], d: &Dirs) -> Result<(), String> {
    if removes.is_empty() {
        return Err(format!(
            "mise's dry run names nothing to remove for {}",
            inst.tv()
        ));
    }
    let folder = crate::manager_facts::mise_folder_name(&inst.tool);
    for p in removes {
        let own = plain_abs(p)
            && (is_exactly(p, &d.data.join("installs"), &folder, &inst.version)
                || d.caches
                    .iter()
                    .any(|c| is_exactly(p, c, &folder, &inst.version)));
        if !own {
            return Err(format!(
                "mise's dry run names {}, which is not {}'s install or cache directory",
                p.display(),
                inst.tv()
            ));
        }
    }
    Ok(())
}

/// The global config, in every spelling swamp knows: mise's config dir,
/// or the file `MISE_GLOBAL_CONFIG_FILE` names.
fn is_global_config(p: &Path, d: &Dirs, bin: &ToolBin) -> bool {
    p.starts_with(&d.config)
        || bin
            .env_value("MISE_GLOBAL_CONFIG_FILE")
            .is_some_and(|g| Path::new(g) == p)
}

/// Every directory from `<data>/installs` down to the install dir itself,
/// by `lstat`: a link anywhere on it points the removal somewhere
/// nobody reviewed.
fn linked_component(install: &Path, d: &Dirs) -> Option<PathBuf> {
    let base = d.data.join("installs");
    let rest = install.strip_prefix(&base).ok()?;
    let mut at = base.clone();
    let mut chain = vec![base.clone()];
    for c in rest.components() {
        at.push(c);
        chain.push(at.clone());
    }
    chain
        .into_iter()
        .find(|p| crate::fs_gate::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
}

/// What swamp says about one installed version that mise's own dry run
/// does not check: a config that requests it, mise's own "active". Advice
/// on the confirm, not a refusal: the person decides, and a fresh review
/// at `Y` must show the same facts. The refusals that follow are about
/// the removal being the removal that was reviewed (a link in the path,
/// an install mise does not keep where it keeps them).
fn guard(inst: &Install, home: &Path, bin: &ToolBin) -> Result<Vec<String>, Refusal> {
    let tv = inst.tv();
    let d = dirs(bin, home);
    let mut advice: Vec<String> = Vec::new();
    if let Some(src) = &inst.source {
        advice.push(if is_global_config(src, &d, bin) {
            format!(
                "{tv} is requested by {} (mise's global config); mise's own dry run does not check this. After it goes, mise in a directory that uses it reinstalls it or fails. `mise unuse -g {}` changes the request.",
                tilde(src, home),
                inst.tool
            )
        } else {
            format!(
                "{tv} is requested by {}; after it goes, mise in that directory reinstalls it or fails.",
                tilde(src, home)
            )
        });
    }
    if inst.active {
        advice.push(format!(
            "mise reports {tv} as active: the config that selects it still points at it."
        ));
    }
    if let Some(to) = &inst.symlinked_to {
        return Err(Refusal::new(
            format!(
                "{tv} is a symlink install (to {}); removing it goes through the link.",
                to.display()
            ),
            "Remove it with mise yourself if you mean to.",
        ));
    }
    if !plain_abs(&inst.install_path) || inst.install_path != expected_install(inst, &d) {
        return Err(Refusal::new(
            format!(
                "{tv} is installed at {}, not where mise keeps it ({}).",
                inst.install_path.display(),
                expected_install(inst, &d).display()
            ),
            "Remove it with mise yourself.",
        ));
    }
    if let Some(link) = linked_component(&inst.install_path, &d) {
        return Err(Refusal::new(
            format!(
                "{} is a symlink; removing {tv} would go through it.",
                link.display()
            ),
            "Remove it with mise yourself if you mean to.",
        ));
    }
    Ok(advice)
}

/// `mise prune --tools --dry-run`, parsed. Its exit code must be 0.
fn prune_dry(bin: &ToolBin, home: &Path) -> Result<(Dry, Vec<String>), Refusal> {
    let out = read_ok(bin, PRUNE_DRY, Manager::Mise, "prune --tools --dry-run")?;
    let shown = shown_output(&out);
    if !out.success() {
        return Err(Refusal::new(
            format!("mise's prune dry run exited {}.", code_text(&out)),
            "Read its output below; swamp runs nothing.",
        )
        .with_output(shown));
    }
    let text = format!("{}{}", out.stderr_lossy(), out.stdout_lossy());
    let dry = parse_dry(&text, Form::Prune, home).map_err(|why| {
        Refusal::new(
            format!("{why}."),
            "Read its output below; swamp runs nothing.",
        )
        .with_output(shown.clone())
    })?;
    Ok((dry, shown))
}

/// What mise printed, both streams (mise writes its dry run on stderr).
fn shown_output(out: &RunOutput) -> Vec<String> {
    let mut lines = clean_block(&out.stderr);
    lines.extend(clean_block(&out.stdout));
    super::bound_lines(lines)
}

pub(super) fn candidates(host: &Host, bin: &ToolBin) -> Result<Vec<Candidate>, Refusal> {
    let home = host.home();
    let installs = list_installs(bin)?;
    let prunable: Vec<String> = prune_dry(bin, &home)
        .map(|(d, _)| d.versions.keys().cloned().collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for inst in installs {
        let tv = inst.tv();
        let mut facts = Vec::new();
        if let Some(src) = &inst.source {
            facts.push(format!("requested by {}", tilde(src, &home)));
        }
        if inst.active {
            facts.push("active".to_string());
        }
        if inst.symlinked_to.is_some() {
            facts.push("symlink install".to_string());
        }
        if prunable.contains(&tv) {
            facts.push("mise reports prunable".to_string());
        }
        out.push(Candidate {
            target: Target::MiseVersion {
                tool: inst.tool.clone(),
                version: inst.version.clone(),
            },
            label: tv,
            facts: facts.join(", "),
            path: Some(inst.install_path.clone()),
        });
    }
    Ok(out)
}

fn size_of(paths: &[&Path], sizes: &[(PathBuf, u64)]) -> Option<Size> {
    let mut total = 0u64;
    for p in paths {
        total += sizes.iter().find(|(q, _)| q == p)?.1;
    }
    Some(Size {
        bytes: total,
        source: "swamp's stored measurement",
    })
}

fn regen(inst: &Install) -> String {
    let tv = inst.tv();
    if matches!(inst.tool.as_str(), "python" | "ruby") {
        format!(
            "Reinstall with `mise install {tv}`: a download, or a build from source for {}; its \
             size and time are not measured.",
            inst.tool
        )
    } else {
        format!("Reinstall is a download (`mise install {tv}`); its size is not measured.")
    }
}

pub(super) fn review(
    host: &Host,
    bin: &ToolBin,
    target: &Target,
    sizes: &[(PathBuf, u64)],
) -> Result<Preview, Refusal> {
    let home = host.home();
    let installs = list_installs(bin)?;
    let (prune, _) = prune_dry(bin, &home)?;
    let find = |tv: &str| installs.iter().find(|i| i.tv() == tv);
    let mut state: Vec<(String, String)> = Vec::new();
    let prune_set: Vec<String> = prune.versions.keys().cloned().collect();
    state.push((
        "mise's prune set".to_string(),
        if prune_set.is_empty() {
            "empty".to_string()
        } else {
            prune_set.join(", ")
        },
    ));
    let checked = "Checked: mise's own list (run from /) and what mise's prune reports; a project \
                   mise has never loaded is not checked."
        .to_string();
    match target {
        Target::MiseVersion { tool, version } => {
            let tv = format!("{tool}@{version}");
            let Some(inst) = find(&tv) else {
                return Err(Refusal::new(
                    format!("{tv} is not in mise's list of installed versions any more."),
                    "Press R to refresh, then review again.",
                ));
            };
            // Every entry mise lists for this version: a duplicate that is
            // requested, active or unreadable refuses, whatever the first
            // one says.
            let mut advice: Vec<String> = Vec::new();
            for dup in installs.iter().filter(|i| i.tv() == tv) {
                for line in guard(dup, &home, bin)? {
                    if !advice.contains(&line) {
                        advice.push(line);
                    }
                }
            }
            if !prune.versions.contains_key(&tv) {
                advice.push(format!(
                    "mise's prune does not list {tv}: mise does not report it as unneeded, so a config mise tracks may still request it. `mise prune --tools --dry-run` shows why."
                ));
            }
            state.push((format!("mise's entry for {tv}"), inst.fact(&home)));
            let exec: Vec<OsString> = ["-C", "/", "uninstall", tv.as_str()]
                .iter()
                .map(OsString::from)
                .collect();
            let mut dry = exec.clone();
            dry.insert(3, OsString::from("--dry-run"));
            let dry_args: Vec<&str> = dry.iter().filter_map(|a| a.to_str()).collect();
            let out = read_ok(bin, &dry_args, Manager::Mise, "uninstall --dry-run")?;
            let shown = shown_output(&out);
            if !out.success() {
                return Err(Refusal::new(
                    format!("mise's dry run exited {}.", code_text(&out)),
                    "Read its output below; swamp runs nothing.",
                )
                .with_output(shown));
            }
            let text = format!("{}{}", out.stderr_lossy(), out.stdout_lossy());
            let parsed = parse_dry(&text, Form::Uninstall, &home).map_err(|why| {
                Refusal::new(
                    format!("{why}."),
                    "Read its output below; swamp runs nothing.",
                )
                .with_output(shown.clone())
            })?;
            let names: Vec<&String> = parsed.versions.keys().collect();
            if names != [&tv] {
                return Err(Refusal::new(
                    format!(
                        "mise's dry run names {} version(s), not exactly {tv}.",
                        names.len()
                    ),
                    "Read its output below; swamp runs nothing.",
                )
                .with_output(shown));
            }
            let removes = parsed.versions[&tv].removes.clone();
            check_removes(inst, &removes, &dirs(bin, &home)).map_err(|why| {
                Refusal::new(
                    format!("{why}."),
                    "Remove it with mise yourself if you mean to.",
                )
                .with_output(shown.clone())
            })?;
            let (open_files, open_warning) =
                super::open_files_fact(host, &removes, &[], Manager::Mise);
            state.push(("open files".to_string(), open_files.clone()));
            let mut warnings = vec![PINNED_BY_ENV.to_string()];
            warnings.extend(advice);
            warnings.extend(open_warning);
            let mut evidence: Vec<String> = prune
                .reasons
                .iter()
                .filter(|r| r.starts_with(&format!("mise {tv} ")))
                .map(|r| format!("mise says: \"{}\"", r.trim_start_matches("mise ")))
                .collect();
            evidence.push(checked);
            Ok(Preview {
                manager: Manager::Mise,
                target: target.clone(),
                bin: bin.clone(),
                dry_output_digest: digest(&shown),
                dry_output: shown,
                exec_argv: exec,
                dry_argv: dry,
                removes: removes.iter().map(|p| p.display().to_string()).collect(),
                listed_as: vec![tv.clone()],
                state,
                size: size_of(&[inst.install_path.as_path()], sizes),
                regen: regen(inst),
                evidence,
                warnings,
                open_files,
                manager_version: String::new(),
                title: tv,
            })
        }
        Target::SimRuntime { .. } => Err(Refusal::new(
            "not a mise target",
            "Review it from the simulator runtimes row.",
        )),
    }
}

/// After a removal: which of the previewed versions mise still lists, and
/// which of its install directories are still on disk.
pub(super) fn still_listed(bin: &ToolBin, preview: &Preview) -> Result<Vec<String>, String> {
    let installs = list_installs(bin).map_err(|r| r.reason)?;
    let mut left: Vec<String> = preview
        .listed_as
        .iter()
        .filter(|tv| installs.iter().any(|i| &i.tv() == *tv))
        .cloned()
        .collect();
    for p in &preview.removes {
        let p = Path::new(p);
        if p.components().any(|c| c.as_os_str() == "installs")
            && crate::fs_gate::symlink_metadata(p).is_ok()
        {
            left.push(format!("{} (still on disk)", p.display()));
        }
    }
    Ok(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/dev";

    /// Verbatim from `mise -C / uninstall --dry-run go@1.23.5` (2026.9.15),
    /// with the home directory renamed.
    const UNINSTALL_DRY: &str = "mise go@1.23.5       uninstall\n\
        mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
        mise go@1.23.5     ✓ uninstalled (dry-run)\n";

    /// Verbatim from `mise -C / prune --tools --dry-run` (2026.9.15).
    const PRUNE_DRY_TEXT: &str = "mise java@temurin-17.0.20+101 is prunable: java is required at zulu-8.96.0.19 by ~/src/etl/mise.toml\n\
        mise java@temurin-17.0.20+101 [dryrun]  uninstall\n\
        mise java@temurin-17.0.20+101 [dryrun]  remove ~/.local/share/mise/installs/java/temurin-17.0.20+101\n\
        mise java@temurin-17.0.20+101 [dryrun]  remove ~/Library/Caches/mise/java/temurin-17.0.20+101\n\
        mise java@temurin-17.0.20+101 [dryrun]  ✓ done\n";

    #[test]
    fn the_verified_dry_runs_parse() {
        let d = parse_dry(UNINSTALL_DRY, Form::Uninstall, Path::new(HOME)).unwrap();
        assert_eq!(
            d.versions["go@1.23.5"].removes,
            vec![PathBuf::from(
                "/Users/dev/.local/share/mise/installs/go/1.23.5"
            )]
        );
        let p = parse_dry(PRUNE_DRY_TEXT, Form::Prune, Path::new(HOME)).unwrap();
        assert_eq!(p.versions["java@temurin-17.0.20+101"].removes.len(), 2);
        assert_eq!(p.reasons.len(), 1);
    }

    /// Tempting wrong patch: "parse the `remove` lines only". The real
    /// uninstall prints the same lines without `(dry-run)`; if `--dry-run`
    /// were ever lost from the argv, its output must not read as a
    /// preview.
    #[test]
    fn a_real_run_s_output_is_not_a_preview() {
        let real = UNINSTALL_DRY.replace(" (dry-run)", "");
        assert!(parse_dry(&real, Form::Uninstall, Path::new(HOME)).is_err());
        let real_prune = PRUNE_DRY_TEXT.replace("[dryrun]  ", "");
        assert!(parse_dry(&real_prune, Form::Prune, Path::new(HOME)).is_err());
        // And each form's marker is not the other's.
        assert!(parse_dry(PRUNE_DRY_TEXT, Form::Uninstall, Path::new(HOME)).is_err());
    }

    /// Tempting wrong patch: bare `mise prune` "because --tools is the
    /// default anyway". Its dry run also names configuration links.
    #[test]
    fn a_dry_run_naming_config_links_refuses() {
        let with_links = format!("{PRUNE_DRY_TEXT}mise pruned configuration links [dryrun]\n");
        let err = parse_dry(&with_links, Form::Prune, Path::new(HOME)).unwrap_err();
        assert!(err.contains("configuration links"), "{err}");
    }

    #[test]
    fn garbage_and_errors_refuse() {
        for text in [
            "mise ERROR nosuch not found in mise tool registry\n",
            "hello world\n",
            "mise go@1.23.5 frobnicate\n",
            "mise -rf@x remove /\n",
        ] {
            assert!(
                parse_dry(text, Form::Uninstall, Path::new(HOME)).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn ls_json_parses_sources_and_symlinks() {
        let json = r#"{"node":[{"version":"24.14.1","requested_version":"latest","install_path":"/Users/dev/.local/share/mise/installs/node/24.14.1","source":{"type":"mise.toml","path":"/Users/dev/.config/mise/config.toml"},"installed":true,"active":true}],
            "dotnet":[{"version":"10.0.201","install_path":"/Users/dev/.local/share/mise/installs/dotnet/10.0.201","symlinked_to":"/x","installed":true,"active":false}]}"#;
        let all = parse_ls(json).unwrap();
        assert_eq!(all.len(), 2);
        let node = all.iter().find(|i| i.tool == "node").unwrap();
        assert_eq!(
            node.source.as_deref(),
            Some(Path::new("/Users/dev/.config/mise/config.toml"))
        );
        assert!(all.iter().any(|i| i.symlinked_to.is_some()));
    }
}
