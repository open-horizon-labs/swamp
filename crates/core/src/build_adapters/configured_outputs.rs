//! Discovery of explicitly configured output directories.
//!
//! This is intentionally narrower than build-layout discovery: config is
//! read only when a known checkout or an already observed directory is a
//! project root. No configuration is executed and no tree is searched here.

use super::ConfiguredOutput;
use super::registry::Registry;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Ask registered adapters for declarations at roots already known to the
/// report pipeline. `observed_dirs` may add nested ecosystem roots, but is
/// only an index of paths the walk already saw; this function does no walk.
pub fn discover(
    registry: &Registry,
    project_roots: &[PathBuf],
    observed_dirs: &[PathBuf],
) -> Vec<ConfiguredOutput> {
    let mut roots = project_roots.to_vec();
    roots.extend(
        observed_dirs
            .iter()
            .filter(|d| {
                project_roots.iter().any(|root| d.starts_with(root))
                    && registry
                        .adapters()
                        .iter()
                        .any(|adapter| adapter.is_project_root(d))
            })
            .cloned(),
    );
    roots.sort();
    roots.dedup();

    let mut out = Vec::new();
    for root in roots {
        for adapter in registry.adapters() {
            out.extend(adapter.configured_outputs(&root));
        }
    }
    out.sort_by(|left, right| {
        (
            &left.adapter_id,
            &left.path,
            &left.project_root,
            &left.evidence,
        )
            .cmp(&(
                &right.adapter_id,
                &right.path,
                &right.project_root,
                &right.evidence,
            ))
    });
    out.dedup();
    out
}

/// Normalize `.` and `..` without touching the filesystem. Parent segments
/// above an absolute root are retained so the caller can reject them at its
/// policy boundary; relative paths are anchored before normalization.
pub(super) fn lexical_absolute(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut result = PathBuf::new();
    let mut parents = 0usize;
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if result.file_name().is_some() {
                    result.pop();
                } else {
                    parents += 1;
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    for _ in 0..parents {
        result.push("..");
    }
    result
}

pub(super) fn typescript_outputs(project_root: &Path, adapter_id: &str) -> Vec<ConfiguredOutput> {
    let config = project_root.join("tsconfig.json");
    let Some(declarations) = read_ts_config(&config, project_root, 0, &mut Vec::new()) else {
        return Vec::new();
    };
    declarations
        .into_iter()
        .map(|(key, (path, source))| ConfiguredOutput {
            adapter_id: adapter_id.into(),
            path,
            project_root: project_root.to_path_buf(),
            evidence: format!("TypeScript compilerOptions.{key} from {}", source.display()),
        })
        .collect()
}

type TsDeclarations = HashMap<&'static str, (PathBuf, PathBuf)>;

fn read_ts_config(
    config: &Path,
    project_root: &Path,
    depth: usize,
    visiting: &mut Vec<PathBuf>,
) -> Option<TsDeclarations> {
    const MAX_EXTENDS_DEPTH: usize = 8;
    if depth >= MAX_EXTENDS_DEPTH || !path_is_regular_config(config) {
        return None;
    }
    let config = lexical_absolute(config, project_root);
    if visiting.contains(&config) {
        return None;
    }
    let text =
        super::bounded_io::read_whole_manifest(&config, super::bounded_io::MAX_MANIFEST_BYTES)?;
    let value: serde_json::Value = serde_json::from_str(&strip_jsonc(&text)?).ok()?;
    let object = value.as_object()?;
    visiting.push(config.clone());
    let mut result = TsDeclarations::new();
    if let Some(extends) = object.get("extends").and_then(serde_json::Value::as_str)
        && let Some(base) = resolve_ts_extends(&config, extends)
        && !visiting.contains(&lexical_absolute(&base, project_root))
        && let Some(inherited) = read_ts_config(&base, project_root, depth + 1, visiting)
    {
        result = inherited;
    }
    if let Some(options) = object
        .get("compilerOptions")
        .and_then(serde_json::Value::as_object)
    {
        for (key, field) in [("outDir", "outDir"), ("declarationDir", "declarationDir")] {
            if let Some(value) = options.get(field) {
                result.remove(key);
                if let Some(path) = value.as_str().filter(|value| !value.is_empty()) {
                    result.insert(
                        key,
                        (
                            lexical_absolute(
                                Path::new(path),
                                config.parent().unwrap_or(project_root),
                            ),
                            config.clone(),
                        ),
                    );
                }
            }
        }
    }
    visiting.pop();
    Some(result)
}

fn resolve_ts_extends(config: &Path, value: &str) -> Option<PathBuf> {
    // Package-name resolution and absolute config paths are intentionally
    // unsupported; only bounded relative config inheritance is inspected.
    let relative = Path::new(value);
    if relative.is_absolute() || !value.starts_with('.') {
        return None;
    }
    if relative.components().count() > 10 {
        return None;
    }
    let base = lexical_absolute(relative, config.parent()?);
    let candidates = if base.extension().is_some() {
        vec![base]
    } else {
        vec![base.with_extension("json"), base.with_extension("jsonc")]
    };
    candidates
        .into_iter()
        .find(|candidate| crate::fs_gate::is_file(candidate))
}

fn path_is_regular_config(path: &Path) -> bool {
    crate::fs_gate::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

/// Remove JSONC comments without treating comment markers inside strings as
/// syntax, then remove trailing commas accepted by TypeScript's parser.
fn strip_jsonc(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::with_capacity(source.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push(b'"');
            i += 1;
        } else if byte == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            out.push(b'\n');
        } else if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            let start = i;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return None;
            }
            if bytes[start..i].contains(&b'\n') {
                out.push(b'\n');
            } else {
                out.push(b' ');
            }
            i += 2;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    if in_string {
        return None;
    }
    let out = String::from_utf8(out).ok()?;
    let chars: Vec<char> = out.chars().collect();
    let mut clean = String::with_capacity(out.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in chars.iter().copied().enumerate() {
        if in_string {
            clean.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            clean.push(ch);
        } else if ch == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace()).copied();
            if !matches!(next, Some('}' | ']')) {
                clean.push(ch);
            }
        } else {
            clean.push(ch);
        }
    }
    Some(clean)
}

pub(super) fn maven_outputs(project_root: &Path, adapter_id: &str) -> Vec<ConfiguredOutput> {
    let pom = project_root.join("pom.xml");
    let Some(text) =
        super::bounded_io::read_whole_manifest(&pom, super::bounded_io::MAX_MANIFEST_BYTES)
    else {
        return Vec::new();
    };
    let Ok(document) = roxmltree::Document::parse(&text) else {
        return Vec::new();
    };
    let Some(build) = document
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "build")
    else {
        return Vec::new();
    };
    let literal = |name: &str| -> Option<String> {
        build
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == name)
            .and_then(|node| node.text())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let build_dir = match literal("directory") {
        Some(value) => resolve_maven_path(&value, project_root),
        None => Some(project_root.join("target")),
    };
    let mut outputs = Vec::new();
    for key in ["directory", "outputDirectory", "testOutputDirectory"] {
        if let Some(value) = literal(key)
            && let Some(path) = resolve_maven_value(key, &value, build_dir.as_deref(), project_root)
        {
            outputs.push(ConfiguredOutput {
                adapter_id: adapter_id.into(),
                path,
                project_root: project_root.to_path_buf(),
                evidence: format!("literal Maven <build><{key}> in {}", pom.display()),
            });
        }
    }
    outputs
}

fn resolve_maven_value(
    key: &str,
    value: &str,
    build_dir: Option<&Path>,
    root: &Path,
) -> Option<PathBuf> {
    match key {
        "directory" => resolve_maven_path(value, root),
        "outputDirectory" | "testOutputDirectory" => {
            let expanded = value
                .replace("${basedir}", &root.display().to_string())
                .replace("${project.basedir}", &root.display().to_string());
            let expanded = match build_dir {
                Some(dir) => expanded
                    .replace("${build.directory}", &dir.display().to_string())
                    .replace("${project.build.directory}", &dir.display().to_string()),
                None => expanded,
            };
            if expanded.contains("${") {
                return None;
            }
            Some(lexical_absolute(Path::new(&expanded), root))
        }
        _ => None,
    }
}

fn resolve_maven_path(value: &str, root: &Path) -> Option<PathBuf> {
    let expanded = value
        .replace("${basedir}", &root.display().to_string())
        .replace("${project.basedir}", &root.display().to_string());
    if expanded.contains("${") {
        return None;
    }
    Some(lexical_absolute(Path::new(&expanded), root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_adapters::registry::Registry;
    use std::fs;

    #[test]
    fn discovery_uses_only_roots_and_observed_nested_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let nested = root.join("packages/app");
        let tsconfig_only = root.join("packages/tsconfig-only");
        let unobserved = root.join("packages/hidden");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&tsconfig_only).unwrap();
        fs::create_dir_all(&unobserved).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(nested.join("package.json"), "{}").unwrap();
        fs::write(
            tsconfig_only.join("tsconfig.json"),
            r#"{"compilerOptions":{"outDir":"built"}}"#,
        )
        .unwrap();
        fs::write(unobserved.join("package.json"), "{}").unwrap();
        fs::write(
            nested.join("tsconfig.json"),
            r#"{"compilerOptions":{"outDir":"dist"}}"#,
        )
        .unwrap();
        fs::write(
            unobserved.join("tsconfig.json"),
            r#"{"compilerOptions":{"outDir":"lost"}}"#,
        )
        .unwrap();

        let found = discover(
            &Registry::with_builtins(),
            std::slice::from_ref(&root),
            &[nested.clone(), tsconfig_only.clone()],
        );
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .any(|output| output.path == nested.join("dist"))
        );
        assert!(
            found
                .iter()
                .any(|output| output.path == tsconfig_only.join("built"))
        );
        assert!(found.iter().all(|output| output.adapter_id != "cargo"));
    }

    #[test]
    fn lexical_normalization_is_stable_and_does_not_resolve_symlinks() {
        let root = Path::new("/tmp/repo");
        assert_eq!(
            lexical_absolute(Path::new("./out/../dist"), root),
            Path::new("/tmp/repo/dist")
        );
        assert_eq!(
            lexical_absolute(Path::new("../../outside"), root),
            Path::new("/outside")
        );
    }

    #[test]
    fn typescript_jsonc_preserves_string_markers_and_unicode_paths() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();
        fs::write(
            tmp.path().join("tsconfig.json"),
            r#"{
                // A comment with a fake property: "outDir": "wrong"
                "compilerOptions": {
                    "outDir": "dist/é",
                    "declarationDir": "https://example.test/a//b/*c*/",
                },
                /* block comment with } and ] */
            }"#,
        )
        .unwrap();
        let found = typescript_outputs(tmp.path(), "node");
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|o| o.path == tmp.path().join("dist/é")));
        assert!(
            found
                .iter()
                .any(|o| o.path == tmp.path().join("https://example.test/a//b/*c*/"))
        );
    }

    #[test]
    fn typescript_inherits_relative_ancestor_config_and_child_null_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let app = repo.join("packages/app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), "{}").unwrap();
        fs::write(
            repo.join("tsconfig.base.json"),
            r#"{"compilerOptions":{"outDir":"build-é","declarationDir":"types"}}"#,
        )
        .unwrap();
        fs::write(
            app.join("tsconfig.json"),
            r#"{"extends":"../../tsconfig.base.json","compilerOptions":{"declarationDir":null}}"#,
        )
        .unwrap();
        let found = typescript_outputs(&app, "node");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, repo.join("build-é"));
        assert!(found[0].evidence.contains("tsconfig.base.json"));
    }

    #[test]
    fn typescript_cycles_malformed_and_missing_configs_are_bounded_unknowns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./base","compilerOptions":{"outDir":"own"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("base.json"),
            r#"{"extends":"./tsconfig.json","compilerOptions":{"outDir":"base"}}"#,
        )
        .unwrap();
        let found = typescript_outputs(root, "node");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("own"));
        fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./missing","compilerOptions":{"outDir":"own"}}"#,
        )
        .unwrap();
        assert_eq!(typescript_outputs(root, "node").len(), 1);
        fs::write(root.join("tsconfig.json"), "{ broken").unwrap();
        assert!(typescript_outputs(root, "node").is_empty());
    }

    #[test]
    fn maven_paths_expand_only_supported_literal_properties() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pom.xml"),
            r#"<project><build><directory>../shared/target</directory><outputDirectory>${project.build.directory}/classes</outputDirectory><testOutputDirectory>test-classes</testOutputDirectory></build></project>"#,
        )
        .unwrap();
        let found = maven_outputs(tmp.path(), "maven");
        assert_eq!(found.len(), 3);
        assert!(
            found
                .iter()
                .any(|o| o.path.ends_with("shared/target/classes"))
        );
        assert!(
            found
                .iter()
                .any(|o| o.path == tmp.path().join("test-classes"))
        );
        fs::write(
            tmp.path().join("pom.xml"),
            r#"<project><build><directory>${unknown}/target</directory><outputDirectory>${project.build.directory}/classes</outputDirectory></build></project>"#,
        )
        .unwrap();
        assert!(maven_outputs(tmp.path(), "maven").is_empty());
    }
}
