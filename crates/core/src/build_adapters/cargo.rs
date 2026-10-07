//! Cargo build-layout identification, on the [`BuildAdapter`] trait.
//!
//! This is the existing Cargo identification, ported rather than
//! rewritten: the unit ids, the evidence source strings
//! (`cargo-folded-v2`) and the role assignments are the ones a stored
//! report already holds, so a report written before the port replays
//! after it.
//!
//! What the port changed is the *shape*, and every change is one of the
//! section 18 guardrails:
//!
//! * the recursive `fs::read_dir` descent became folded walk rows plus
//!   capped `shallow_list` calls for the three directories that
//!   genuinely need one (a profile, an `examples/`, a `.fingerprint/`);
//! * `fs::read_to_string` on fingerprint JSON became
//!   [`bounded_io::read_manifest`];
//! * the units are built through [`NestedUnitBuilder`].
//!
//! Cargo's build directory is documented as an implementation detail
//! (<https://doc.rust-lang.org/cargo/reference/build-cache.html>), so
//! every claim here rests on path layout and on companion metadata
//! Cargo itself wrote. Nothing invokes Cargo, reads a build script, or
//! executes project code.

use super::{
    BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, ConfiguredOutput, NestedUnitBuilder,
};
use crate::artifact::{
    ArtifactRole, ArtifactVariant, NestedArtifact, architecture_from_target, relative_path,
};
use crate::entities::Confidence;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const LABEL: &str = "cargo";

pub struct Adapter;

impl BuildAdapter for Adapter {
    fn id(&self) -> &'static str {
        "cargo"
    }

    fn name(&self) -> &'static str {
        "Rust / Cargo"
    }

    fn is_project_root(&self, path: &Path) -> bool {
        crate::fs_gate::is_file(path.join("Cargo.toml"))
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            // Cargo's registry/git caches are separate detector-resolved
            // locations, not something this adapter decomposes.
            identifies_shared_stores: true,
            // A fingerprint names the target; the package identity comes
            // from the checkout, not from the artifact.
            attributes_package_identity: true,
            actions_available: false,
        }
    }

    fn store_kinds(&self) -> &'static [crate::locations::BuildStoreKind] {
        &[crate::locations::BuildStoreKind::GenericCacheBuildOutputs]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        if !crate::fs_gate::is_file(project_root.join("Cargo.toml")) {
            return Vec::new();
        }
        // The configured build directories, which may be outside the
        // checkout entirely (a shared target dir is the usual reason a
        // Cargo directory gets large enough to ask about).
        let layout = crate::cargo_artifacts::layout_for(project_root);
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "cargo",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        for p in [layout.target_dir.clone(), layout.build_dir.clone()]
            .into_iter()
            .flatten()
        {
            claim(p);
        }
        for c in candidates {
            if c.file_name().and_then(|n| n.to_str()) == Some("target") {
                claim(c.clone());
            }
        }
        out
    }

    fn configured_outputs(&self, project_root: &Path) -> Vec<ConfiguredOutput> {
        if !self.is_project_root(project_root) {
            return Vec::new();
        }
        crate::cargo_artifacts::declared_build_paths(project_root)
            .into_iter()
            .map(|(path, evidence)| ConfiguredOutput {
                adapter_id: self.id().to_string(),
                path: super::configured_outputs::lexical_absolute(&path, project_root),
                project_root: project_root.to_path_buf(),
                evidence,
            })
            .collect()
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        let root = &container.path;
        let rows = ctx.folded().under(root);
        if container.store_kind == Some(crate::locations::BuildStoreKind::GenericCacheBuildOutputs)
            && !cargo_target_has_layout(root, &rows)
        {
            let mut candidates = nested_cargo_targets(root, &rows);
            candidates.sort();
            let mut units = Vec::new();
            for target in candidates {
                let child = BuildContainer::shared_store("cargo", target);
                for mut unit in self.identify(&child, ctx) {
                    // The generic cache root owns the one replay window and
                    // persisted adapter result. The Cargo target path remains
                    // the physical location of each interior unit.
                    unit.container_id = Some(container.scope());
                    if let Some(consequence) = &mut unit.consequence {
                        consequence.push_str(
                            "; the layout alone does not establish which project or exact build command recreates this target",
                        );
                    }
                    units.push(unit);
                }
            }
            if units.is_empty() {
                return Vec::new();
            }
            // The renderers group interiors beneath their persisted
            // container row. This measured, explicitly unsupported root is
            // a display/accounting anchor for the mixed cache; Cargo evidence
            // below it does not establish that the entire cache is a Cargo
            // output or that one build command can recreate it.
            units.push(
                NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Container)
                    .unsupported_layout(
                        "only evidenced Cargo descendants are classified; this generic cache may contain unrelated data",
                    )
                    .build(),
            );
            return units;
        }
        let measured = rows;
        if measured.is_empty() {
            // Not "nothing here": a container the walk did not measure
            // is a container swamp cannot explain, and saying so is the
            // difference between an empty target dir and an unread one.
            return vec![
                NestedUnitBuilder::new(container, ArtifactRole::Container, root.clone())
                    .is_dir(true)
                    .unsupported_layout(
                        "no measured directories under this Cargo build root; its interior was \
                         not observed this pass",
                    )
                    .build(),
            ];
        }

        let mut units: Vec<NestedArtifact> = Vec::new();
        let mut fingerprint_dirs = Vec::new();
        let mut example_dirs = Vec::new();
        let mut profile_dirs = Vec::new();

        for d in &measured {
            let rel = relative_path(root, &d.path);
            let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let offset = usize::from(parts.first().is_some_and(|p| looks_like_target_triple(p)));
            if parts.len() == offset + 1 {
                profile_dirs.push(d.path.clone());
            }
            if parts.len() == offset + 3 && parts.get(offset + 1) == Some(&".fingerprint") {
                fingerprint_dirs.push(d.path.clone());
            }
            if parts.len() == offset + 2 && parts.get(offset + 1) == Some(&"examples") {
                example_dirs.push(d.path.clone());
            }
            // Keep the shallow structure: the container, its profiles,
            // their immediate category directories, and one level into
            // `incremental`/`build` where the per-crate directories are
            // the unit a person would act on. Everything deeper stays
            // folded -- a `deps/` holding 250k files is one row.
            let keep = parts.len() <= offset + 2
                || (parts.len() == offset + 3
                    && matches!(parts.get(offset + 1), Some(&"incremental" | &"build")));
            if !keep {
                continue;
            }
            let (role, variant) = classify_path(&rel, true);
            units.push(
                NestedUnitBuilder::new(container, role.clone(), d.path.clone())
                    .folded(d)
                    .variant(variant)
                    .supported_with_reason(
                        "directory layout documented in the Cargo build-cache reference",
                    )
                    .evidence(
                        "cargo-folded-v2",
                        "directory aggregates; internal files are not retained",
                        Confidence::High,
                    )
                    .limit(
                        "internal file history and subgroup hardlink attribution are not retained",
                    )
                    .consequence(consequence_for(&role))
                    .build(),
            );
        }

        // A folded group's last change includes its deeper directories.
        // Reuse their measured metadata; never stat files to rebuild an
        // exact inventory.
        let indexes: HashMap<PathBuf, usize> = units
            .iter()
            .enumerate()
            .map(|(i, u)| (u.path.clone(), i))
            .collect();
        for d in &measured {
            for ancestor in d.path.ancestors().take_while(|p| p.starts_with(root)) {
                if let Some(&i) = indexes.get(ancestor) {
                    units[i] = NestedUnitBuilder::amend(units[i].clone())
                        .modified_at_least(d.mtime_max)
                        .complete_only_if(d.complete)
                        .build();
                }
            }
        }

        // Fingerprints give a baseline test/executable distinction
        // without running Cargo. Each names one target; the executable
        // it describes lives in `deps/` under the same hash.
        let mut fingerprint_files = Vec::new();
        for dir in fingerprint_dirs {
            for entry in ctx.list(&dir) {
                let Some(stem) = entry.name.strip_suffix(".json") else {
                    continue;
                };
                if entry.is_dir || !stem.starts_with("test-") {
                    continue;
                }
                let Some(target) = test_target_name(stem) else {
                    continue;
                };
                let Some((_, hash)) = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.rsplit_once('-'))
                else {
                    continue;
                };
                let Some(profile) = dir.parent().and_then(Path::parent) else {
                    continue;
                };
                let executable = profile.join("deps").join(format!("{target}-{hash}"));
                let path = dir.join(&entry.name);
                if let Some(u) =
                    file_unit(container, ctx, &executable, ArtifactRole::TestExecutable)
                {
                    fingerprint_files.push((executable.clone(), target.to_string(), path.clone()));
                    units.push(u);
                    if let Some(meta) =
                        file_unit(container, ctx, &path, ArtifactRole::CompanionMetadata)
                    {
                        units.push(meta);
                    }
                }
            }
        }

        for dir in example_dirs {
            for entry in ctx.list(&dir) {
                if entry.is_dir {
                    continue;
                }
                if let Some(u) = executable_unit(
                    container,
                    ctx,
                    &dir.join(&entry.name),
                    ArtifactRole::Example,
                ) {
                    units.push(u);
                }
            }
        }

        // Final outputs live immediately inside a profile, not inside
        // `deps`. Shallow even when the target holds millions of files.
        for dir in profile_dirs {
            for entry in ctx.list(&dir) {
                if entry.is_dir {
                    continue;
                }
                let path = dir.join(&entry.name);
                let library = matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rlib" | "a" | "so" | "dylib")
                );
                let u = if library {
                    file_unit(container, ctx, &path, ArtifactRole::FinalOutput)
                } else {
                    executable_unit(container, ctx, &path, ArtifactRole::FinalOutput)
                };
                if let Some(u) = u {
                    units.push(u);
                }
            }
        }

        enrich_fingerprints(container, ctx, &fingerprint_files, &mut units);

        units.retain(|u| {
            u.is_dir
                || matches!(
                    u.role,
                    ArtifactRole::TestExecutable
                        | ArtifactRole::Example
                        | ArtifactRole::FinalOutput
                )
        });
        units.sort_by(|a, b| a.path.cmp(&b.path));
        units.dedup_by(|a, b| a.path == b.path);
        if container.store_kind == Some(crate::locations::BuildStoreKind::GenericCacheBuildOutputs)
        {
            for unit in &mut units {
                if let Some(consequence) = &mut unit.consequence {
                    consequence.push_str(
                        "; the layout alone does not establish which project or exact build command recreates this target",
                    );
                }
            }
        }
        units
    }
}

fn cargo_info_file(root: &Path) -> bool {
    crate::fs_gate::symlink_metadata(root.join(".rustc_info.json"))
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn profile_root_for(target: &Path, profile: &Path) -> bool {
    let relative = relative_path(target, profile);
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    (parts.len() == 1 && matches!(parts[0], "debug" | "release"))
        || (parts.len() == 2
            && looks_like_target_triple(parts[0])
            && matches!(parts[1], "debug" | "release"))
}

fn profile_has_cargo_structure(
    profile: &Path,
    measured_paths: &std::collections::HashSet<PathBuf>,
) -> bool {
    ["deps", ".fingerprint", "build", "incremental"]
        .iter()
        .any(|child| measured_paths.contains(&profile.join(child)))
}

fn cargo_target_has_layout(target: &Path, rows: &[&super::FoldedDir]) -> bool {
    if !cargo_info_file(target) {
        return false;
    }
    let measured_paths: std::collections::HashSet<PathBuf> =
        rows.iter().map(|row| row.path.clone()).collect();
    rows.iter().any(|row| {
        profile_root_for(target, &row.path)
            && profile_has_cargo_structure(&row.path, &measured_paths)
    })
}

/// Targets below a generic cache root are selected only from its measured
/// directory rows. Their names carry no authority; Cargo's own rustc marker
/// and a profile/category layout must both be present.
fn nested_cargo_targets(root: &Path, rows: &[&super::FoldedDir]) -> Vec<PathBuf> {
    let measured_paths: std::collections::HashSet<PathBuf> =
        rows.iter().map(|row| row.path.clone()).collect();
    let mut candidates = std::collections::BTreeSet::new();
    for row in rows {
        if !row
            .path
            .file_name()
            .is_some_and(|name| matches!(name.to_str(), Some("debug" | "release")))
        {
            continue;
        }
        let profile = row.path.clone();
        if !profile_has_cargo_structure(&profile, &measured_paths) {
            continue;
        }
        let profile_parent = profile.parent().unwrap_or(root);
        if profile_parent != root
            && profile_parent.starts_with(root)
            && profile_root_for(profile_parent, &profile)
            && cargo_info_file(profile_parent)
        {
            // The immediate parent is the target root even when its name
            // happens to resemble a target triple.
            candidates.insert(profile_parent.to_path_buf());
        } else if profile_parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(looks_like_target_triple)
            && let Some(target) = profile_parent.parent()
            && target != root
            && target.starts_with(root)
            && profile_root_for(target, &profile)
            && cargo_info_file(target)
        {
            candidates.insert(target.to_path_buf());
        }
    }
    candidates.into_iter().collect()
}

/// What removing a unit in this role would cost, in Cargo's own terms.
/// A consequence, never a verdict: it says what happens if the bytes go,
/// not whether they should.
fn consequence_for(role: &ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::Dependency => {
            "the next `cargo build` recompiles these dependencies from the already-downloaded \
             sources"
        }
        ArtifactRole::Incremental => {
            "the next `cargo build` in this profile is a full rebuild rather than an incremental \
             one"
        }
        ArtifactRole::BuildScriptOutput => {
            "the next `cargo build` re-runs this crate's build script"
        }
        ArtifactRole::TestExecutable => "the next `cargo test` relinks this test binary",
        ArtifactRole::Example => "the next `cargo build --examples` relinks this example",
        ArtifactRole::FinalOutput => "the next `cargo build` relinks this output",
        ArtifactRole::CompanionMetadata => {
            "Cargo rewrites this metadata on the next build of the target it describes"
        }
        ArtifactRole::Profile | ArtifactRole::Container => {
            "the next `cargo build` in this profile rebuilds everything it holds"
        }
        _ => "the next `cargo build` regenerates what it needs from source",
    }
}

fn test_target_name(stem: &str) -> Option<&str> {
    stem.strip_prefix("test-lib-")
        .or_else(|| stem.strip_prefix("test-bin-"))
        .or_else(|| stem.strip_prefix("test-integration-test-"))
}

fn file_unit(
    container: &BuildContainer,
    ctx: &BuildCtx,
    path: &Path,
    role: ArtifactRole,
) -> Option<NestedArtifact> {
    let meta = ctx.stat(path)?;
    if !meta.is_file() {
        return None;
    }
    Some(build_file_unit(container, path, &meta, role))
}

fn executable_unit(
    container: &BuildContainer,
    ctx: &BuildCtx,
    path: &Path,
    role: ArtifactRole,
) -> Option<NestedArtifact> {
    use crate::fs_gate::PermissionsExt;
    let meta = ctx.stat(path)?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some(build_file_unit(container, path, &meta, role))
}

fn build_file_unit(
    container: &BuildContainer,
    path: &Path,
    meta: &crate::fs_gate::Metadata,
    role: ArtifactRole,
) -> NestedArtifact {
    use crate::fs_gate::MetadataExt;
    let rel = relative_path(&container.path, path);
    let (_, variant) = classify_path(&rel, false);
    let b = NestedUnitBuilder::new(container, role.clone(), path.to_path_buf())
        .from_file_metadata(meta)
        .variant(variant)
        .supported_with_reason("named output position in a Cargo profile directory")
        .evidence(
            "cargo-folded-v2",
            "directory aggregates; internal files are not retained",
            Confidence::High,
        )
        .limit("internal file history and subgroup hardlink attribution are not retained")
        .consequence(consequence_for(&role));
    // A hardlinked member is charged to exactly one unit, and swamp
    // cannot tell from here which one that is. `from_file_metadata`
    // already recorded `SharedHardlink` membership for it; charging it
    // here would inflate the container's total by however many links
    // exist.
    if meta.nlink() == 1 {
        b.charged_uniquely().build()
    } else {
        b.no_action_because(
            "this file has other hardlinks; how much space its removal frees is unknown",
        )
        .build()
    }
}

/// Reads the `test-*` fingerprint JSON Cargo wrote beside each test
/// build and attaches the target/features/toolchain it records.
///
/// This is *current* evidence about an observed build, not permanent
/// identity: a unit whose fingerprint is gone loses the enrichment and
/// falls back to `Dependency`, which is what an unenriched file in
/// `deps/` is.
fn enrich_fingerprints(
    container: &BuildContainer,
    ctx: &BuildCtx,
    fingerprints: &[(PathBuf, String, PathBuf)],
    units: &mut [NestedArtifact],
) {
    let mut facts: HashMap<&PathBuf, (&String, &PathBuf)> = HashMap::new();
    for (executable, target, json) in fingerprints {
        facts.insert(executable, (target, json));
    }
    for u in units.iter_mut() {
        let Some((target, json_path)) = facts.get(&u.path) else {
            continue;
        };
        let amend = NestedUnitBuilder::amend(u.clone());
        let Some(manifest) = ctx.manifest(json_path) else {
            *u = amend
                .limit("this target's fingerprint metadata could not be read")
                .build();
            continue;
        };
        if manifest.truncated {
            *u = amend
                .limit(
                    "this target's fingerprint metadata is larger than the manifest cap and was \
                     not parsed",
                )
                .build();
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&manifest.text) else {
            *u = amend
                .limit("this target's fingerprint metadata is not valid JSON")
                .build();
            continue;
        };
        let mut variant = u.variant.clone();
        variant.target = Some((*target).clone());
        variant.features = json.get("features").map(|v| v.to_string());
        variant.toolchain = json.get("rustc").map(|v| format!("fingerprint:{v}"));
        variant
            .unknowns
            .retain(|s| s != "features" && s != "toolchain");
        *u = amend
            .role(ArtifactRole::TestExecutable)
            .variant(variant)
            .evidence(
                "cargo-fingerprint",
                json_path.display().to_string(),
                Confidence::Medium,
            )
            .action_group_id(NestedArtifact::storage_id(
                &container.path,
                &format!("action:{}", u.relative_path),
            ))
            .build();
    }
}

// ---------------------------------------------------------------------
// Path classification, shared with the legacy direct-inspection API
// ---------------------------------------------------------------------

/// Cargo's layout, as a role and a variant.
///
/// A variant is what stops `debug/` under `aarch64-apple-darwin/` and
/// `debug/` at the root collapsing into one row: the target triple is
/// carried as the configuration and the architecture, so two builds of
/// the same profile for two triples stay distinguishable.
pub(crate) fn classify_path(rel: &str, is_dir: bool) -> (ArtifactRole, ArtifactVariant) {
    if rel.is_empty() {
        return (ArtifactRole::Container, ArtifactVariant::default());
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let triple = looks_like_target_triple(parts[0]);
    let offset = usize::from(triple);
    let mut variant = profile_variant(parts.get(offset).copied().unwrap_or("unknown"));
    if triple {
        variant.architecture = architecture_from_target(parts[0]);
        variant.configuration = Some(parts[0].into());
        variant.unknowns.retain(|s| s != "architecture");
    }
    if parts.len() <= offset {
        return (ArtifactRole::Container, variant);
    }
    if parts.len() == offset + 1 && is_dir {
        return (ArtifactRole::Profile, variant);
    }
    let role = match parts.get(offset + 1).copied() {
        Some("deps") => ArtifactRole::Dependency,
        Some("examples") => ArtifactRole::Example,
        Some("incremental") => ArtifactRole::Incremental,
        Some("build") => ArtifactRole::BuildScriptOutput,
        Some(".fingerprint") => ArtifactRole::CompanionMetadata,
        _ if !is_dir && parts.len() == offset + 2 => ArtifactRole::FinalOutput,
        _ => ArtifactRole::Residual,
    };
    let role = if !is_dir && rel.ends_with(".d") {
        ArtifactRole::CompanionMetadata
    } else {
        role
    };
    (role, variant)
}

pub(crate) fn profile_variant(profile: &str) -> ArtifactVariant {
    let mut v = ArtifactVariant::default();
    if profile != "unknown" {
        v.profile = Some(profile.into());
    } else {
        v.unknowns.push("profile".into());
    }
    v.unknowns.extend(
        ["architecture", "toolchain", "features", "generation"]
            .into_iter()
            .map(String::from),
    );
    v
}

pub(crate) fn looks_like_target_triple(name: &str) -> bool {
    name.matches('-').count() >= 2 && !name.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{Membership, TimeSource};
    use crate::build_adapters::{ContainerCache, FoldedDir, FoldedIndex};
    use crate::fs_events::EventCoverage;
    use std::fs;

    fn folded(paths: &[(PathBuf, u64, u64)]) -> FoldedIndex {
        FoldedIndex::from_dirs(paths.iter().map(|(p, b, m)| FoldedDir {
            path: p.clone(),
            allocated_total: *b,
            mtime_max: *m,
            complete: true,
        }))
    }

    fn run(container: &BuildContainer, index: &FoldedIndex) -> Vec<NestedArtifact> {
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let ctx = BuildCtx::new(1_000, index, &none, &cache);
        Adapter.identify(container, &ctx)
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &FoldedIndex::default());
        assert_eq!(units.len(), 1, "an unmeasured container is still a row");
        assert!(!units[0].coverage.supported);
        assert!(
            units[0]
                .coverage
                .limits
                .iter()
                .any(|l| l.contains("not observed")),
            "the limit names why, rather than the row simply being absent: {:?}",
            units[0].coverage.limits
        );
    }

    #[test]
    fn cargo_identified_profile_children_fill_generic_build_families() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let debug = target.join("debug");
        fs::create_dir_all(debug.join("deps")).unwrap();
        fs::create_dir_all(debug.join("incremental")).unwrap();
        fs::create_dir_all(debug.join("unrecognized")).unwrap();
        let index = folded(&[
            (target.clone(), 30_000_000_000, 500),
            (debug.clone(), 29_999_991_808, 500),
            (debug.join("deps"), 28_000_000_000, 500),
            (debug.join("incremental"), 1_500_000_000, 500),
            (debug.join("unrecognized"), 8_192, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index);
        let summary = crate::build_adapters::summarize_container(&target, &units);

        let dependencies = summary
            .families
            .iter()
            .find(|f| f.family == crate::artifact::RoleFamily::Dependencies)
            .expect("Cargo's deps directory should be visible in the generic summary");
        let intermediates = summary
            .families
            .iter()
            .find(|f| f.family == crate::artifact::RoleFamily::Intermediates)
            .expect("Cargo's incremental directory should be visible in the generic summary");
        let residual = summary
            .families
            .iter()
            .find(|f| f.family == crate::artifact::RoleFamily::Residual)
            .expect("unclassified but supported Cargo directories remain Not identified");
        assert_eq!(dependencies.bytes, 28_000_000_000);
        assert_eq!(dependencies.recommendation, "Review: rebuild dependencies");
        assert_eq!(intermediates.bytes, 1_500_000_000);
        assert_eq!(residual.bytes, 8_192);
        assert_eq!(summary.unsupported_count, 0);
        assert_eq!(summary.unaccounted_bytes, Some(499_991_808));
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let fp = target.join("debug/.fingerprint/thing-abc123");
        fs::create_dir_all(&fp).unwrap();
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        // A fingerprint far larger than any real one, and larger than
        // the cap: identification must stop at the cap, not read it all.
        fs::write(
            fp.join("test-lib-thing.json"),
            format!(
                "{{\"rustc\":1,\"features\":\"[]\",\"pad\":\"{}\"}}",
                "x".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES * 2)
            ),
        )
        .unwrap();
        fs::write(target.join("debug/deps/thing-abc123"), b"exe").unwrap();
        let index = folded(&[
            (target.clone(), 100, 500),
            (target.join("debug"), 100, 500),
            (target.join("debug/deps"), 50, 500),
            (target.join("debug/.fingerprint"), 10, 500),
            (fp.clone(), 10, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let (_units, counted) = crate::work_counters::measured(|| run(&c, &index));
        assert!(
            counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64,
            "identification read {} bytes; the cap is {}",
            counted.header_bytes_read,
            super::super::bounded_io::MAX_MANIFEST_BYTES
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join("debug/build/thing-abc/out")).unwrap();
        // A build script output directory holding an executable named
        // exactly what an adapter might be tempted to run.
        let script = target.join("debug/build/thing-abc/build-script-build");
        fs::write(&script, b"#!/bin/sh\nexit 3\n").unwrap();
        let index = folded(&[
            (target.clone(), 100, 500),
            (target.join("debug"), 100, 500),
            (target.join("debug/build"), 50, 500),
            (target.join("debug/build/thing-abc"), 50, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let (_units, counted) = crate::work_counters::measured(|| run(&c, &index));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "identification spawned a process; Cargo is never invoked to identify its own output"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // Two `debug` profiles, one per target triple. A filename-only
        // grouper reports one `debug`; the variant keeps them apart.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let host = target.join("debug");
        let cross = target.join("aarch64-unknown-linux-gnu/debug");
        fs::create_dir_all(&host).unwrap();
        fs::create_dir_all(&cross).unwrap();
        let index = folded(&[
            (target.clone(), 200, 500),
            (host.clone(), 100, 500),
            (target.join("aarch64-unknown-linux-gnu"), 100, 500),
            (cross.clone(), 100, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index);
        let host_u = units.iter().find(|u| u.path == host).expect("host debug");
        let cross_u = units.iter().find(|u| u.path == cross).expect("cross debug");
        assert_ne!(host_u.id, cross_u.id, "two profiles, two identities");
        assert_eq!(host_u.variant.profile.as_deref(), Some("debug"));
        assert_eq!(cross_u.variant.profile.as_deref(), Some("debug"));
        assert_eq!(
            cross_u.variant.configuration.as_deref(),
            Some("aarch64-unknown-linux-gnu"),
            "the triple is what distinguishes them, and it is recorded"
        );
        assert_eq!(host_u.variant.configuration, None);
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        let index = folded(&[(target.clone(), 100, 1), (target.join("debug"), 100, 1)]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index);
        let old = units
            .iter()
            .find(|u| u.path == target.join("debug"))
            .unwrap();
        assert_eq!(
            old.time_source,
            TimeSource::FoldedDirectoryModification,
            "the timestamp is labelled as a modification fact, from a stated source"
        );
        assert_eq!(
            old.action,
            crate::artifact::NestedActionCapability::InspectionOnly,
            "a very old unit is still only inspectable; age offers no action"
        );
        // The verdict-vocabulary scan over every string this unit can
        // put in front of a person lives in
        // `crates/core/tests/build_adapter_contract.rs::no_unit_renders_a_verdict`.
        // It is not repeated here, because spelling the banned words in
        // `crates/core/src` is itself what `scripts/check.sh`'s grep
        // audit rejects -- and a weaker copy of a stronger check is not
        // worth the exemption.
    }

    #[test]
    fn a_hardlinked_output_is_not_charged_and_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let debug = target.join("debug");
        fs::create_dir_all(debug.join("deps")).unwrap();
        let a = debug.join("app");
        fs::write(&a, b"binary").unwrap();
        fs::hard_link(&a, debug.join("deps/app-123")).unwrap();
        let mut perms = fs::metadata(&a).unwrap().permissions();
        use crate::fs_gate::PermissionsExt;
        perms.set_mode(0o755);
        fs::set_permissions(&a, perms).unwrap();
        let index = folded(&[
            (target.clone(), 100, 500),
            (debug.clone(), 100, 500),
            (debug.join("deps"), 50, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index);
        let app = units.iter().find(|u| u.path == a).expect("final output");
        assert_eq!(app.membership, Membership::SharedHardlink);
        assert_eq!(app.physical_total, 0, "a shared member is charged nowhere");
        assert!(matches!(
            app.action,
            crate::artifact::NestedActionCapability::Unsupported { .. }
        ));
    }

    #[test]
    fn a_test_executable_keeps_its_identity_when_the_fingerprint_is_unreadable() {
        // Reclassification must not fabricate anything: a fingerprint
        // that cannot be parsed leaves an explicit limit, not a guess.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let fp = target.join("debug/.fingerprint/thing-abc123");
        fs::create_dir_all(&fp).unwrap();
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::write(fp.join("test-lib-thing.json"), b"not json at all").unwrap();
        fs::write(target.join("debug/deps/thing-abc123"), b"exe").unwrap();
        let index = folded(&[
            (target.clone(), 100, 500),
            (target.join("debug"), 100, 500),
            (target.join("debug/deps"), 50, 500),
            (target.join("debug/.fingerprint"), 10, 500),
            (fp.clone(), 10, 500),
        ]);
        let c = BuildContainer::project("cargo", target.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index);
        let exe = units
            .iter()
            .find(|u| u.path == target.join("debug/deps/thing-abc123"))
            .expect("the executable is still identified");
        assert!(
            exe.coverage
                .limits
                .iter()
                .any(|l| l.contains("not valid JSON")),
            "{:?}",
            exe.coverage.limits
        );
        assert_eq!(
            exe.variant.features, None,
            "no feature set is invented from an unreadable fingerprint"
        );
    }
}
