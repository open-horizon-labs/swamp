//! R3: artifact classification at discovery, attributed to the nearest
//! containing checkout/worktree.
//!
//! A directory is classified by basename the moment it is found; a
//! classified directory is sized as one unit (allocated bytes, not
//! descended into for classification) and never walked further. Anything
//! left over inside a worktree becomes that worktree's single `Source`
//! row. Anything outside every worktree is an `UnownedRow`.

use crate::fs_gate::{self as fs, MetadataExt};
use crate::report::{ArtifactKind, ArtifactRow, UnownedRow, WorktreeRow};
#[cfg(test)]
use crate::report::{Source, UnownedReason};
#[cfg(test)]
use std::collections::HashSet;
use std::path::Path;

/// One basename -> kind entry. Data-driven so a later slice (R7) can
/// attach a recovery contract per row without touching the walk.
const ARTIFACT_KINDS: &[(&str, ArtifactKind)] = &[
    // JavaScript / TypeScript
    ("node_modules", ArtifactKind::DependencyTree),
    (".angular", ArtifactKind::Cache),
    (".next", ArtifactKind::BuildOutput),
    (".nuxt", ArtifactKind::BuildOutput),
    (".turbo", ArtifactKind::Cache),
    (".parcel-cache", ArtifactKind::Cache),
    (".expo", ArtifactKind::Cache),
    (".metro", ArtifactKind::Cache),
    (".svelte-kit", ArtifactKind::BuildOutput),
    (".output", ArtifactKind::BuildOutput),
    // Rust
    ("target", ArtifactKind::BuildOutput),
    (".xwin-cache", ArtifactKind::Cache),
    // Python
    (".venv", ArtifactKind::DependencyTree),
    ("venv", ArtifactKind::DependencyTree),
    ("__pycache__", ArtifactKind::BuildOutput),
    ("__pypackages__", ArtifactKind::DependencyTree),
    (".mypy_cache", ArtifactKind::Cache),
    (".pytest_cache", ArtifactKind::Cache),
    (".ruff_cache", ArtifactKind::Cache),
    (".tox", ArtifactKind::Cache),
    (".nox", ArtifactKind::Cache),
    (".pixi", ArtifactKind::DependencyTree),
    (".ipynb_checkpoints", ArtifactKind::Cache),
    // JVM
    (".gradle", ArtifactKind::Cache),
    // Apple
    ("Pods", ArtifactKind::DependencyTree),
    ("DerivedData", ArtifactKind::BuildOutput),
    (".build", ArtifactKind::BuildOutput),
    (".swiftpm", ArtifactKind::Cache),
    // Haskell
    (".stack-work", ArtifactKind::BuildOutput),
    ("dist-newstyle", ArtifactKind::BuildOutput),
    // Elixir
    ("_build", ArtifactKind::BuildOutput),
    (".elixir-tools", ArtifactKind::Cache),
    (".elixir_ls", ArtifactKind::Cache),
    (".lexical", ArtifactKind::Cache),
    // Dart / Flutter
    (".dart_tool", ArtifactKind::Cache),
    // Zig
    ("zig-cache", ArtifactKind::Cache),
    (".zig-cache", ArtifactKind::Cache),
    ("zig-out", ArtifactKind::BuildOutput),
    // C / C++
    ("cmake-build-debug", ArtifactKind::BuildOutput),
    ("cmake-build-release", ArtifactKind::BuildOutput),
    // PHP / Ruby / Go (also Deno's vendored deps)
    // Terraform
    (".terraform", ArtifactKind::DependencyTree),
    // Generic
    (".cache", ArtifactKind::Cache),
    (".git", ArtifactKind::Git),
];

/// Names that are artifacts only next to a project marker, because the
/// bare name is ordinary source elsewhere (`bin/` and `obj/` in a .NET
/// project are build output; `bin/` in a shell repo is scripts). Salvaged
/// from kondo's per-project-type tables: the marker is a sibling in the
/// same directory.
const MARKED_ARTIFACT_KINDS: &[(&str, ArtifactKind, &[&str])] = &[
    // .NET: any *.csproj / *.fsproj / *.sln sibling (checked by extension below)
    (
        "bin",
        ArtifactKind::BuildOutput,
        &["*.csproj", "*.fsproj", "*.sln", "*.vbproj"],
    ),
    (
        "obj",
        ArtifactKind::BuildOutput,
        &["*.csproj", "*.fsproj", "*.sln", "*.vbproj"],
    ),
    // Unity
    (
        "Library",
        ArtifactKind::Cache,
        &["ProjectSettings", "Assets"],
    ),
    ("Temp", ArtifactKind::Cache, &["ProjectSettings", "Assets"]),
    (
        "Obj",
        ArtifactKind::BuildOutput,
        &["ProjectSettings", "Assets"],
    ),
    ("Logs", ArtifactKind::Cache, &["ProjectSettings", "Assets"]),
    (
        "MemoryCaptures",
        ArtifactKind::Cache,
        &["ProjectSettings", "Assets"],
    ),
    (
        "Build",
        ArtifactKind::BuildOutput,
        &["ProjectSettings", "Assets", "*.uproject"],
    ),
    (
        "Builds",
        ArtifactKind::BuildOutput,
        &["ProjectSettings", "Assets"],
    ),
    // Unreal
    ("Binaries", ArtifactKind::BuildOutput, &["*.uproject"]),
    ("Intermediate", ArtifactKind::BuildOutput, &["*.uproject"]),
    ("Saved", ArtifactKind::Cache, &["*.uproject"]),
    ("DerivedDataCache", ArtifactKind::Cache, &["*.uproject"]),
];

fn has_marker(parent: &Path, markers: &[&str]) -> bool {
    let Ok(entries) = crate::fs_gate::read_dir(parent) else {
        return false;
    };
    let names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    // Any one marker present is enough: a `.uproject` file, a `.csproj`,
    // or Unity's `ProjectSettings` directory.
    markers.iter().any(|m| match m.strip_prefix("*.") {
        Some(ext) => names.iter().any(|n| n.ends_with(&format!(".{ext}"))),
        None => names.iter().any(|n| n == m),
    })
}

/// What a fold job carries to prove its directory was classified: only
/// [`classified_at`] (a name/marker/self-declared-cache match) and
/// [`Classified::stored`] (a row the store already classified when it
/// was first walked) make one. `walk::AttrJob::Size` requires it, so
/// "fold this subtree" cannot be written without the classification --
/// not under an `if let` whose initializer merely *mentions*
/// `classify_at`, not anywhere (`.oh/guardrails/folding-only-for-artifacts.md`).
#[derive(Debug, Clone)]
pub(crate) struct Classified(ArtifactKind);

impl Classified {
    /// A kind the store recorded when this path was first walked and
    /// classified; a re-size never reclassifies.
    pub(crate) fn stored(kind: ArtifactKind) -> Classified {
        Classified(kind)
    }

    pub(crate) fn kind(&self) -> &ArtifactKind {
        &self.0
    }
}

/// [`classify_at`], as the witness a fold job needs.
pub(crate) fn classified_at(parent: &Path, name: &str) -> Option<Classified> {
    classify_at(parent, name).map(Classified)
}

/// Classification with the parent directory available, so marker-gated
/// names (`bin`, `obj`, Unity's `Library`, Unreal's `Intermediate`) count
/// only inside the project type that generates them.
pub(crate) fn classify_at(parent: &Path, name: &str) -> Option<ArtifactKind> {
    if let Some(k) = classify(name) {
        return Some(k);
    }
    // The strongest evidence there is: the tool that created the
    // directory says it is a cache. No name, no marker, no guess.
    if let Some(k) = self_declared_cache(&parent.join(name)) {
        return Some(k);
    }
    // Names several ecosystems generate (`build`, `dist`, `vendor`, `out`,
    // `coverage`, `*.egg-info`, …) count only next to a marker of one that
    // does: clean-dev-dirs' per-language detection, table-driven.
    if let Some(k) = crate::ecosystem::classify_gated(parent, name) {
        return Some(k);
    }
    MARKED_ARTIFACT_KINDS
        .iter()
        .find(|(n, _, markers)| *n == name && has_marker(parent, markers))
        .map(|(_, k, _)| k.clone())
}

/// A directory that declares itself a cache under the Cache Directory
/// Tagging Specification (<https://bford.info/cachedir/>): a *regular*
/// file `CACHEDIR.TAG` whose first 43 bytes are exactly the signature
/// below. Cargo writes one into `target/`, pytest into `.pytest_cache/`,
/// uv into `.venv/`. Unlike a name or a sibling marker, this is the
/// creating tool's own claim, so it needs no ecosystem gate — and a
/// directory that merely *mentions* the string does not qualify, because
/// the signature must start at byte 0.
pub(crate) fn self_declared_cache(dir: &Path) -> Option<ArtifactKind> {
    const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
    let tag = dir.join("CACHEDIR.TAG");
    // `symlink_metadata`: the spec requires a regular file, and a symlink
    // here would also be a path out of the tree we are sizing.
    let meta = crate::fs_gate::symlink_metadata(&tag).ok()?;
    if !meta.is_file() || meta.len() < SIGNATURE.len() as u64 {
        return None;
    }
    let head = crate::fs_gate::read::bounded_read(
        &tag,
        crate::fs_gate::read::BoundedCap::header_at_most(SIGNATURE.len()),
    )
    .ok()?;
    (head.bytes == SIGNATURE).then_some(ArtifactKind::Cache)
}

/// Basenames that, when found *outside* every checkout/worktree, are a
/// shared cache rather than an ordinary unowned path.
const SHARED_CACHE_NAMES: &[&str] = &[
    ".cache",
    ".cargo-registry",
    ".npm",
    ".pnpm-store",
    ".gradle",
];

pub(crate) fn classify(name: &str) -> Option<ArtifactKind> {
    ARTIFACT_KINDS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, k)| k.clone())
        // Shared-cache-shaped names not already in the artifact table
        // (e.g. `.cargo-registry`, `.npm`, `.pnpm-store`) are still sized
        // as one unit rather than descended into; `record_artifact`
        // decides project-internal Cache vs. unowned SharedCache from
        // whether a containing worktree is found.
        .or_else(|| is_shared_cache_name(name).then_some(ArtifactKind::Cache))
}

pub(crate) fn is_shared_cache_name(name: &str) -> bool {
    SHARED_CACHE_NAMES.contains(&name)
}

/// Allocated bytes for one file: `st_blocks * 512`, the actual space the
/// file occupies on disk rather than its logical length.
pub(crate) fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    meta.blocks() * 512
}

/// A worktree known to the attribution pass: its path (for nearest-match)
/// and the id it should attach rows to.
#[cfg(test)]
struct KnownWorktree<'a> {
    path: &'a Path,
    worktree_id: &'a str,
}

/// Output of the attribution walk: per-worktree artifact rows (including
/// the synthesized `Source` row), unowned rows, and the walked total.
#[derive(Debug, Clone)]
pub struct AttributionResult {
    /// worktree_id -> artifact rows for that worktree.
    pub artifacts_by_worktree: std::collections::HashMap<String, Vec<ArtifactRow>>,
    pub unowned: Vec<UnownedRow>,
    pub walked_total: u64,
    /// Sum of every artifact row's bytes (all worktrees), used for
    /// reconciliation instead of re-deriving it from the result map.
    pub attributed_total: u64,
    /// Sum of unowned bytes actually seen by this filesystem walk
    /// (excludes anything added later from an out-of-band source such as
    /// Docker facts, which never appear in `walked_total` either).
    pub unowned_total: u64,
    /// R4c: per-directory rollups under each worktree's Source tree.
    /// Empty from this serial reference walk (`attribute`, kept for its
    /// existing unit tests); only the parallel walk
    /// (`walk::attribute_parallel`) populates this.
    pub dirs: Vec<crate::report::DirRollup>,
    /// R4c: large-file rows (>= the configured threshold) found under
    /// each worktree's Source tree.
    pub files: Vec<crate::report::FileRow>,
}

#[cfg(test)]
struct Ctx<'a> {
    worktrees: Vec<KnownWorktree<'a>>,
    seen_inodes: HashSet<(u64, u64)>,
    artifacts_by_worktree: std::collections::HashMap<String, Vec<ArtifactRow>>,
    source_bytes: std::collections::HashMap<String, u64>,
    unowned: Vec<UnownedRow>,
    walked_total: u64,
    attributed_total: u64,
    unowned_total: u64,
    observed_at: u64,
}

#[cfg(test)]
/// Finds the id of the worktree whose path is the longest prefix of
/// `path` (the nearest containing checkout/worktree), if any.
fn nearest_worktree<'a>(worktrees: &'a [KnownWorktree<'a>], path: &Path) -> Option<&'a str> {
    worktrees
        .iter()
        .filter(|w| path.starts_with(w.path))
        .max_by_key(|w| w.path.as_os_str().len())
        .map(|w| w.worktree_id)
}

#[cfg(test)]
impl<'a> Ctx<'a> {
    fn dedup(&mut self, meta: &fs::Metadata) -> bool {
        // Hardlinks are counted once per report.
        self.seen_inodes.insert((meta.dev(), meta.ino()))
    }

    /// Sizes a classified artifact directory as one unit: sums allocated
    /// bytes of every regular file underneath (deduped by hardlink),
    /// without treating any of it as source or classifying subdirectories
    /// further.
    fn size_as_unit(&mut self, path: &Path) -> u64 {
        let mut total = 0u64;
        let Ok(entries) = crate::fs_gate::read_dir(path) else {
            return total;
        };
        for entry in entries.flatten() {
            let Ok(meta) = crate::fs_gate::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                total += self.size_as_unit(&entry.path());
            } else if meta.is_file() && self.dedup(&meta) {
                total += allocated_bytes(&meta);
            }
        }
        total
    }

    /// Records a classified artifact at `path` (kind already resolved),
    /// attributing it to the nearest containing worktree or recording it
    /// as unowned.
    fn record_artifact(&mut self, path: &Path, kind: ArtifactKind) {
        let bytes = self.size_as_unit(path);
        self.walked_total += bytes;
        match nearest_worktree(&self.worktrees, path) {
            Some(worktree_id) => {
                self.attributed_total += bytes;
                self.artifacts_by_worktree
                    .entry(worktree_id.to_string())
                    .or_default()
                    .push(ArtifactRow {
                        kind,
                        path: path.to_path_buf(),
                        bytes,
                        mtime_max: 0,
                        ecosystem: None,
                        hardlinked: false,
                        dedup_stale: false,
                        allocated_bytes: None,
                        allocated_growth_bytes: None,
                        local_bytes: 0,
                        track: None,
                        growth_bytes: None,
                        regrowth_count: 0,
                        observed_at: self.observed_at,
                        confidence: crate::entities::Confidence::High,
                        source: Source::new("filesystem.walk"),
                        note: None,
                        created_at: None,
                        containers: Vec::new(),
                        shared_with: Vec::new(),
                        dangling: false,
                        evidence: Vec::new(),
                    });
            }
            None => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let reason = if is_shared_cache_name(name) {
                    UnownedReason::SharedCache
                } else {
                    UnownedReason::NoContainingRepo
                };
                self.unowned_total += bytes;
                self.unowned.push(UnownedRow {
                    measurement: None,
                    path_or_object: path.display().to_string(),
                    bytes,
                    reason,
                    shared_bytes: None,
                    note: None,
                    created_at: None,
                    containers: Vec::new(),
                    shared_with: Vec::new(),
                    dangling: false,
                    docker_kind: None,
                    evidence: Vec::new(),
                });
            }
        }
    }

    fn record_permission_denied(&mut self, path: &Path) {
        self.unowned.push(UnownedRow {
            measurement: None,
            path_or_object: path.display().to_string(),
            bytes: 0,
            reason: UnownedReason::PermissionDenied,
            shared_bytes: None,
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            docker_kind: None,
            evidence: Vec::new(),
        });
    }

    fn record_file(&mut self, path: &Path, meta: &fs::Metadata) {
        if !self.dedup(meta) {
            return;
        }
        let bytes = allocated_bytes(meta);
        self.walked_total += bytes;
        match nearest_worktree(&self.worktrees, path) {
            Some(worktree_id) => {
                self.attributed_total += bytes;
                *self
                    .source_bytes
                    .entry(worktree_id.to_string())
                    .or_default() += bytes;
            }
            None => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let reason = if is_shared_cache_name(name) {
                    UnownedReason::SharedCache
                } else {
                    UnownedReason::NoContainingRepo
                };
                self.unowned_total += bytes;
                self.unowned.push(UnownedRow {
                    measurement: None,
                    path_or_object: path.display().to_string(),
                    bytes,
                    reason,
                    shared_bytes: None,
                    note: None,
                    created_at: None,
                    containers: Vec::new(),
                    shared_with: Vec::new(),
                    dangling: false,
                    docker_kind: None,
                    evidence: Vec::new(),
                });
            }
        }
    }

    fn walk(&mut self, path: &Path) {
        let Ok(meta) = crate::fs_gate::symlink_metadata(path) else {
            return;
        };
        if meta.file_type().is_symlink() {
            return;
        }
        if meta.is_file() {
            self.record_file(path, &meta);
            return;
        }
        if !meta.is_dir() {
            return;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if let Some(kind) = path.parent().and_then(|parent| classify_at(parent, name)) {
            self.record_artifact(path, kind);
            return;
        }

        let entries = match crate::fs_gate::read_dir(path) {
            Ok(e) => e,
            Err(_) => {
                self.record_permission_denied(path);
                return;
            }
        };
        for entry in entries.flatten() {
            self.walk(&entry.path());
        }
    }
}

/// Runs the R3 classification/attribution walk under `root`, given the
/// worktrees R2 already discovered (path + worktree_id): the serial
/// reference walk, kept as the oracle this module's tests compare the
/// parallel pool (`walk::discover_and_attribute`) against.
///
/// Test-only (`.oh/guardrails/walk-optimized-parallel-pool.md`): a
/// production path that reached for it -- directly, through an alias,
/// or inside a local `macro_rules!` -- does not compile.
#[cfg(test)]
pub fn attribute(root: &Path, worktrees: &[(&Path, &str)], observed_at: u64) -> AttributionResult {
    let known: Vec<KnownWorktree> = worktrees
        .iter()
        .map(|(path, id)| KnownWorktree {
            path,
            worktree_id: id,
        })
        .collect();
    let mut ctx = Ctx {
        worktrees: known,
        seen_inodes: HashSet::new(),
        artifacts_by_worktree: std::collections::HashMap::new(),
        source_bytes: std::collections::HashMap::new(),
        unowned: Vec::new(),
        walked_total: 0,
        attributed_total: 0,
        unowned_total: 0,
        observed_at,
    };
    ctx.walk(root);

    for (worktree_id, bytes) in &ctx.source_bytes {
        if *bytes == 0 {
            continue;
        }
        let path = worktrees
            .iter()
            .find(|(_, id)| id == worktree_id)
            .map(|(p, _)| p.to_path_buf())
            .unwrap_or_default();
        ctx.artifacts_by_worktree
            .entry(worktree_id.clone())
            .or_default()
            .push(ArtifactRow {
                kind: ArtifactKind::Source,
                path,
                bytes: *bytes,
                mtime_max: 0,
                ecosystem: None,
                hardlinked: false,
                dedup_stale: false,
                allocated_bytes: None,
                allocated_growth_bytes: None,
                local_bytes: 0,
                track: None,
                growth_bytes: None,
                regrowth_count: 0,
                observed_at: ctx.observed_at,
                confidence: crate::entities::Confidence::High,
                source: Source::new("filesystem.walk"),
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            });
    }

    AttributionResult {
        artifacts_by_worktree: ctx.artifacts_by_worktree,
        unowned: ctx.unowned,
        walked_total: ctx.walked_total,
        attributed_total: ctx.attributed_total,
        unowned_total: ctx.unowned_total,
        dirs: Vec::new(),
        files: Vec::new(),
    }
}

/// Fills each `WorktreeRow`'s `artifacts` from the attribution result.
pub fn apply_to_worktree(row: &mut WorktreeRow, result: &mut AttributionResult) {
    if let Some(rows) = result.artifacts_by_worktree.remove(&row.worktree_id) {
        row.artifacts = rows;
    }
}

/// Runs `du -skPx <root>` and returns bytes (KiB * 1024), or `None` if
/// the command is unavailable or fails.
pub fn du_total(root: &Path) -> Option<u64> {
    // `du` over a whole root is slow by nature; the bound is a hang
    // guard, not a cost budget (`--verify-du` is an explicit opt-in).
    let out = crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Du,
        [std::ffi::OsStr::new("-skPx"), root.as_os_str()],
        std::time::Duration::from_secs(3600),
    )
    .ok()?;
    if !out.success() {
        return None;
    }
    let text = out.stdout_lossy();
    let kib: u64 = text.split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn marker_gated_names_classify_only_next_to_their_project_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dotnet = tmp.path().join("app");
        std::fs::create_dir_all(dotnet.join("bin")).unwrap();
        std::fs::write(dotnet.join("App.csproj"), "<Project/>").unwrap();
        assert_eq!(classify_at(&dotnet, "bin"), Some(ArtifactKind::BuildOutput));
        assert_eq!(classify_at(&dotnet, "obj"), Some(ArtifactKind::BuildOutput));

        let shell = tmp.path().join("scripts");
        std::fs::create_dir_all(shell.join("bin")).unwrap();
        assert_eq!(
            classify_at(&shell, "bin"),
            None,
            "bin/ in a shell repo is source"
        );

        let unity = tmp.path().join("game");
        std::fs::create_dir_all(unity.join("ProjectSettings")).unwrap();
        assert_eq!(classify_at(&unity, "Library"), Some(ArtifactKind::Cache));
        assert_eq!(classify_at(&shell, "Library"), None);

        let unreal = tmp.path().join("ue");
        std::fs::create_dir_all(&unreal).unwrap();
        std::fs::write(unreal.join("Game.uproject"), "{}").unwrap();
        assert_eq!(
            classify_at(&unreal, "Intermediate"),
            Some(ArtifactKind::BuildOutput)
        );

        // Unconditional names still classify anywhere.
        assert_eq!(
            classify_at(&shell, "node_modules"),
            Some(ArtifactKind::DependencyTree)
        );
        assert_eq!(
            classify_at(&shell, ".stack-work"),
            Some(ArtifactKind::BuildOutput)
        );
        assert_eq!(
            classify_at(&shell, "zig-out"),
            Some(ArtifactKind::BuildOutput)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn touch(path: &Path, bytes: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes as usize]).unwrap();
    }

    #[test]
    fn classification_stops_at_artifact_boundary_does_not_descend() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let repo = root.join("proj");
        touch(&repo.join("node_modules/pkg/deeply/nested/file.js"), 4096);
        touch(&repo.join("README.md"), 4096);

        let worktrees = [(repo.as_path(), "wt1")];
        let result = attribute(root, &worktrees, 0);

        let rows = result.artifacts_by_worktree.get("wt1").unwrap();
        let node_modules_row = rows
            .iter()
            .find(|r| r.kind == ArtifactKind::DependencyTree)
            .expect("node_modules classified as one unit");
        assert_eq!(node_modules_row.path, repo.join("node_modules"));
        // Only one artifact row for the whole node_modules subtree: the
        // deeply nested file never produced its own row.
        assert_eq!(
            rows.iter()
                .filter(|r| r.kind == ArtifactKind::DependencyTree)
                .count(),
            1
        );
    }

    #[test]
    fn ruby_vendor_bundle_folds_only_the_bundle_subtree() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().join("ruby-app");
        touch(&repo.join("Gemfile"), 16);
        touch(&repo.join("vendor/bundle/gems/installed.rb"), 4096);
        touch(&repo.join("vendor/handwritten/license.txt"), 2048);

        let result = attribute(tmp.path(), &[(repo.as_path(), "ruby-wt")], 0);
        let rows = result.artifacts_by_worktree.get("ruby-wt").unwrap();
        let bundle = rows
            .iter()
            .find(|r| r.kind == ArtifactKind::DependencyTree)
            .expect("vendor/bundle is a dependency artifact");
        assert_eq!(bundle.path, repo.join("vendor/bundle"));
        assert_eq!(
            rows.iter()
                .filter(|r| r.kind == ArtifactKind::DependencyTree)
                .count(),
            1
        );
        let source = rows
            .iter()
            .find(|r| r.kind == ArtifactKind::Source)
            .expect("unrelated vendor content remains source");
        assert!(source.bytes >= 2048);
    }

    #[test]
    fn rust_workspace_target_is_single_and_nested_nonmember_target_is_retained() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        // A shared workspace target is one physical artifact at the root;
        // the member has no target of its own.
        touch(&workspace.join("target/shared.bin"), 4096);
        fs::create_dir_all(workspace.join("member")).unwrap();
        fs::write(
            workspace.join("member/Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // This nested Cargo project is deliberately outside the workspace
        // members and owns an independent target that must not be hidden by
        // the shared-target fixture.
        let nested = workspace.join("tools/independent");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"independent\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        touch(&nested.join("target/independent.bin"), 2048);

        let result = attribute(root, &[(workspace.as_path(), "workspace-wt")], 0);
        let rows = result.artifacts_by_worktree.get("workspace-wt").unwrap();
        let targets: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == ArtifactKind::BuildOutput)
            .filter(|r| r.path.file_name().and_then(|n| n.to_str()) == Some("target"))
            .collect();
        assert_eq!(targets.len(), 2, "both physical targets are retained once");
        assert!(targets.iter().any(|r| r.path == workspace.join("target")));
        assert!(targets.iter().any(|r| r.path == nested.join("target")));
        assert!(targets.iter().all(|r| r.bytes > 0));
    }

    #[test]
    fn nested_repo_build_dir_attributes_to_nested_repo_not_parent() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let parent = root.join("parent");
        let nested = parent.join("nested");
        // `build/` is an artifact only next to a marker of an ecosystem
        // that generates it (clean-dev-dirs' rule); CMake here.
        touch(&nested.join("CMakeLists.txt"), 16);
        touch(&nested.join("build/out.bin"), 4096);
        touch(&parent.join("README.md"), 4096);

        let worktrees = [
            (parent.as_path(), "parent-wt"),
            (nested.as_path(), "nested-wt"),
        ];
        let result = attribute(root, &worktrees, 0);

        let nested_rows = result.artifacts_by_worktree.get("nested-wt").unwrap();
        assert!(
            nested_rows
                .iter()
                .any(|r| r.kind == ArtifactKind::BuildOutput && r.path == nested.join("build")),
            "nested repo owns its own build/ artifact: {nested_rows:?}"
        );
        let parent_rows = result.artifacts_by_worktree.get("parent-wt");
        let parent_has_build = parent_rows
            .map(|rows| rows.iter().any(|r| r.kind == ArtifactKind::BuildOutput))
            .unwrap_or(false);
        assert!(
            !parent_has_build,
            "parent must not also claim the nested build/"
        );
    }

    #[test]
    fn a_directory_that_declares_itself_a_cache_is_one_whatever_its_name() {
        const SIG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("weird-name-nobody-tabulated");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(classify_at(tmp.path(), "weird-name-nobody-tabulated"), None);
        fs::write(dir.join("CACHEDIR.TAG"), SIG).unwrap();
        assert_eq!(
            classify_at(tmp.path(), "weird-name-nobody-tabulated"),
            Some(ArtifactKind::Cache),
            "the creating tool's own claim needs no marker gate"
        );
        // A file that merely contains the words is not a declaration: the
        // signature must be the first 43 bytes.
        let other = tmp.path().join("pretender");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("CACHEDIR.TAG"),
            b"# see Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        assert_eq!(classify_at(tmp.path(), "pretender"), None);
        // Neither is a symlink standing in for the tag.
        let linked = tmp.path().join("linked");
        fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(dir.join("CACHEDIR.TAG"), linked.join("CACHEDIR.TAG")).unwrap();
        assert_eq!(classify_at(tmp.path(), "linked"), None);
    }

    #[test]
    fn shared_cache_outside_repos_is_unowned_once_never_duplicated() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("cache/.cargo-registry/a"), 4096);
        touch(&root.join("cache/.cargo-registry/b"), 4096);

        let worktrees: [(&Path, &str); 0] = [];
        let result = attribute(root, &worktrees, 0);

        let cache_rows: Vec<_> = result
            .unowned
            .iter()
            .filter(|u| u.reason == UnownedReason::SharedCache)
            .collect();
        assert_eq!(
            cache_rows.len(),
            1,
            "the shared cache dir is exactly one row, not one per file: {cache_rows:?}"
        );
        assert_eq!(cache_rows[0].bytes, 8192);
        assert_eq!(result.attributed_total, 0);
    }

    #[test]
    fn permission_denied_dir_is_counted_not_dropped() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        touch(&locked.join("secret"), 4096);
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&locked, perms).unwrap();

        let worktrees: [(&Path, &str); 0] = [];
        let result = attribute(root, &worktrees, 0);

        // Restore permissions so tempdir cleanup can remove it.
        let mut restored = fs::metadata(&locked).unwrap().permissions();
        restored.set_mode(0o755);
        fs::set_permissions(&locked, restored).unwrap();

        // Running as root can still read a 0o000 directory; only assert
        // the guard fires when the OS actually denied the read.
        if result
            .unowned
            .iter()
            .any(|u| u.reason == UnownedReason::PermissionDenied)
        {
            let row = result
                .unowned
                .iter()
                .find(|u| u.reason == UnownedReason::PermissionDenied)
                .unwrap();
            assert_eq!(row.bytes, 0);
            assert_eq!(row.path_or_object, locked.display().to_string());
        }
    }
}
