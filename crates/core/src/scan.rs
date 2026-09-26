use crate::entities::*;
use crate::fs_gate::MetadataExt;
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub cross_device: bool,
    pub max_depth: Option<usize>,
}

impl ScanOptions {
    pub fn canonical_roots(&self) -> Result<Vec<PathBuf>> {
        self.roots
            .iter()
            .map(|root| {
                crate::fs_gate::canonicalize(root)
                    .with_context(|| format!("canonicalize {}", root.display()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ScanRow {
    pub path: PathBuf,
    pub bytes: u64,
    pub device: u64,
    pub folded: bool,
    pub repo_root_id: Option<String>,
    pub linked_worktree: bool,
}

pub fn scan(options: &ScanOptions) -> Result<Vec<ScanRow>> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for root in options.canonical_roots()? {
        let device = crate::fs_gate::metadata_following(&root)
            .with_context(|| format!("stat {}", root.display()))?
            .dev();
        walk(&root, device, options, 0, None, &mut seen, &mut rows)?;
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(rows)
}

fn walk(
    path: &Path,
    device: u64,
    options: &ScanOptions,
    depth: usize,
    inherited_repo: Option<String>,
    seen: &mut HashSet<(u64, u64)>,
    rows: &mut Vec<ScanRow>,
) -> Result<u64> {
    let meta = crate::fs_gate::symlink_metadata(path)?;
    if !options.cross_device && meta.dev() != device {
        return Ok(0);
    }
    if meta.is_file() {
        let key = (meta.dev(), meta.ino());
        if !seen.insert(key) {
            return Ok(0);
        }
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Ok(0);
    }
    let repo = repo_identity(path).or(inherited_repo);
    let entries = match crate::fs_gate::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            rows.push(ScanRow {
                path: path.to_path_buf(),
                bytes: 0,
                device: meta.dev(),
                folded: true,
                repo_root_id: repo.clone(),
                linked_worktree: false,
            });
            return Ok(0);
        }
    };
    if options.max_depth.is_some_and(|d| depth >= d) {
        rows.push(ScanRow {
            path: path.to_path_buf(),
            bytes: 0,
            device: meta.dev(),
            folded: true,
            repo_root_id: repo.clone(),
            linked_worktree: false,
        });
        return Ok(0);
    }
    let mut total = 0;
    for entry in entries.flatten() {
        total += walk(
            &entry.path(),
            device,
            options,
            depth + 1,
            repo.clone(),
            seen,
            rows,
        )?;
    }
    rows.push(ScanRow {
        path: path.to_path_buf(),
        bytes: total,
        device: meta.dev(),
        folded: false,
        repo_root_id: repo,
        linked_worktree: false,
    });
    Ok(total)
}

pub fn repo_identity(path: &Path) -> Option<String> {
    let git = path.join(".git");
    let text = if crate::fs_gate::is_dir(&git) {
        let meta = crate::fs_gate::metadata_following(&git).ok()?;
        format!("git-object-store:{}:{}", meta.dev(), meta.ino())
    } else if crate::fs_gate::is_file(&git) {
        crate::fs_gate::read::bounded_string(&git, crate::fs_gate::read::BoundedCap::POINTER)
            .ok()?
            .trim()
            .strip_prefix("gitdir:")?
            .trim()
            .to_owned()
    } else {
        return None;
    };
    Some(id_for(&text))
}

pub fn fold_artifacts(rows: &[ScanRow]) -> Vec<Artifact> {
    let mut grouped: BTreeMap<PathBuf, &ScanRow> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.folded) {
        grouped.insert(row.path.clone(), row);
    }
    grouped
        .into_iter()
        .map(|(path, row)| {
            let kind = classify(&path);
            let stable_subject = format!(
                "{}:{:?}:{}",
                row.repo_root_id.as_deref().unwrap_or("volume"),
                kind,
                path.file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("artifact")
            );
            Artifact {
                id: id_for(&stable_subject),
                project_id: row.repo_root_id.clone(),
                kind: kind.clone(),
                path: path.clone(),
                relative_path: None,
                bytes: row.bytes,
                recovery: recovery_for(&kind),
                present: true,
                regrowth_count: 0,
                meta: FactMeta::now("filesystem.walk", Confidence::Medium),
            }
        })
        .collect()
}

fn classify(path: &Path) -> ArtifactKind {
    match path.file_name().and_then(|v| v.to_str()).unwrap_or("") {
        "node_modules" | "target" | "dist" | "build" => ArtifactKind::BuildOutput,
        ".git" => ArtifactKind::Git,
        _ => ArtifactKind::Unknown,
    }
}
fn recovery_for(kind: &ArtifactKind) -> RecoveryContract {
    match kind {
        ArtifactKind::BuildOutput | ArtifactKind::DependencyTree => RecoveryContract::LocalRebuild,
        ArtifactKind::Git => RecoveryContract::Irrecoverable,
        _ => RecoveryContract::Manual,
    }
}

pub fn observation(options: &ScanOptions) -> Result<Observation> {
    let rows = scan(options)?;
    let artifacts = fold_artifacts(&rows);
    let bytes = rows
        .iter()
        .filter(|r| !r.folded)
        .map(|r| r.bytes)
        .max()
        .unwrap_or_default();
    let roots = options.canonical_roots()?;
    let projects = roots
        .iter()
        .filter_map(|p| {
            repo_identity(p).map(|id| Project {
                id,
                path: p.clone(),
                confidence: Confidence::High,
                meta: FactMeta::now("git", Confidence::High),
            })
        })
        .collect();
    Ok(Observation {
        volume_id: crate::fs_gate::metadata_following(
            options.roots.first().context("at least one root")?,
        )?
        .dev(),
        roots,
        artifacts,
        projects,
        worktrees: vec![],
        signals: vec![],
        observed_at: now(),
        source: "filesystem.walk".into(),
        coverage_bytes: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    #[test]
    fn repo_identity_survives_path_rename() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join(".git")).unwrap();
        let a = repo_identity(d.path()).unwrap();
        fs::rename(d.path(), d.path().with_extension("moved")).unwrap();
        assert_eq!(a, repo_identity(&d.path().with_extension("moved")).unwrap());
    }

    #[test]
    fn nested_rows_carry_repo_identity() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join(".git")).unwrap();
        fs::create_dir(d.path().join("nested")).unwrap();
        fs::write(d.path().join("nested/file"), b"x").unwrap();
        let rows = scan(&ScanOptions {
            roots: vec![d.path().to_path_buf()],
            cross_device: false,
            max_depth: None,
        })
        .unwrap();
        let repo = repo_identity(d.path()).unwrap();
        assert!(
            rows.iter()
                .filter(|r| r.path.starts_with(d.path()))
                .all(|r| r.repo_root_id.as_deref() == Some(repo.as_str()))
        );
    }
}
