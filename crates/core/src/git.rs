//! Git checkout/worktree discovery.
//!
//! A project's identity is the shared object store (the *common* `.git`
//! directory), never a filesystem path. A `.git` **directory** is a main
//! checkout: its own `.git` directory *is* the common dir, so its identity
//! is its own canonical path. A `.git` **file** (`gitdir: <path>`) is a
//! linked worktree: resolve `gitdir:` to `<common>/worktrees/<name>`, then
//! read that directory's `commondir` file to find the shared common dir.
//! Path is an observed attribute of a worktree row; it is never the
//! identity used to group worktrees into a project.

use crate::entities::id_for;
use crate::fs_gate::{
    self as fs,
    read::{BoundedCap, bounded_read},
};
use crate::report::WorktreeKind;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Directories that are never descended into during discovery: `.git`
/// itself (its contents are not project source), plus artifact roots that
/// R3 will classify and size. Stopping here keeps discovery cheap and
/// keeps a `node_modules`/`target`/etc. from ever being misread as a
/// nested project root.
const STOP_DIRS: &[&str] = &["node_modules", "target", "dist", "build"];

/// One discovered git checkout or linked worktree, with its project
/// identity already resolved to the shared object store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorktree {
    /// Stable identity of the shared object store (never a path).
    pub project_id: String,
    /// Name of the main checkout this project's object store belongs to.
    pub project_name: String,
    /// Where this checkout/worktree lives on disk. An attribute, not identity.
    pub path: PathBuf,
    pub kind: WorktreeKind,
    /// The project's `origin` remote URL, read from the shared object
    /// store's `config` file (a linked worktree and a submodule both read
    /// their own common/gitdir's `config`). `None` when there is no
    /// `[remote "origin"]` section, e.g. a checkout with no remote set.
    pub remote_url: Option<String>,
}

/// Reads `[remote "origin"] url = ...` out of a git `config` file living in
/// `git_dir` (a `.git` directory, or a linked worktree's resolved gitdir's
/// common dir). Returns `None` if the file is missing, unreadable, or has
/// no origin remote configured.
fn read_origin_url(git_dir: &Path) -> Option<String> {
    let content = small_text(&git_dir.join("config"), BoundedCap::MANIFEST)?;
    let mut in_origin_remote = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin_remote = trimmed.eq_ignore_ascii_case(r#"[remote "origin"]"#);
            continue;
        }
        if in_origin_remote
            && let Some(rest) = trimmed.strip_prefix("url")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Recurses under `root` for `.git` directories/files and returns one
/// [`DiscoveredWorktree`] per checkout or linked worktree found. Never
/// descends into `.git` itself or into `STOP_DIRS`; never follows
/// symlinks; stays on the device `root` resides on.
pub fn discover(root: &Path) -> Result<Vec<DiscoveredWorktree>> {
    let mut out = Vec::new();
    if !fs::exists(root) {
        return Ok(out);
    }
    let root_dev = fs::symlink_metadata(root)
        .with_context(|| format!("stat {}", root.display()))?
        .dev_for_scan();
    walk(root, root_dev, &mut out);
    Ok(out)
}

// Small trait so we don't repeat `MetadataExt` imports at every call site.
trait DevExt {
    fn dev_for_scan(&self) -> u64;
}
impl DevExt for fs::Metadata {
    fn dev_for_scan(&self) -> u64 {
        use crate::fs_gate::MetadataExt;
        self.dev()
    }
}

fn walk(dir: &Path, device: u64, out: &mut Vec<DiscoveredWorktree>) {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return;
    };
    if meta.dev_for_scan() != device {
        return;
    }
    if meta.file_type().is_symlink() {
        return;
    }
    if !meta.is_dir() {
        return;
    }

    let git_path = dir.join(".git");
    if let Ok(git_meta) = fs::symlink_metadata(&git_path) {
        if git_meta.is_dir() {
            if let Some(dw) = classify_main_checkout(dir, &git_path) {
                out.push(dw);
            }
        } else if git_meta.is_file()
            && let Some(dw) = classify_git_file(dir, &git_path)
        {
            out.push(dw);
        }
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || STOP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        walk(&entry.path(), device, out);
    }
}

fn checkout_name(dir: &Path) -> String {
    dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

fn project_name(dir: &Path, remote_url: Option<&str>) -> String {
    if remote_url.is_none()
        && let Some(name) = crate::ecosystem::manifest_name(dir)
    {
        return name;
    }
    checkout_name(dir)
}

pub(crate) fn classify_main_checkout(dir: &Path, git_dir: &Path) -> Option<DiscoveredWorktree> {
    let common = fs::canonicalize(git_dir).ok()?;
    let project_id = id_for(&common.display().to_string());
    let remote_url = read_origin_url(&common);
    Some(DiscoveredWorktree {
        project_id,
        project_name: project_name(dir, remote_url.as_deref()),
        path: dir.to_path_buf(),
        kind: WorktreeKind::Main,
        remote_url,
    })
}

/// Classifies a `.git` **file** (`gitdir: <path>`). Two cases share this
/// format and must be told apart:
///
/// - A linked worktree's resolved gitdir (`<common>/worktrees/<name>`)
///   contains a `commondir` file pointing back at the shared object
///   store: project identity is that shared common dir, kind `Linked`.
/// - A submodule's `.git` file points at `<parent>/.git/modules/<name>`,
///   which has no `commondir` — it is an independent object store, not a
///   share of the parent's. It is its own project, kind `Main`, exactly
///   like a nested repo discovered through a `.git` directory.
pub(crate) fn classify_git_file(
    worktree_dir: &Path,
    git_file: &Path,
) -> Option<DiscoveredWorktree> {
    let gitdir_path = resolve_gitdir(worktree_dir, git_file)?;
    let commondir_file = gitdir_path.join("commondir");
    if let Some(commondir_content) = small_text(&commondir_file, BoundedCap::POINTER) {
        let common = resolve_common_from_gitdir(&gitdir_path, &commondir_content)?;
        let project_id = id_for(&common.display().to_string());
        // The common dir is `<main checkout>/.git`; the project name is
        // the main checkout's directory name, not the linked worktree's.
        let project_root = common.parent().unwrap_or(worktree_dir);
        let remote_url = read_origin_url(&common);
        Some(DiscoveredWorktree {
            project_id,
            project_name: project_name(project_root, remote_url.as_deref()),
            path: worktree_dir.to_path_buf(),
            kind: WorktreeKind::Linked,
            remote_url,
        })
    } else {
        let project_id = id_for(&gitdir_path.display().to_string());
        let remote_url = read_origin_url(&gitdir_path);
        Some(DiscoveredWorktree {
            project_id,
            project_name: project_name(worktree_dir, remote_url.as_deref()),
            path: worktree_dir.to_path_buf(),
            kind: WorktreeKind::Main,
            remote_url,
        })
    }
}

/// Parses a `.git` file's `gitdir: <path>` line and returns the
/// canonicalized gitdir it points at.
fn resolve_gitdir(worktree_dir: &Path, git_file: &Path) -> Option<PathBuf> {
    let content = small_text(git_file, BoundedCap::POINTER)?;
    let gitdir_line = content.lines().next()?.trim();
    let gitdir_raw = gitdir_line.strip_prefix("gitdir:")?.trim();
    let gitdir_path = PathBuf::from(gitdir_raw);
    let gitdir_path = if gitdir_path.is_absolute() {
        gitdir_path
    } else {
        worktree_dir.join(gitdir_path)
    };
    fs::canonicalize(&gitdir_path).ok()
}

/// Resolves a `commondir` file's contents (relative to `gitdir_path`
/// unless absolute) to the canonicalized shared common `.git` dir.
fn resolve_common_from_gitdir(gitdir_path: &Path, commondir_content: &str) -> Option<PathBuf> {
    let commondir_raw = commondir_content.trim();
    let common_path = PathBuf::from(commondir_raw);
    let common_path = if common_path.is_absolute() {
        common_path
    } else {
        gitdir_path.join(common_path)
    };
    fs::canonicalize(&common_path).ok()
}

/// A small git control file (`.git` pointer, `commondir`, `config`),
/// whole, through the bounded read. `None` when missing, unreadable or
/// larger than `cap`: git never writes these that large, and a prefix
/// parsed as a whole would be a wrong answer.
fn small_text(path: &Path, cap: BoundedCap) -> Option<String> {
    bounded_read(path, cap).ok()?.complete_utf8()
}

/// For a linked worktree at `worktree` (whose `.git` is a `gitdir:`
/// file), the shared common `.git` directory; `None` for a main checkout
/// or anything unreadable. What `git worktree prune` runs against after
/// the worktree was moved to the Trash.
pub fn linked_common_dir(worktree: &Path) -> Option<PathBuf> {
    let git_file = worktree.join(".git");
    if !fs::is_real_file(&git_file) {
        return None;
    }
    let gitdir = resolve_gitdir(worktree, &git_file)?;
    let commondir = small_text(&gitdir.join("commondir"), BoundedCap::POINTER)?;
    resolve_common_from_gitdir(&gitdir, &commondir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn run_git(dir: &Path, args: &[&str]) {
        crate::work_counters::record_spawn();
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn zero_projects_root_returns_empty() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("plain-dir")).unwrap();
        fs::write(tmp.path().join("plain-dir/file.txt"), b"hello").unwrap();
        let found = discover(tmp.path()).unwrap();
        assert!(found.is_empty(), "expected zero projects, got {found:?}");
    }

    #[test]
    fn main_checkout_is_classified_from_git_directory() {
        let tmp = tempdir().unwrap();
        let checkout = tmp.path().join("proj");
        fs::create_dir_all(&checkout).unwrap();
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(&checkout, &["config", "commit.gpgsign", "false"]);
        fs::write(checkout.join("README.md"), b"x").unwrap();
        run_git(&checkout, &["add", "README.md"]);
        run_git(&checkout, &["commit", "-q", "-m", "init"]);

        let found = discover(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, WorktreeKind::Main);
        assert_eq!(found[0].path, checkout);
        assert_eq!(found[0].project_name, "proj");
    }

    #[test]
    fn remote_less_checkout_uses_declarative_manifest_name() {
        let tmp = tempdir().unwrap();
        let checkout = tmp.path().join("folder-name");
        fs::create_dir_all(&checkout).unwrap();
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(&checkout, &["config", "commit.gpgsign", "false"]);
        fs::write(
            checkout.join("settings.gradle"),
            "rootProject.name = \"declared-name\"\n",
        )
        .unwrap();
        run_git(&checkout, &["add", "settings.gradle"]);
        run_git(&checkout, &["commit", "-q", "-m", "init"]);

        let found = discover(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project_name, "declared-name");
    }

    #[test]
    fn git_file_worktree_resolves_to_main_checkout_common_dir() {
        let tmp = tempdir().unwrap();
        let checkout = tmp.path().join("proj");
        fs::create_dir_all(&checkout).unwrap();
        run_git(&checkout, &["init", "-q", "-b", "main"]);
        run_git(&checkout, &["config", "commit.gpgsign", "false"]);
        fs::write(checkout.join("README.md"), b"x").unwrap();
        run_git(&checkout, &["add", "README.md"]);
        run_git(&checkout, &["commit", "-q", "-m", "init"]);

        let linked = tmp.path().join("proj-linked");
        run_git(
            &checkout,
            &[
                "worktree",
                "add",
                "-q",
                linked.to_str().unwrap(),
                "-b",
                "linked",
            ],
        );

        let found = discover(tmp.path()).unwrap();
        assert_eq!(found.len(), 2, "expected main + linked: {found:?}");

        let main_row = found
            .iter()
            .find(|d| d.path == checkout)
            .expect("main checkout row");
        let linked_row = found
            .iter()
            .find(|d| d.path == linked)
            .expect("linked worktree row");

        assert_eq!(main_row.kind, WorktreeKind::Main);
        assert_eq!(linked_row.kind, WorktreeKind::Linked);
        assert_eq!(
            main_row.project_id, linked_row.project_id,
            "linked worktree must share the main checkout's object-store identity"
        );
        assert_eq!(linked_row.project_name, "proj");
    }

    #[test]
    fn nested_repo_is_its_own_project() {
        let tmp = tempdir().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir_all(&outer).unwrap();
        run_git(&outer, &["init", "-q", "-b", "main"]);
        run_git(&outer, &["config", "commit.gpgsign", "false"]);
        fs::write(outer.join("README.md"), b"x").unwrap();
        run_git(&outer, &["add", "README.md"]);
        run_git(&outer, &["commit", "-q", "-m", "init"]);

        let inner = outer.join("nested");
        fs::create_dir_all(&inner).unwrap();
        run_git(&inner, &["init", "-q", "-b", "main"]);
        run_git(&inner, &["config", "commit.gpgsign", "false"]);
        fs::write(inner.join("README.md"), b"y").unwrap();
        run_git(&inner, &["add", "README.md"]);
        run_git(&inner, &["commit", "-q", "-m", "init"]);

        let found = discover(tmp.path()).unwrap();
        assert_eq!(found.len(), 2);
        let outer_id = found
            .iter()
            .find(|d| d.path == outer)
            .unwrap()
            .project_id
            .clone();
        let inner_id = found
            .iter()
            .find(|d| d.path == inner)
            .unwrap()
            .project_id
            .clone();
        assert_ne!(outer_id, inner_id, "nested repo must be a distinct project");
    }

    #[test]
    fn submodule_git_file_is_its_own_main_project_not_a_linked_worktree() {
        // A submodule's `.git` file has the same `gitdir: <path>` shape as a
        // linked worktree's, but its resolved gitdir has no `commondir`
        // file: it is an independent object store, not a share of the
        // parent's. It must be classified Main, as its own project.
        let tmp = tempdir().unwrap();
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        run_git(&parent, &["init", "-q", "-b", "main"]);
        run_git(&parent, &["config", "commit.gpgsign", "false"]);
        fs::write(parent.join("README.md"), b"x").unwrap();
        run_git(&parent, &["add", "README.md"]);
        run_git(&parent, &["commit", "-q", "-m", "init"]);

        let sub_source = tmp.path().join("sub-source");
        fs::create_dir_all(&sub_source).unwrap();
        run_git(&sub_source, &["init", "-q", "-b", "main"]);
        run_git(&sub_source, &["config", "commit.gpgsign", "false"]);
        fs::write(sub_source.join("README.md"), b"y").unwrap();
        run_git(&sub_source, &["add", "README.md"]);
        run_git(&sub_source, &["commit", "-q", "-m", "init"]);

        crate::work_counters::record_spawn();
        let out = Command::new("git")
            .arg("-C")
            .arg(&parent)
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub_source.to_str().unwrap(),
                "vendor/sub",
            ])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "submodule add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let found = discover(tmp.path()).unwrap();
        let submodule_path = parent.join("vendor/sub");
        let sub_row = found
            .iter()
            .find(|d| d.path == submodule_path)
            .expect("submodule discovered as a project");
        assert_eq!(sub_row.kind, WorktreeKind::Main);

        let parent_row = found
            .iter()
            .find(|d| d.path == parent)
            .expect("parent discovered as a project");
        assert_ne!(
            parent_row.project_id, sub_row.project_id,
            "submodule must not share the parent's project identity"
        );
    }

    #[test]
    fn does_not_descend_into_stop_dirs() {
        let tmp = tempdir().unwrap();
        // A repo hiding under node_modules must not be discovered: R2 stops
        // at artifact roots so R3 can classify/size them without racing a
        // project scan through the same tree.
        let hidden = tmp.path().join("node_modules").join("some-pkg");
        fs::create_dir_all(&hidden).unwrap();
        run_git(&hidden, &["init", "-q", "-b", "main"]);
        run_git(&hidden, &["config", "commit.gpgsign", "false"]);
        fs::write(hidden.join("README.md"), b"x").unwrap();
        run_git(&hidden, &["add", "README.md"]);
        run_git(&hidden, &["commit", "-q", "-m", "init"]);

        let found = discover(tmp.path()).unwrap();
        assert!(
            found.is_empty(),
            "must not descend into node_modules: {found:?}"
        );
    }
}
