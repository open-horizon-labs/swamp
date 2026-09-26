//! Associates external build output and shared dependency caches with
//! known projects (#57). Every association here is a *declared*
//! reference (a lockfile entry, an Xcode workspace path recorded at
//! build time) -- never proof that a particular cache entry was
//! actually used at runtime. Multiple consumers, orphaned references
//! (a project no longer present), and unattributed entries are all
//! first-class outcomes; nothing here forces a single owner or
//! duplicates the underlying storage.
//!
//! Every parser operates on already-read text (never touches the
//! filesystem itself), so tests exercise the identity/matching logic
//! against fixture text without real project checkouts or package
//! caches.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
use std::path::Path;

fn now() -> u64 {
    crate::entities::now()
}

// ---------------------------------------------------------------------
// Xcode DerivedData <Name>-<hash>/info.plist -> WorkspacePath
// ---------------------------------------------------------------------

/// Extracts `WorkspacePath`'s string value from an XML-plist rendering
/// of a DerivedData `info.plist` (`<key>WorkspacePath</key><string>
/// ...</string>`). Deliberately not a binary-plist parser: real
/// binary-format `info.plist` files are converted to XML first via the
/// bounded, read-only, output-only `plutil -convert xml1 -o - <path>`
/// (see [`read_workspace_path`]) -- this function only ever sees text.
pub fn parse_workspace_path_from_plist_xml(xml: &str) -> Option<String> {
    let key_pos = xml.find("<key>WorkspacePath</key>")?;
    let after_key = &xml[key_pos..];
    let string_start = after_key.find("<string>")? + "<string>".len();
    let string_end = after_key[string_start..].find("</string>")?;
    let value = &after_key[string_start..string_start + string_end];
    Some(html_unescape(value))
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Real read: converts `info_plist_path` to XML via `plutil` (a fixed,
/// read-only, output-only system utility -- it never rewrites the
/// source file with `-o -`) and extracts `WorkspacePath`. Any failure
/// (missing file, plutil unavailable, unexpected format) is a named
/// `Err`, never a silent `None` indistinguishable from "no workspace
/// recorded".
pub fn read_workspace_path(info_plist_path: &Path) -> Result<Option<String>, String> {
    let output = crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Plutil,
        [
            std::ffi::OsStr::new("-convert"),
            std::ffi::OsStr::new("xml1"),
            std::ffi::OsStr::new("-o"),
            std::ffi::OsStr::new("-"),
            info_plist_path.as_os_str(),
        ],
        std::time::Duration::from_secs(10),
    )
    .map_err(|e| format!("plutil could not be started: {e}"))?;
    if !output.success() {
        return Err(format!(
            "plutil failed to convert {}: {}",
            info_plist_path.display(),
            output.stderr_lossy()
        ));
    }
    let xml = output.stdout_lossy();
    Ok(parse_workspace_path_from_plist_xml(&xml))
}

/// One level of `DerivedData`'s own subdirectories (each named
/// `<ProjectName>-<hash>`, one per built workspace/project): a single
/// bounded `read_dir`, never a recursive walk into any one project's
/// build output. Used to find each subfolder's own `info.plist` for
/// [`read_workspace_path`]/[`xcode_derived_data_association`].
pub fn list_derived_data_subfolders(derived_data: &Path) -> Vec<std::path::PathBuf> {
    // One bounded, single-level listing through the shared helper: this
    // enumerates DerivedData's immediate children, never descends, and
    // is capped (`.oh/guardrails/no-second-traversal-on-report-path.md`).
    crate::locations::shallow_dir_names(derived_data)
        .into_iter()
        .map(|name| derived_data.join(name))
        .collect()
}

/// Builds the Xcode DerivedData -> project consumer evidence: `Known`
/// when `workspace_path` matches (canonicalized) one of `project_roots`,
/// `Unknown` when it names a path that matches none of them (moved or
/// missing project -- never guessed from the DerivedData folder's own
/// name, which is a basename+hash, not a path).
pub fn xcode_derived_data_association(
    workspace_path: Option<&str>,
    project_roots: &[std::path::PathBuf],
) -> Evidence {
    let source = EvidenceSource::BuildMetadata {
        path: "info.plist#WorkspacePath".into(),
    };
    let Some(workspace_path) = workspace_path else {
        return Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            source,
            now(),
            crate::reason!("info.plist has no WorkspacePath recorded"),
        );
    };
    let ws = Path::new(workspace_path);
    let matches: Vec<String> = project_roots
        .iter()
        .filter(|root| ws.starts_with(root.as_path()) || ws == root.as_path())
        .map(|r| r.display().to_string())
        .collect();
    match matches.len() {
        0 => Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            source,
            now(),
            crate::reason!(
                "WorkspacePath {workspace_path} matches no currently known project (moved or missing)"
            ),
        ),
        1 => Evidence::known(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            FactValue::Text(matches[0].clone()),
            source,
            now(),
        ),
        _ => Evidence::conflicting(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            matches.into_iter().map(FactValue::Text).collect(),
            source,
            now(),
            crate::reason!(
                "WorkspacePath matches more than one known project root (nested checkouts)"
            ),
        ),
    }
}

// ---------------------------------------------------------------------
// Dependency lockfile identity extraction
// ---------------------------------------------------------------------

/// One dependency identity a project's lockfile declares: never proof
/// that the corresponding cache entry was actually built/imported this
/// run, only that the project's own lockfile names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DependencyIdentity {
    pub ecosystem: &'static str,
    pub name: String,
    pub version: String,
}

/// `Cargo.lock`: TOML `[[package]]` tables with `name`/`version`.
pub fn parse_cargo_lock(text: &str) -> Result<Vec<DependencyIdentity>, String> {
    #[derive(serde::Deserialize)]
    struct Doc {
        #[serde(default, rename = "package")]
        packages: Vec<Pkg>,
    }
    #[derive(serde::Deserialize)]
    struct Pkg {
        name: String,
        version: String,
    }
    let doc: Doc = toml::from_str(text).map_err(|e| format!("invalid Cargo.lock: {e}"))?;
    Ok(doc
        .packages
        .into_iter()
        .map(|p| DependencyIdentity {
            ecosystem: "cargo",
            name: p.name,
            version: p.version,
        })
        .collect())
}

/// `package-lock.json` (npm lockfile v2/v3 `packages` map, keyed by
/// install path with a `version` field; the root package's own empty-
/// string key is skipped). Falls back to the older `dependencies` map
/// when `packages` is absent.
pub fn parse_package_lock_json(text: &str) -> Result<Vec<DependencyIdentity>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid package-lock.json: {e}"))?;
    let mut out = Vec::new();
    if let Some(packages) = value.get("packages").and_then(|v| v.as_object()) {
        for (path, entry) in packages {
            if path.is_empty() {
                continue; // the root project itself, not a dependency
            }
            let name = path
                .rsplit("node_modules/")
                .next()
                .unwrap_or(path)
                .to_string();
            if let Some(version) = entry.get("version").and_then(|v| v.as_str()) {
                out.push(DependencyIdentity {
                    ecosystem: "npm",
                    name,
                    version: version.to_string(),
                });
            }
        }
        return Ok(out);
    }
    if let Some(deps) = value.get("dependencies").and_then(|v| v.as_object()) {
        for (name, entry) in deps {
            if let Some(version) = entry.get("version").and_then(|v| v.as_str()) {
                out.push(DependencyIdentity {
                    ecosystem: "npm",
                    name: name.clone(),
                    version: version.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// `pnpm-lock.yaml`'s `packages:` section (a narrow line-based scan of
/// its documented `/name@version:` / `/@scope/name@version:` key shape
/// -- not a general YAML parser, mirroring how `.condarc`'s plain
/// YAML-list lines are already read in `locations::conda`). A key that
/// does not match this shape is skipped, not treated as a parse error,
/// since pnpm-lock.yaml carries many other sections this module does
/// not need.
pub fn parse_pnpm_lock_yaml(text: &str) -> Vec<DependencyIdentity> {
    let mut out = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        if line.starts_with("packages:") {
            in_packages = true;
            continue;
        }
        if in_packages {
            if !line.starts_with(' ') && !line.is_empty() {
                break; // left the packages: section
            }
            let trimmed = line.trim();
            let Some(key) = trimmed
                .strip_suffix(':')
                .or_else(|| trimmed.strip_suffix(":"))
            else {
                continue;
            };
            let key = key.trim_start_matches('/');
            if let Some(at) = key.rfind('@') {
                let (name, version) = key.split_at(at);
                let version = &version[1..];
                if !name.is_empty() && !version.is_empty() {
                    out.push(DependencyIdentity {
                        ecosystem: "pnpm",
                        name: name.to_string(),
                        version: version.to_string(),
                    });
                }
            }
        }
    }
    out
}

/// `go.sum`: `<module> <version[/go.mod]> <hash>` lines; deduplicates
/// the plain-module and `/go.mod` line pair for the same module@version.
pub fn parse_go_sum(text: &str) -> Vec<DependencyIdentity> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(module) = parts.next() else { continue };
        let Some(version) = parts.next() else {
            continue;
        };
        let version = version.trim_end_matches("/go.mod");
        if seen.insert((module.to_string(), version.to_string())) {
            out.push(DependencyIdentity {
                ecosystem: "go",
                name: module.to_string(),
                version: version.to_string(),
            });
        }
    }
    out
}

/// `pom.xml`'s `<dependencies><dependency>` entries (direct
/// dependencies only -- `<dependencyManagement>` entries are BOM/version
/// declarations, not necessarily actually depended on, so they are
/// deliberately excluded rather than counted as used), *plus* the
/// dependencies whose identity could not be established at all.
///
/// Parsed with `roxmltree` (a real, read-only, non-validating XML
/// parser -- see `Cargo.toml`; MIT/Apache-2.0, actively maintained),
/// never a hand-rolled tag scan the way
/// `xcode_derived_data_association`'s plist reader gets away with for
/// one fixed tag. `name` is `"group:artifact"`, matching
/// [`parse_gradle_lockfile`]'s own coordinate convention so both
/// ecosystems' identities compare the same way.
///
/// A version that is an unresolved Maven property (`${revision}` in a
/// multi-module build) or inherited from a parent POM that is not on
/// disk here cannot be joined without full Maven property and
/// inheritance resolution, which this module does not implement. Such
/// an entry comes back in the second return value, never guessed and
/// never silently dropped: dropping them made a Maven project with a
/// property-driven version read as declaring nothing, and its local
/// repository read as having no consumer -- the PR #123 review's
/// `a_pom_whose_versions_cannot_be_resolved_must_state_the_gap`
/// counterexample. Swamp cannot say which cache entry the dependency
/// points at; what it can do is say which one it could not resolve and
/// why.
pub fn parse_pom_xml_with_gaps(
    text: &str,
) -> Result<(Vec<DependencyIdentity>, Vec<String>), String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("invalid pom.xml: {e}"))?;
    let mut out = Vec::new();
    let mut gaps: Vec<String> = Vec::new();
    let inherits_from_parent = doc.descendants().any(|n| n.has_tag_name("parent"));
    for node in doc.descendants().filter(|n| n.has_tag_name("dependency")) {
        if node
            .ancestors()
            .any(|a| a.has_tag_name("dependencyManagement"))
        {
            continue;
        }
        let child_text = |tag: &str| -> Option<String> {
            node.children()
                .find(|c| c.has_tag_name(tag))
                .and_then(|c| c.text())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let (group, artifact, version) = (
            child_text("groupId"),
            child_text("artifactId"),
            child_text("version"),
        );
        let (Some(group), Some(artifact)) = (group, artifact) else {
            continue;
        };
        let Some(version) = version else {
            gaps.push(format!(
                "{group}:{artifact} declares no version{}",
                if inherits_from_parent {
                    ", so it is inherited from a parent POM this parser does not resolve"
                } else {
                    ""
                }
            ));
            continue;
        };
        if version.contains("${") {
            // Never guessed -- but never dropped either.
            gaps.push(format!(
                "{group}:{artifact} version is the unresolved Maven property {version}"
            ));
            continue;
        }
        out.push(DependencyIdentity {
            ecosystem: "maven",
            name: format!("{group}:{artifact}"),
            version,
        });
    }
    Ok((out, gaps))
}

/// `gradle.lockfile`: `group:artifact:version=configurations` lines
/// (the `empty=...` marker line and comments are skipped).
pub fn parse_gradle_lockfile(text: &str) -> Vec<DependencyIdentity> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("empty=") {
            continue;
        }
        let Some((coord, _configs)) = line.split_once('=') else {
            continue;
        };
        let parts: Vec<&str> = coord.split(':').collect();
        if parts.len() == 3 {
            out.push(DependencyIdentity {
                ecosystem: "gradle",
                name: format!("{}:{}", parts[0], parts[1]),
                version: parts[2].to_string(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------
// Live shared-store joins (#57 live wiring): given a dependency identity
// already parsed from a project's lockfile, test whether the shared
// store actually holds an entry for it via one deterministic candidate
// path -- a hash lookup (`Path::exists`), never an enumeration of the
// whole store (which can hold many thousands of unrelated entries from
// projects outside scanned scope). This is deliberately the reverse of
// "list the store, then match names back to lockfiles": computing the
// expected path directly from the parsed identity is both cheaper and
// unambiguous (a crate name can itself contain hyphens, so splitting a
// `registry/src` directory's own name back into name+version would be
// lossy; a name+version pair we already have from the lockfile is not).
// ---------------------------------------------------------------------

/// Go's documented module-path escaping
/// (<https://pkg.go.dev/golang.org/x/mod/module#EscapePath>): each
/// uppercase letter becomes `!` followed by its lowercase form (module
/// paths are case-sensitive on a case-insensitive filesystem otherwise).
/// Applied to both the module and version components, since a version
/// can itself contain uppercase pseudo-version metadata.
pub fn go_module_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 4);
    for c in path.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Cargo registry `src/<index-dir>/<name>-<version>` (one or more index
/// directories may exist if more than one registry source was ever
/// configured; every one is checked, still bounded -- typically exactly
/// one, `index.crates.io-<hash>`). `registry_src` is the already-
/// resolved `registry/src` unit path.
pub fn cargo_registry_entry_exists(registry_src: &Path, name: &str, version: &str) -> bool {
    let want = format!("{name}-{version}");
    // One bounded listing of the index directories, then one `exists`
    // probe each -- never a walk into the registry's contents.
    crate::locations::shallow_dir_names(registry_src)
        .into_iter()
        .any(|idx| crate::fs_gate::exists(registry_src.join(idx).join(&want)))
}

/// Go module cache `<GOMODCACHE>/<escaped-module>@<version>`.
/// `gomodcache` is the already-resolved module-cache unit path.
pub fn go_module_cache_entry_exists(gomodcache: &Path, module: &str, version: &str) -> bool {
    let candidate = gomodcache.join(format!(
        "{}@{}",
        go_module_escape(module),
        go_module_escape(version)
    ));
    crate::fs_gate::exists(&candidate)
}

/// Gradle `modules-2/files-2.1/<group>/<artifact>/<version>` under the
/// already-resolved Gradle `caches` unit path. `group_artifact` is
/// `"group:artifact"` ([`parse_gradle_lockfile`]'s own `name` shape).
pub fn gradle_cache_entry_exists(
    gradle_caches: &Path,
    group_artifact: &str,
    version: &str,
) -> bool {
    let Some((group, artifact)) = group_artifact.split_once(':') else {
        return false;
    };
    let entry = gradle_caches
        .join("modules-2/files-2.1")
        .join(group)
        .join(artifact)
        .join(version);
    crate::fs_gate::exists(&entry)
}

/// Maven local repository `<group/path>/<artifact>/<version>` under the
/// already-resolved repository root. `group_artifact` is
/// `"group:artifact"` ([`parse_pom_xml_with_gaps`]'s own `name` shape).
pub fn maven_repo_entry_exists(repo_root: &Path, group_artifact: &str, version: &str) -> bool {
    let Some((group, artifact)) = group_artifact.split_once(':') else {
        return false;
    };
    let entry = repo_root
        .join(group.replace('.', "/"))
        .join(artifact)
        .join(version);
    crate::fs_gate::exists(&entry)
}

/// An unparseable lockfile is a named evidence gap, never a silent
/// "no dependencies declared" (which would be indistinguishable from a
/// project that genuinely has none).
/// A dependency the manifest declares but whose identity this parser
/// cannot resolve. `Unknown`, not absent: the project *does* declare a
/// consumer relationship, swamp just cannot say which cache entry it
/// points at.
pub fn unresolved_dependency_evidence(ecosystem: &str, path: &Path, gap: &str) -> Evidence {
    Evidence::unknown(
        FactKind::Consumer,
        FactSubtype::DeclaredConsumer,
        EvidenceSource::Lockfile {
            ecosystem: ecosystem.to_string(),
            path: path.display().to_string(),
        },
        now(),
        crate::reason!("declared dependency with an unresolved identity: {gap}"),
    )
}

pub fn invalid_lockfile_evidence(ecosystem: &str, path: &Path, parse_error: &str) -> Evidence {
    Evidence::unavailable(
        FactKind::Consumer,
        FactSubtype::DeclaredConsumer,
        EvidenceSource::Lockfile {
            ecosystem: ecosystem.to_string(),
            path: path.display().to_string(),
        },
        now(),
        crate::reason!("could not parse lockfile: {parse_error}"),
    )
}

/// Normalizes an already-established Docker join (compose-project
/// label, `org.opencontainers.image.source` match, or a label naming a
/// path inside a discovered worktree -- see `attribution.rs`'s existing
/// join logic) into this module's shared `Evidence` contract, so a
/// Docker consumer relationship and a lockfile-declared one render the
/// same way. This does not re-derive the join; it only carries an
/// already-decided project label (or `None` for `DockerNoJoin`) into
/// the new shape.
pub fn docker_join_evidence(project_label: Option<&str>, join_basis: &str) -> Evidence {
    let source = EvidenceSource::Inferred {
        basis: join_basis.to_string(),
    };
    match project_label {
        Some(label) => Evidence::known(
            FactKind::Consumer,
            FactSubtype::InferredConsumer,
            FactValue::Text(label.to_string()),
            source,
            now(),
        ),
        None => Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::InferredConsumer,
            source,
            now(),
            crate::reason!(
                "no compose-project label, image-source label, or worktree-path label matched a discovered project"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SAMPLE_INFO_PLIST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>WorkspacePath</key>
    <string>/Users/dev/src/MyApp/MyApp.xcworkspace</string>
</dict>
</plist>
"#;

    #[test]
    fn extracts_workspace_path_from_plist_xml() {
        assert_eq!(
            parse_workspace_path_from_plist_xml(SAMPLE_INFO_PLIST_XML),
            Some("/Users/dev/src/MyApp/MyApp.xcworkspace".to_string())
        );
    }

    #[test]
    fn missing_workspace_path_key_is_none() {
        assert_eq!(
            parse_workspace_path_from_plist_xml("<plist><dict></dict></plist>"),
            None
        );
    }

    #[test]
    fn xcode_association_matches_known_project() {
        let roots = vec![PathBuf::from("/Users/dev/src/MyApp")];
        let ev =
            xcode_derived_data_association(Some("/Users/dev/src/MyApp/MyApp.xcworkspace"), &roots);
        assert!(ev.is_known());
    }

    #[test]
    fn xcode_association_moved_project_is_unknown_never_guessed_from_basename() {
        // The tempting shortcut this rejects: matching DerivedData's own
        // `MyApp-abc123` folder name against a project named "MyApp"
        // even though the recorded WorkspacePath points elsewhere.
        let roots = vec![PathBuf::from("/Users/dev/src/OtherApp")];
        let ev =
            xcode_derived_data_association(Some("/Users/dev/src/MyApp/MyApp.xcworkspace"), &roots);
        assert!(!ev.is_known());
    }

    #[test]
    fn invalid_cargo_lock_is_a_named_gap_not_a_silent_empty_list() {
        let err = parse_cargo_lock("not valid toml [[[").unwrap_err();
        let ev = invalid_lockfile_evidence("cargo", Path::new("/proj/Cargo.lock"), &err);
        assert!(matches!(
            ev.status,
            crate::evidence::FactStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn package_lock_json_parses_packages_map() {
        let text = r#"{
            "packages": {
                "": {"name": "root"},
                "node_modules/lodash": {"version": "4.17.21"}
            }
        }"#;
        let entries = parse_package_lock_json(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "lodash");
        assert_eq!(entries[0].version, "4.17.21");
    }

    #[test]
    fn pnpm_lock_yaml_parses_scoped_and_unscoped_packages() {
        let text = "lockfileVersion: '6.0'\npackages:\n  /lodash@4.17.21:\n    resolution: {}\n  /@babel/core@7.20.0:\n    resolution: {}\nsettings:\n  autoInstallPeers: true\n";
        let entries = parse_pnpm_lock_yaml(text);
        assert!(
            entries
                .iter()
                .any(|e| e.name == "lodash" && e.version == "4.17.21")
        );
        assert!(
            entries
                .iter()
                .any(|e| e.name == "@babel/core" && e.version == "7.20.0")
        );
    }

    #[test]
    fn go_sum_parses_module_version_pairs_deduplicating_go_mod_lines() {
        let text = "golang.org/x/text v0.14.0 h1:abc=\ngolang.org/x/text v0.14.0/go.mod h1:def=\n";
        let entries = parse_go_sum(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "golang.org/x/text");
        assert_eq!(entries[0].version, "v0.14.0");
    }

    #[test]
    fn gradle_lockfile_parses_coordinate_lines() {
        let text =
            "org.jetbrains.kotlin:kotlin-stdlib:1.9.0=compileClasspath\nempty=testCompileOnly\n";
        let entries = parse_gradle_lockfile(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "org.jetbrains.kotlin:kotlin-stdlib");
        assert_eq!(entries[0].version, "1.9.0");
    }

    #[test]
    fn docker_join_normalizes_into_shared_contract() {
        let joined = docker_join_evidence(Some("hiphi-relay"), "compose-project label");
        assert!(joined.is_known());
        let unjoined = docker_join_evidence(None, "no matching label");
        assert!(!unjoined.is_known());
    }

    #[test]
    fn pom_xml_parses_direct_dependencies_excluding_dependency_management() {
        let text = r#"<project>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>com.example.bom</groupId>
        <artifactId>bom</artifactId>
        <version>1.0.0</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <dependencies>
    <dependency>
      <groupId>org.apache.commons</groupId>
      <artifactId>commons-lang3</artifactId>
      <version>3.14.0</version>
    </dependency>
    <dependency>
      <groupId>com.example</groupId>
      <artifactId>unresolved</artifactId>
      <version>${revision}</version>
    </dependency>
  </dependencies>
</project>"#;
        let (entries, gaps) = parse_pom_xml_with_gaps(text).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "BOM entry and unresolved property must both be excluded"
        );
        assert_eq!(entries[0].name, "org.apache.commons:commons-lang3");
        assert_eq!(entries[0].version, "3.14.0");
        // Excluded from the identity list is not the same as dropped:
        // the unresolved-property entry comes back as a named gap. The
        // BOM entry is not a gap -- it was never claimed as a
        // dependency in the first place.
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert!(gaps[0].contains("${revision}"), "{gaps:?}");
    }

    #[test]
    fn pom_xml_invalid_xml_is_a_named_gap() {
        let err = parse_pom_xml_with_gaps("<project><unterminated>").unwrap_err();
        assert!(err.contains("invalid pom.xml"));
    }

    #[test]
    fn go_module_escape_matches_documented_convention() {
        // github.com/BurntSushi/toml -> github.com/!burnt!sushi/toml
        assert_eq!(
            go_module_escape("github.com/BurntSushi/toml"),
            "github.com/!burnt!sushi/toml"
        );
        assert_eq!(go_module_escape("golang.org/x/text"), "golang.org/x/text");
    }

    #[test]
    fn cargo_registry_entry_exists_is_a_targeted_lookup_not_an_enumeration() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = tmp.path().join("index.crates.io-abc123");
        std::fs::create_dir_all(idx.join("serde-1.0.203")).unwrap();
        assert!(cargo_registry_entry_exists(tmp.path(), "serde", "1.0.203"));
        assert!(!cargo_registry_entry_exists(tmp.path(), "serde", "1.0.150"));
        assert!(!cargo_registry_entry_exists(tmp.path(), "libc", "0.2.155"));
    }

    #[test]
    fn go_module_cache_entry_exists_escapes_uppercase() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("github.com/!burnt!sushi/toml@v0.3.1")).unwrap();
        assert!(go_module_cache_entry_exists(
            tmp.path(),
            "github.com/BurntSushi/toml",
            "v0.3.1"
        ));
        assert!(!go_module_cache_entry_exists(
            tmp.path(),
            "github.com/BurntSushi/toml",
            "v0.4.0"
        ));
    }

    #[test]
    fn gradle_cache_entry_exists_checks_coordinate_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            tmp.path()
                .join("modules-2/files-2.1/org.jetbrains.kotlin/kotlin-stdlib/1.9.0"),
        )
        .unwrap();
        assert!(gradle_cache_entry_exists(
            tmp.path(),
            "org.jetbrains.kotlin:kotlin-stdlib",
            "1.9.0"
        ));
        assert!(!gradle_cache_entry_exists(
            tmp.path(),
            "org.jetbrains.kotlin:kotlin-stdlib",
            "2.0.0"
        ));
    }

    #[test]
    fn maven_repo_entry_exists_checks_group_path_conversion() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("org/apache/commons/commons-lang3/3.14.0"))
            .unwrap();
        assert!(maven_repo_entry_exists(
            tmp.path(),
            "org.apache.commons:commons-lang3",
            "3.14.0"
        ));
        assert!(!maven_repo_entry_exists(
            tmp.path(),
            "org.apache.commons:commons-lang3",
            "3.0.0"
        ));
    }

    #[test]
    fn list_derived_data_subfolders_is_one_level_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("MyApp-abc123/Build/Products")).unwrap();
        std::fs::create_dir_all(tmp.path().join("OtherApp-def456")).unwrap();
        let mut names: Vec<String> = list_derived_data_subfolders(tmp.path())
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["MyApp-abc123", "OtherApp-def456"]);
    }
}
