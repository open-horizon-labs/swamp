//! Compose-file discovery for the `compose_file_name` docker join rule
//! (#28): a `com.docker.compose.project` label frequently carries the
//! compose project name rather than the checkout's own name (e.g.
//! `hiphi-staging` for a `hiphi-relay`/`hiphi-authorizer` checkout), so
//! join rule 1 (label == discovered project name) misses it. This module
//! finds compose files inside a discovered worktree and reads their
//! candidate project names -- the top-level `name:` value and the
//! directory basename -- so the join can match on those instead.
//!
//! Bounded and read-only: files over 1 MiB are skipped, symlinks are
//! never followed (both the directory listing and the file read use
//! non-following metadata), and only the worktree root and one level
//! down are scanned.

use std::path::{Path, PathBuf};

/// Skip any compose file larger than this; a legitimate compose file is
/// nowhere near this size, and a huge match is more likely a mistaken
/// path than a real project manifest.
const MAX_COMPOSE_FILE_BYTES: u64 = 1024 * 1024;

/// One compose file found inside a worktree, with every candidate
/// project name it contributes: its top-level `name:` value (if any) and
/// the basename of the directory it lives in.
#[derive(Debug, Clone)]
pub struct ComposeFile {
    pub path: PathBuf,
    pub names: Vec<String>,
}

fn is_compose_filename(name: &str) -> bool {
    matches!(
        name,
        "compose.yaml" | "compose.yml" | "docker-compose.yaml" | "docker-compose.yml"
    ) || (name.starts_with("compose.") && (name.ends_with(".yaml") || name.ends_with(".yml")))
}

/// Reads the top-level `name:` key from minimal compose YAML: a line
/// starting in column 0 (not indented, so it's a top-level mapping key,
/// not e.g. a service's own `name:`) of the form `name: <value>`. This
/// is deliberately not a full YAML parser -- compose's `name:` is always
/// a short top-level scalar, and pulling in a YAML crate for one field
/// is not worth the dependency (per #28, only if one isn't already a
/// dependency, and none is).
fn parse_top_level_name(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(rest) = line.strip_prefix("name:") else {
            continue;
        };
        let value = rest.trim();
        let value = if let Some(quoted) = value
            .strip_prefix('"')
            .and_then(|v| v.split_once('"'))
            .map(|(v, _)| v)
        {
            quoted
        } else if let Some(quoted) = value
            .strip_prefix('\'')
            .and_then(|v| v.split_once('\''))
            .map(|(v, _)| v)
        {
            quoted
        } else {
            // Unquoted: strip a trailing `#` comment.
            value.split('#').next().unwrap_or(value).trim()
        };
        if value.is_empty() {
            continue;
        }
        return Some(value.to_string());
    }
    None
}

/// Reads one compose file's candidates: its `name:` value (if present
/// and the file is readable and within the size bound) plus the
/// basename of its containing directory. Never follows symlinks and
/// never reads a file bigger than [`MAX_COMPOSE_FILE_BYTES`].
fn read_compose_file(path: &Path) -> Option<ComposeFile> {
    let metadata = crate::fs_gate::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        // Covers symlinks (is_file() is false for a symlink under
        // symlink_metadata, which does not follow it) as well as
        // directories and other non-regular entries.
        return None;
    }
    let dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string);

    let mut names = Vec::new();
    if metadata.len() <= MAX_COMPOSE_FILE_BYTES
        && let Ok(contents) =
            crate::fs_gate::read::bounded_string(path, crate::fs_gate::read::BoundedCap::MANIFEST)
        && let Some(name) = parse_top_level_name(&contents)
    {
        names.push(name);
    }
    if let Some(dir_name) = dir_name {
        names.push(dir_name);
    }
    if names.is_empty() {
        return None;
    }
    Some(ComposeFile {
        path: path.to_path_buf(),
        names,
    })
}

/// Scans a single directory (non-recursively) for compose files.
fn scan_dir(dir: &Path, out: &mut Vec<ComposeFile>) {
    let Ok(entries) = crate::fs_gate::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // `DirEntry::file_type` is lstat-based (does not follow
        // symlinks), so a symlinked compose file is excluded here
        // rather than silently followed.
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_compose_filename(&name) {
            continue;
        }
        if let Some(compose_file) = read_compose_file(&entry.path()) {
            out.push(compose_file);
        }
    }
}

/// Finds compose files at `worktree_root` and one level down (matching
/// `compose.yaml`/`compose.yml`, `docker-compose.yaml`/`docker-compose.yml`,
/// and `compose.*.yaml`/`compose.*.yml` overrides), and returns every one
/// found with its candidate project names.
pub fn discover(worktree_root: &Path) -> Vec<ComposeFile> {
    let mut found = Vec::new();
    scan_dir(worktree_root, &mut found);

    let Ok(entries) = crate::fs_gate::read_dir(worktree_root) else {
        return found;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        scan_dir(&entry.path(), &mut found);
    }
    found
}

/// Convenience wrapper over [`discover`]: the flat, deduplicated set of
/// candidate project names a worktree's compose files contribute.
pub fn discover_candidate_names(worktree_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = discover(worktree_root)
        .into_iter()
        .flat_map(|f| f.names)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_name_only() {
        let contents = "name: fixture-stack\nservices:\n  web:\n    name: not-this-one\n";
        assert_eq!(
            parse_top_level_name(contents),
            Some("fixture-stack".to_string())
        );
    }

    #[test]
    fn parses_quoted_name_and_strips_comment() {
        let contents = "name: \"fixture-stack\" # a comment\n";
        assert_eq!(
            parse_top_level_name(contents),
            Some("fixture-stack".to_string())
        );
    }

    #[test]
    fn discovers_root_and_one_level_down() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        std::fs::write(tmp.path().join("compose.yaml"), "name: root-stack\n").unwrap();
        let sub = tmp.path().join("deploy");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("docker-compose.yml"), "name: sub-stack\n").unwrap();
        let deeper = sub.join("nested");
        std::fs::create_dir_all(&deeper).unwrap();
        std::fs::write(deeper.join("compose.yaml"), "name: too-deep\n").unwrap();

        let names = discover_candidate_names(tmp.path());
        assert!(names.contains(&"root-stack".to_string()));
        assert!(names.contains(&"sub-stack".to_string()));
        assert!(!names.contains(&"too-deep".to_string()));
    }

    #[test]
    fn skips_oversized_file_name_but_keeps_dirname_candidate() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let big = "x".repeat((MAX_COMPOSE_FILE_BYTES + 1) as usize);
        std::fs::write(
            tmp.path().join("compose.yaml"),
            format!("name: should-be-skipped\n{big}"),
        )
        .unwrap();

        let names = discover_candidate_names(tmp.path());
        assert!(!names.contains(&"should-be-skipped".to_string()));
        let dir_name = tmp
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        assert!(names.contains(&dir_name));
    }

    #[test]
    fn never_follows_a_symlinked_compose_file() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let real = tmp.path().join("real-compose.yaml");
        std::fs::write(&real, "name: real-target\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, tmp.path().join("compose.yaml")).unwrap();

        let names = discover_candidate_names(tmp.path());
        assert!(!names.contains(&"real-target".to_string()));
    }

    #[test]
    fn matches_override_filenames() {
        assert!(is_compose_filename("compose.prod.yaml"));
        assert!(is_compose_filename("compose.override.yml"));
        assert!(is_compose_filename("docker-compose.yaml"));
        assert!(!is_compose_filename("compose.txt"));
        assert!(!is_compose_filename("notcompose.yaml"));
    }
}
