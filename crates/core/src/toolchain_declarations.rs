//! Associates installed tool versions with project declarations and
//! configured defaults (#56). Every parse here is read-only plain text
//! or simple TOML key extraction -- never a shell/mise/asdf plugin
//! evaluation, never activating an environment, never installing
//! anything. See `docs/locations.md` for the version-manager detector
//! catalog this module matches declarations against.
//!
//! A declaration is a fact about what a project or a global default
//! *asks for*; it is not proof of use (a declared-but-unused pin is
//! still a declaration, and its absence is never "unused" -- a project
//! with no `.python-version` may still be actively using Python via an
//! ad-hoc command line). Matching a declaration to a measured
//! installation is manager-specific: an alias/range (`lts/*`, a bare
//! `3.12`) stays an explicit unresolved range unless exactly one
//! installed version uniquely matches it -- never a guessed "closest"
//! version.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
use std::path::{Path, PathBuf};

/// Where a declaration was found: a specific project, or a manager's
/// global/default configuration (`~/.tool-versions`, `pyenv`'s
/// `version` file, ...). Kept distinct so many projects and one global
/// default are never collapsed into the same "consumer".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeclarationScope {
    Project(PathBuf),
    GlobalDefault,
}

/// One version reference read from a project or global config file.
/// `Serialize`/`Deserialize` (#56 live wiring) so the per-worktree
/// mtime-keyed cache in `toolchain_wiring.rs` can persist already-parsed
/// declarations to the current-state report cache without re-reading
/// unchanged declaration files on every report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolVersionDeclaration {
    /// The version-manager convention this came from (`"asdf"`,
    /// `"mise"`, `"pyenv"`, `"rbenv"`, `"nvm"`, `"rustup"`, ...).
    pub manager: String,
    /// The tool the version applies to (`"python"`, `"ruby"`,
    /// `"nodejs"`, `"rust"`, ...). `.tool-versions`/`mise.toml` name
    /// several in one file; every other format implies exactly one.
    pub tool: String,
    pub version_spec: String,
    pub source_path: PathBuf,
    pub scope: DeclarationScope,
}

/// How a declaration's `version_spec` matches (or fails to match) a
/// list of measured installed version strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionMatch {
    /// Resolved to exactly one measured installation.
    Exact { installed: String },
    /// An alias or range (`lts/*`, a bare `3.12`) that no single
    /// measured installation uniquely resolves -- stays explicit,
    /// never guessed.
    UnresolvedRange,
    /// A specific version was declared but no measured installation
    /// matches it. Absence of a match is not evidence the reference is
    /// unused -- the installation may be outside scanned scope, or the
    /// manager query that would confirm it was not run.
    NoMatchingInstallation,
    /// More than one measured installation matches an alias/range
    /// declaration -- named explicitly rather than picking one.
    Conflicting { candidates: Vec<String> },
}

/// A declaration plus its resolution, as a decision-evidence fact.
#[derive(Debug, Clone)]
pub struct ToolVersionAssociation {
    pub declaration: ToolVersionDeclaration,
    pub match_result: VersionMatch,
    pub evidence: Evidence,
}

fn now() -> u64 {
    crate::entities::now()
}

/// True for a version string this module treats as an alias/range
/// rather than a fully pinned version: `nvm`'s `lts/*`/`node`/`stable`
/// aliases, or a bare major(.minor) prefix like `3.12` (as opposed to a
/// fully pinned `3.12.4`).
pub fn is_alias_or_range(spec: &str) -> bool {
    let s = spec.trim();
    if s.is_empty() {
        return false;
    }
    if s.contains('*') || matches!(s, "lts/*" | "node" | "stable" | "system" | "default") {
        return true;
    }
    if let Some(rest) = s.strip_prefix("lts/") {
        return !rest.is_empty(); // lts/<codename> is still an alias
    }
    // A bare numeric prefix (e.g. "3.12", "17") with at most two dotted
    // segments is a range; three segments ("3.12.4") is a full pin.
    let numeric_dotted = s
        .split('.')
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    numeric_dotted && s.split('.').count() < 3
}

/// Matches one declaration's `version_spec` against a manager's
/// measured installed-version strings. `installed` is exact strings
/// this module never fuzzily reinterprets (e.g. `"20.11.0"`, not
/// `"v20.11.0"` unless that is literally how the manager records it).
pub fn match_version(spec: &str, installed: &[String]) -> VersionMatch {
    let spec = spec.trim();
    if installed.iter().any(|v| v == spec) {
        return VersionMatch::Exact {
            installed: spec.to_string(),
        };
    }
    if is_alias_or_range(spec) {
        // A range/prefix: unique-prefix match only, never a guessed
        // "closest" version.
        let prefix = format!("{spec}.");
        let candidates: Vec<String> = installed
            .iter()
            .filter(|v| v.starts_with(&prefix) || **v == spec)
            .cloned()
            .collect();
        return match candidates.len() {
            0 => VersionMatch::UnresolvedRange,
            1 => VersionMatch::Exact {
                installed: candidates[0].clone(),
            },
            _ => VersionMatch::Conflicting { candidates },
        };
    }
    VersionMatch::NoMatchingInstallation
}

/// rustup toolchain directories are named `<channel>-<host-triple>`
/// (`stable-x86_64-apple-darwin`, `1.82.0-aarch64-apple-darwin`) --
/// rustup's own documented naming convention, never a guess. A
/// `rust-toolchain(.toml)` declaration names only the channel, so
/// `match_version`'s exact-string comparison would otherwise never
/// match a real installed toolchain directory name. Returns the one
/// installed directory name whose channel prefix (split on the *first*
/// hyphen only -- rustup channels never contain one: `stable`, `beta`,
/// `nightly`, or a plain `MAJOR.MINOR.PATCH`) exactly equals `spec`,
/// when exactly one such directory exists; `None` when zero or more
/// than one match (left to `match_version`'s own no-match/conflicting
/// handling rather than guessed here).
pub fn resolve_rustup_channel_to_dir(
    spec: &str,
    installed_toolchain_dirs: &[String],
) -> Option<String> {
    if installed_toolchain_dirs.iter().any(|d| d == spec) {
        return None; // already an exact match; let match_version handle it directly
    }
    let mut matches = installed_toolchain_dirs
        .iter()
        .filter(|d| d.split_once('-').map(|(channel, _)| channel) == Some(spec));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None; // more than one -- ambiguous, not guessed
    }
    Some(first.clone())
}

fn declaration_evidence(decl: &ToolVersionDeclaration, m: &VersionMatch) -> Evidence {
    let source = EvidenceSource::ConfigDeclaration {
        path: decl.source_path.display().to_string(),
    };
    match m {
        VersionMatch::Exact { installed } => Evidence::known(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            FactValue::Text(installed.clone()),
            source,
            now(),
        ),
        VersionMatch::UnresolvedRange => Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            source,
            now(),
            crate::reason!(
                "'{}' is an alias/range; no single measured installation uniquely resolves it",
                decl.version_spec
            ),
        ),
        VersionMatch::NoMatchingInstallation => Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            source,
            now(),
            crate::reason!(
                "declared version '{}' matches no measured installation in scanned scope",
                decl.version_spec
            ),
        ),
        VersionMatch::Conflicting { candidates } => Evidence::conflicting(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            candidates.iter().cloned().map(FactValue::Text).collect(),
            source,
            now(),
            crate::reason!(
                "'{}' matches more than one measured installation",
                decl.version_spec
            ),
        ),
    }
}

/// Resolves one declaration against a manager's measured installed
/// versions, producing the evidence fact.
pub fn resolve(decl: ToolVersionDeclaration, installed: &[String]) -> ToolVersionAssociation {
    let match_result = match_version(&decl.version_spec, installed);
    let evidence = declaration_evidence(&decl, &match_result);
    ToolVersionAssociation {
        declaration: decl,
        match_result,
        evidence,
    }
}

/// Builds a [`ToolVersionAssociation`] from an already-decided
/// [`VersionMatch`], bypassing [`match_version`]'s own dotted-version/
/// alias heuristics. Used by `consumer_wiring.rs` for the one manager
/// (rustup) whose installed-directory naming convention
/// (`<channel>-<host-triple>`) `match_version` does not model on its
/// own -- see [`resolve_rustup_channel_to_dir`], which does that
/// widening explicitly rather than guessing inside `match_version`
/// itself (which stays unchanged, and every one of its existing tests
/// keeps passing unmodified).
pub fn resolve_explicit(
    decl: ToolVersionDeclaration,
    match_result: VersionMatch,
) -> ToolVersionAssociation {
    let evidence = declaration_evidence(&decl, &match_result);
    ToolVersionAssociation {
        declaration: decl,
        match_result,
        evidence,
    }
}

// ---------------------------------------------------------------------
// Read-only parsers. Every one operates on already-read text; nothing
// here touches the filesystem itself, so callers control exactly which
// paths are read and tests never need real files.
// ---------------------------------------------------------------------

/// `.tool-versions` (asdf/mise): one `<tool> <version...>` pair per
/// non-comment, non-blank line. mise accepts multiple space-separated
/// versions per tool (fallback list); only the first (preferred) one is
/// treated as the declaration, the rest are recorded as alternates but
/// not separately matched (mise's own semantics, not guessed by this
/// module).
pub fn parse_tool_versions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(tool) = parts.next() else { continue };
        let Some(version) = parts.next() else {
            continue;
        };
        out.push((tool.to_string(), version.to_string()));
    }
    out
}

/// A single-value version file: `.python-version`, `.ruby-version`,
/// `.nvmrc`, `.node-version`, `.java-version`. First non-empty,
/// non-comment line, trimmed.
pub fn parse_single_version_file(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
}

/// `rust-toolchain` (plain string) or `rust-toolchain.toml`
/// (`[toolchain]\nchannel = "..."`). `is_toml` selects the format;
/// callers decide it from the filename, never by sniffing content.
pub fn parse_rust_toolchain(text: &str, is_toml: bool) -> Option<String> {
    if !is_toml {
        return parse_single_version_file(text);
    }
    #[derive(serde::Deserialize)]
    struct Doc {
        toolchain: Toolchain,
    }
    #[derive(serde::Deserialize)]
    struct Toolchain {
        channel: Option<String>,
    }
    toml::from_str::<Doc>(text)
        .ok()
        .and_then(|d| d.toolchain.channel)
}

/// `.mise.toml`/`mise.toml`'s `[tools]` table: `{tool = "version", ...}`
/// or `{tool = ["v1", "v2"]}` (fallback list; first entry only, mirroring
/// `.tool-versions`'s multi-version handling).
pub fn parse_mise_toml(text: &str) -> Vec<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Doc {
        #[serde(default)]
        tools: std::collections::BTreeMap<String, toml::Value>,
    }
    let Ok(doc) = toml::from_str::<Doc>(text) else {
        return Vec::new();
    };
    doc.tools
        .into_iter()
        .filter_map(|(tool, v)| match v {
            toml::Value::String(s) => Some((tool, s)),
            toml::Value::Array(a) => a
                .into_iter()
                .next()
                .and_then(|v| v.as_str().map(|s| (tool, s.to_string()))),
            _ => None,
        })
        .collect()
}

/// rustup's `settings.toml` `default_toolchain` field (the global
/// default, distinct from any project's `rust-toolchain.toml`).
pub fn parse_rustup_default_toolchain(text: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Doc {
        default_toolchain: Option<String>,
    }
    toml::from_str::<Doc>(text)
        .ok()
        .and_then(|d| d.default_toolchain)
}

/// Builds a declaration list for one project root by checking every
/// supported file this module reads, given each file's already-read
/// text (`None` when the file does not exist -- callers do the actual
/// `fs::read_to_string`, keeping this module's own logic file-I/O-free
/// and trivially testable).
#[derive(Default)]
pub struct ProjectDeclarationSources<'a> {
    pub tool_versions: Option<&'a str>,
    pub mise_toml: Option<&'a str>,
    pub python_version: Option<&'a str>,
    pub ruby_version: Option<&'a str>,
    pub nvmrc: Option<&'a str>,
    pub node_version: Option<&'a str>,
    pub rust_toolchain: Option<&'a str>,
    pub rust_toolchain_toml: Option<&'a str>,
    pub java_version: Option<&'a str>,
}

pub fn project_declarations(
    project_root: &Path,
    sources: &ProjectDeclarationSources,
) -> Vec<ToolVersionDeclaration> {
    let scope = DeclarationScope::Project(project_root.to_path_buf());
    let mut out = Vec::new();
    let mut push = |manager: &str, tool: &str, version: String, file: &str| {
        out.push(ToolVersionDeclaration {
            manager: manager.to_string(),
            tool: tool.to_string(),
            version_spec: version,
            source_path: project_root.join(file),
            scope: scope.clone(),
        });
    };
    if let Some(text) = sources.tool_versions {
        for (tool, version) in parse_tool_versions(text) {
            push("asdf-or-mise", &tool, version, ".tool-versions");
        }
    }
    if let Some(text) = sources.mise_toml {
        for (tool, version) in parse_mise_toml(text) {
            push("mise", &tool, version, ".mise.toml");
        }
    }
    if let Some(text) = sources.python_version
        && let Some(v) = parse_single_version_file(text)
    {
        push("pyenv", "python", v, ".python-version");
    }
    if let Some(text) = sources.ruby_version
        && let Some(v) = parse_single_version_file(text)
    {
        push("rbenv-or-rvm", "ruby", v, ".ruby-version");
    }
    if let Some(text) = sources.nvmrc
        && let Some(v) = parse_single_version_file(text)
    {
        push("nvm", "nodejs", v, ".nvmrc");
    }
    if let Some(text) = sources.node_version
        && let Some(v) = parse_single_version_file(text)
    {
        push("nvm", "nodejs", v, ".node-version");
    }
    if let Some(text) = sources.rust_toolchain
        && let Some(v) = parse_rust_toolchain(text, false)
    {
        push("rustup", "rust", v, "rust-toolchain");
    }
    if let Some(text) = sources.rust_toolchain_toml
        && let Some(v) = parse_rust_toolchain(text, true)
    {
        push("rustup", "rust", v, "rust-toolchain.toml");
    }
    if let Some(text) = sources.java_version
        && let Some(v) = parse_single_version_file(text)
    {
        push("jenv-or-sdkman", "java", v, ".java-version");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_versions_lines() {
        let text = "# comment\npython 3.12.4\nnodejs 20.11.0\n\n";
        let parsed = parse_tool_versions(text);
        assert_eq!(
            parsed,
            vec![
                ("python".into(), "3.12.4".into()),
                ("nodejs".into(), "20.11.0".into())
            ]
        );
    }

    #[test]
    fn parses_single_version_file_skipping_comments() {
        assert_eq!(
            parse_single_version_file("# pin\n3.12.4\n"),
            Some("3.12.4".into())
        );
        assert_eq!(parse_single_version_file("\n\n"), None);
    }

    #[test]
    fn parses_rust_toolchain_toml() {
        let text = "[toolchain]\nchannel = \"1.82.0\"\ncomponents = [\"rustfmt\"]\n";
        assert_eq!(parse_rust_toolchain(text, true), Some("1.82.0".into()));
    }

    #[test]
    fn parses_mise_toml_tools_table() {
        let text = "[tools]\npython = \"3.12\"\nnode = [\"20.11.0\", \"18.19.0\"]\n";
        let mut parsed = parse_mise_toml(text);
        parsed.sort();
        assert_eq!(
            parsed,
            vec![
                ("node".into(), "20.11.0".into()),
                ("python".into(), "3.12".into())
            ]
        );
    }

    #[test]
    fn parses_rustup_global_default() {
        let text =
            "default_host_triple = \"aarch64-apple-darwin\"\ndefault_toolchain = \"stable\"\n";
        assert_eq!(parse_rustup_default_toolchain(text), Some("stable".into()));
    }

    #[test]
    fn exact_pinned_version_matches_installed() {
        let installed = vec!["3.12.4".to_string(), "3.11.0".to_string()];
        assert_eq!(
            match_version("3.12.4", &installed),
            VersionMatch::Exact {
                installed: "3.12.4".into()
            }
        );
    }

    #[test]
    fn bare_prefix_range_resolves_when_unique() {
        let installed = vec!["3.12.4".to_string(), "3.11.0".to_string()];
        assert_eq!(
            match_version("3.12", &installed),
            VersionMatch::Exact {
                installed: "3.12.4".into()
            }
        );
    }

    #[test]
    fn bare_prefix_range_stays_unresolved_when_ambiguous() {
        let installed = vec!["3.12.4".to_string(), "3.12.1".to_string()];
        match match_version("3.12", &installed) {
            VersionMatch::Conflicting { candidates } => assert_eq!(candidates.len(), 2),
            other => panic!("expected Conflicting, got {other:?}"),
        }
    }

    #[test]
    fn nvm_alias_never_guessed_even_with_installations_present() {
        // The tempting shortcut this rejects: picking the newest
        // installed Node version as "probably what lts/* means".
        let installed = vec!["20.11.0".to_string(), "18.19.0".to_string()];
        assert_eq!(
            match_version("lts/*", &installed),
            VersionMatch::UnresolvedRange
        );
    }

    #[test]
    fn missing_installation_is_no_match_not_unused() {
        let installed = vec!["3.11.0".to_string()];
        assert_eq!(
            match_version("3.12.4", &installed),
            VersionMatch::NoMatchingInstallation
        );
        // NoMatchingInstallation must not be conflated with "declaration
        // absent"/"unused" in the evidence it produces.
        let decl = ToolVersionDeclaration {
            manager: "pyenv".into(),
            tool: "python".into(),
            version_spec: "3.12.4".into(),
            source_path: PathBuf::from("/proj/.python-version"),
            scope: DeclarationScope::Project(PathBuf::from("/proj")),
        };
        let assoc = resolve(decl, &installed);
        assert!(!assoc.evidence.is_known());
        match assoc.evidence.status {
            crate::evidence::FactStatus::Unknown { .. } => {}
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn global_default_scope_is_distinct_from_project_scope() {
        let global = DeclarationScope::GlobalDefault;
        let project = DeclarationScope::Project(PathBuf::from("/proj"));
        assert_ne!(global, project);
    }

    #[test]
    fn many_projects_share_one_version_without_collapsing_scope() {
        let a = ToolVersionDeclaration {
            manager: "nvm".into(),
            tool: "nodejs".into(),
            version_spec: "20.11.0".into(),
            source_path: PathBuf::from("/proj-a/.nvmrc"),
            scope: DeclarationScope::Project(PathBuf::from("/proj-a")),
        };
        let b = ToolVersionDeclaration {
            manager: "nvm".into(),
            tool: "nodejs".into(),
            version_spec: "20.11.0".into(),
            source_path: PathBuf::from("/proj-b/.nvmrc"),
            scope: DeclarationScope::Project(PathBuf::from("/proj-b")),
        };
        assert_ne!(a.scope, b.scope);
        assert_eq!(a.version_spec, b.version_spec);
    }

    #[test]
    fn project_with_no_declarations_is_not_labelled_unused() {
        let sources = ProjectDeclarationSources::default();
        let decls = project_declarations(Path::new("/proj"), &sources);
        assert!(decls.is_empty());
        // The contract-level guarantee this protects: an empty
        // declaration list must never be rendered/interpreted as
        // "unused toolchain" -- that judgment belongs to a renderer,
        // and this module simply reports nothing found.
    }

    #[test]
    fn project_declarations_reads_every_supported_file() {
        let sources = ProjectDeclarationSources {
            tool_versions: Some("ruby 3.3.0\n"),
            python_version: Some("3.12\n"),
            nvmrc: Some("20\n"),
            rust_toolchain: Some("1.82.0\n"),
            ..Default::default()
        };
        let decls = project_declarations(Path::new("/proj"), &sources);
        let tools: std::collections::HashSet<_> = decls.iter().map(|d| d.tool.as_str()).collect();
        assert!(tools.contains("ruby"));
        assert!(tools.contains("python"));
        assert!(tools.contains("nodejs"));
        assert!(tools.contains("rust"));
    }

    #[test]
    fn rustup_channel_resolves_to_its_one_installed_host_triple_dir() {
        let installed = vec![
            "stable-x86_64-apple-darwin".to_string(),
            "1.82.0-x86_64-apple-darwin".to_string(),
        ];
        assert_eq!(
            resolve_rustup_channel_to_dir("stable", &installed),
            Some("stable-x86_64-apple-darwin".to_string())
        );
        assert_eq!(
            resolve_rustup_channel_to_dir("1.82.0", &installed),
            Some("1.82.0-x86_64-apple-darwin".to_string())
        );
    }

    #[test]
    fn rustup_channel_ambiguous_across_two_host_triples_is_not_guessed() {
        // The tempting shortcut this rejects: picking one host triple's
        // installation arbitrarily when the same channel is installed
        // for more than one target.
        let installed = vec![
            "stable-x86_64-apple-darwin".to_string(),
            "stable-aarch64-apple-darwin".to_string(),
        ];
        assert_eq!(resolve_rustup_channel_to_dir("stable", &installed), None);
    }

    #[test]
    fn rustup_channel_with_no_installed_match_is_none() {
        let installed = vec!["1.82.0-x86_64-apple-darwin".to_string()];
        assert_eq!(resolve_rustup_channel_to_dir("beta", &installed), None);
    }
}
