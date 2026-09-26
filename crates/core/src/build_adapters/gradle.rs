//! Gradle project build directories and shared caches (#67).
//!
//! Gradle's directory layout is documented
//! (<https://docs.gradle.org/current/userguide/directory_layout.html>)
//! and its build directory's interior is conventional: `build/classes`,
//! `build/libs`, `build/tmp`, `build/reports`, `build/test-results`.
//! Everything this adapter claims rests on that convention plus the
//! Gradle user home's own cache layout.
//!
//! # What is never done
//!
//! **Build scripts and plugins are never evaluated.** That is the whole
//! reason a Gradle build directory is hard to explain: `buildDir` can be
//! reassigned in `build.gradle`, and a plugin can write anywhere under
//! it. Running Gradle to ask would execute the repository's own code,
//! so this adapter reports what the conventional layout accounts for and
//! marks the rest an unidentified residual -- a named limitation rather
//! than a silent omission or a guess.
//!
//! # What a cache entry does and does not say
//!
//! A `transforms-*` or `build-cache-*` entry is keyed by a hash whose
//! inputs Gradle does not record on disk. Its modification time says
//! when it was written, which is not when it was last read and not
//! whether any current build would hit it. #67 asks for native cache
//! retention evidence to stay separate from filesystem mtime; where
//! Gradle records none, the unit says the retention basis is unknown
//! rather than borrowing the mtime for the claim.

use super::{BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, NestedUnitBuilder};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// Cache categories inside a Gradle user home.
const HOME_ENTRIES: &[(&str, ArtifactRole, &str, &str)] = &[
    (
        "caches",
        ArtifactRole::SharedStoreEntry,
        "downloaded modules, generated jars and transformed artifacts",
        "the next build re-downloads or regenerates these -- needs network access for the \
         downloaded ones",
    ),
    (
        "wrapper",
        ArtifactRole::SharedStoreEntry,
        "Gradle distributions the wrapper downloaded",
        "the wrapper downloads the distribution again on the next build -- needs network access",
    ),
    (
        "daemon",
        ArtifactRole::Metadata,
        "daemon registry and logs",
        "the next build starts a new daemon and writes a new log",
    ),
    (
        "native",
        ArtifactRole::Intermediate,
        "platform-native helper binaries Gradle unpacked",
        "Gradle unpacks them again on the next build",
    ),
    (
        "jdks",
        ArtifactRole::SharedStoreEntry,
        "JDKs Gradle's toolchain provisioning downloaded",
        "toolchain provisioning downloads the JDK again -- needs network access",
    ),
];

/// Cache subdirectories inside `<gradle-user-home>/caches/`, and whether
/// their entries carry coordinates.
///
/// The key is the name with its trailing layout version removed
/// (`modules-2` -> `modules`, `build-cache-1` -> `build-cache`): the
/// version is data, not part of the category, and keeping it in the key
/// would mean a new Gradle layout generation silently becoming an
/// unidentified residual.
const CACHE_CATEGORIES: &[(&str, &str, bool)] = &[
    (
        "modules",
        "the dependency cache: one directory per module coordinate",
        true,
    ),
    (
        "transforms",
        "artifact transform outputs, keyed by a hash whose inputs Gradle does not record on disk",
        false,
    ),
    (
        "jars",
        "instrumented and relocated jars Gradle generated",
        false,
    ),
    (
        "build-cache",
        "task outputs, keyed by a cache key whose inputs Gradle does not record on disk",
        false,
    ),
];

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "gradle"
    }

    fn name(&self) -> &'static str {
        "Gradle"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: true,
            actions_available: true,
        }
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        let gradle = [
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]
        .iter()
        .any(|m| crate::fs_gate::is_file(project_root.join(m)));
        if !gradle {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "gradle",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        claim(project_root.join("build"));
        claim(project_root.join(".gradle"));
        for c in candidates {
            if matches!(
                c.file_name().and_then(|n| n.to_str()),
                Some("build") | Some(".gradle")
            ) {
                claim(c.clone());
            }
        }
        out
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[
            BuildStoreKind::GradleCaches,
            BuildStoreKind::GradleWrapperDists,
            BuildStoreKind::GradleDaemon,
            BuildStoreKind::GradleNative,
        ]
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        if container.shared {
            // The detector proposes the user home's parts, not the home:
            // `caches`, `wrapper/dists`, `daemon`, `native`, each its
            // own measured store. A home handed in whole (a fixture, or
            // a caller with no declaration) is still understood.
            return match container.store_kind {
                Some(BuildStoreKind::GradleCaches) => {
                    let mut units = vec![home_part_unit(container, ctx, "caches")];
                    units.extend(identify_caches(container, ctx, &container.path));
                    units
                }
                Some(BuildStoreKind::GradleWrapperDists) => {
                    let mut units = vec![home_part_unit(container, ctx, "wrapper")];
                    units.extend(identify_dists(container, ctx, &container.path));
                    units
                }
                Some(BuildStoreKind::GradleDaemon) => {
                    let mut units = vec![home_part_unit(container, ctx, "daemon")];
                    units.extend(identify_daemons(container, ctx, &container.path));
                    units
                }
                Some(BuildStoreKind::GradleNative) => {
                    vec![home_part_unit(container, ctx, "native")]
                }
                _ => identify_user_home(container, ctx),
            };
        }
        match container.path.file_name().and_then(|n| n.to_str()) {
            Some(".gradle") => identify_project_cache(container, ctx),
            _ => identify_build_dir(container, ctx),
        }
    }
}

fn root_unit(
    container: &BuildContainer,
    ctx: &BuildCtx,
    role: ArtifactRole,
    reason: &str,
    consequence: &str,
) -> NestedArtifact {
    let mut b = NestedUnitBuilder::new(container, role, container.path.clone())
        .is_dir(true)
        .supported_with_reason(reason)
        .evidence("gradle-layout", reason.to_string(), Confidence::Medium)
        .consequence(consequence);
    match ctx.folded().get(&container.path) {
        Some(d) => b = b.folded(d),
        None => b = b.limit("this directory was not measured by the walk this pass"),
    }
    b.build()
}

fn identify_build_dir(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![root_unit(
        container,
        ctx,
        ArtifactRole::Output,
        "a Gradle project build directory, in the conventional layout",
        "the next `gradle build` regenerates everything it holds",
    )];
    for child in ctx.folded().children(&container.path) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let known = crate::build_adapters::jvm_common::GRADLE_BUILD_ENTRIES
            .iter()
            .find(|(n, _, _)| *n == name);
        let mut b = NestedUnitBuilder::new(
            container,
            known
                .map(|(_, r, _)| r.clone())
                .unwrap_or(ArtifactRole::Residual),
            child.path.clone(),
        )
        .folded(child);
        b = match known {
            Some((_, _, consequence)) => b
                .supported_with_reason(format!(
                    "`build/{name}` is part of Gradle's conventional build layout"
                ))
                .evidence(
                    "gradle-layout",
                    format!("conventional build/{name}"),
                    Confidence::Medium,
                )
                .consequence(*consequence),
            None => b.unsupported_layout(format!(
                "`build/{name}` is not part of the conventional layout; a build script or plugin \
                 chose it, and swamp never evaluates either"
            )),
        };
        units.push(b.build());
    }
    units
}

fn identify_project_cache(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![root_unit(
        container,
        ctx,
        ArtifactRole::Intermediate,
        "`.gradle/` is the project-local cache Gradle keeps between builds",
        "the next build re-reads the build scripts and re-derives its task history; the build \
         itself is unaffected",
    )];
    for child in ctx.folded().children(&container.path) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // A `.gradle/<version>` directory is a per-Gradle-version cache,
        // and its version is what stops two of them collapsing.
        let mut variant = ArtifactVariant::default();
        let versioned = crate::build_adapters::jvm_common::looks_like_version(&name);
        if versioned {
            variant.toolchain = Some(format!("gradle {name}"));
        } else {
            variant.unknowns.push("toolchain".into());
        }
        units.push(
            NestedUnitBuilder::new(container, ArtifactRole::Intermediate, child.path.clone())
                .folded(child)
                .variant(variant)
                .supported_with_reason(if versioned {
                    format!("`.gradle/{name}` is the project cache for Gradle {name}")
                } else {
                    format!("`.gradle/{name}` is a project-cache subdirectory")
                })
                .evidence(
                    "gradle-layout",
                    format!("project cache entry {name}"),
                    Confidence::Medium,
                )
                .consequence("the next build with this Gradle version re-derives it")
                .build(),
        );
    }
    units
}

/// The root unit of one part of a Gradle user home the detector measured
/// as its own store, described by the same table the whole-home
/// identification uses.
fn home_part_unit(container: &BuildContainer, ctx: &BuildCtx, part: &str) -> NestedArtifact {
    let known = HOME_ENTRIES.iter().find(|(n, _, _, _)| *n == part);
    let role = known
        .map(|(_, r, _, _)| r.clone())
        .unwrap_or(ArtifactRole::Residual);
    let mut b = NestedUnitBuilder::new(container, role, container.path.clone())
        .is_dir(true)
        .membership(Membership::Unknown)
        .no_action_because("this directory is shared by every Gradle project on this machine");
    b = match known {
        Some((_, _, what, consequence)) => b
            .supported_with_reason(format!("the Gradle user home's `{part}/` holds {what}"))
            .evidence(
                "gradle-layout",
                format!("{part}: {what}"),
                Confidence::Medium,
            )
            .consequence(*consequence),
        None => b.unsupported_layout(format!(
            "`{part}/` is not a Gradle user-home directory this adapter identifies"
        )),
    };
    match ctx.folded().get(&container.path) {
        Some(d) => b.folded(d),
        None => b.limit("this directory was not measured by the walk this pass"),
    }
    .build()
}

/// A Gradle user home (`~/.gradle` or `GRADLE_USER_HOME`), resolved by
/// the caller and handed in as a shared container.
fn identify_user_home(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut root = NestedUnitBuilder::new(
        container,
        ArtifactRole::SharedStoreEntry,
        container.path.clone(),
    )
    .is_dir(true)
    .supported_with_reason("a Gradle user home shared by every Gradle build on this machine")
    .evidence("gradle-layout", "gradle user home", Confidence::Medium)
    .consequence(
        "the next build re-downloads modules and distributions it needs -- needs network access",
    )
    .no_action_because("this home is shared by every Gradle project on this machine");
    root = match ctx.folded().get(&container.path) {
        Some(d) => root.folded(d),
        None => root.limit("this Gradle user home was not measured by the walk this pass"),
    };
    let mut units = vec![root.build()];
    for child in ctx.folded().children(&container.path) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let known = HOME_ENTRIES.iter().find(|(n, _, _, _)| *n == name);
        let mut b = NestedUnitBuilder::new(
            container,
            known
                .map(|(_, r, _, _)| r.clone())
                .unwrap_or(ArtifactRole::Residual),
            child.path.clone(),
        )
        .folded(child)
        .membership(Membership::Unknown);
        b = match known {
            Some((_, _, what, consequence)) => b
                .supported_with_reason(format!("`{name}/` holds {what}"))
                .evidence(
                    "gradle-layout",
                    format!("{name}: {what}"),
                    Confidence::Medium,
                )
                .consequence(*consequence)
                .no_action_because("this directory is shared by every Gradle project"),
            None => b.unsupported_layout(format!(
                "`{name}/` is not a Gradle user-home directory this adapter identifies"
            )),
        };
        units.push(b.build());
        if name == "caches" {
            units.extend(identify_caches(container, ctx, &child.path));
        }
        if name == "wrapper" {
            units.extend(identify_wrapper(container, ctx, &child.path));
        }
        if name == "daemon" {
            units.extend(identify_daemons(container, ctx, &child.path));
        }
    }
    units
}

fn identify_caches(
    container: &BuildContainer,
    ctx: &BuildCtx,
    caches: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for child in ctx.folded().children(caches) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // `modules-2`, `transforms-3`, `jars-9`, `build-cache-1`: the
        // trailing number is the cache's own layout version, and it is
        // what keeps two generations of the same cache distinct instead
        // of collapsing them under one name.
        let (prefix, layout_version) = split_versioned(&name);
        let known = CACHE_CATEGORIES.iter().find(|(p, _, _)| *p == prefix);
        let mut variant = ArtifactVariant::default();
        match layout_version {
            Some(v) => variant.configuration = Some(format!("{prefix} layout {v}")),
            None => variant.unknowns.push("configuration".into()),
        }
        let mut b = NestedUnitBuilder::new(
            container,
            ArtifactRole::SharedStoreEntry,
            child.path.clone(),
        )
        .folded(child)
        .variant(variant)
        .no_action_because("a shared cache entry may be in use by any Gradle build");
        b = match known {
            Some((_, what, _)) => b
                .supported_with_reason(format!("`caches/{name}` is {what}"))
                .evidence(
                    "gradle-layout",
                    format!("cache category {prefix}"),
                    Confidence::Medium,
                )
                .limit(
                    "Gradle records no retention or last-read evidence for a cache entry on disk; \
                     the timestamp is a modification time only",
                )
                .consequence(
                    "the next build re-downloads or regenerates these entries -- the downloaded \
                     ones need network access",
                ),
            None => b.unsupported_layout(format!(
                "`caches/{name}` is not a cache category this adapter identifies"
            )),
        };
        units.push(b.build());
        if prefix == "modules" {
            units.extend(identify_modules(container, ctx, &child.path));
        }
    }
    units
}

/// `modules-2/files-2.1/<group>/<artifact>/<version>/` -- the one Gradle
/// cache whose entries carry real coordinates.
fn identify_modules(
    container: &BuildContainer,
    ctx: &BuildCtx,
    modules: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for files in ctx.folded().children(modules) {
        let files_name = files
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if files_name.starts_with("metadata-") {
            // `modules-2/metadata-2.106/`: Gradle's resolution metadata
            // for the cached modules (descriptors, resolved versions),
            // keyed by the metadata format version in its name.
            let variant = ArtifactVariant {
                configuration: Some(format!("module metadata {files_name}")),
                ..Default::default()
            };
            units.push(
                NestedUnitBuilder::new(container, ArtifactRole::Metadata, files.path.clone())
                    .folded(files)
                    .variant(variant)
                    .supported_with_reason(format!(
                        "`modules-2/{files_name}` is the dependency cache's own resolution \
                         metadata"
                    ))
                    .evidence(
                        "gradle-modules",
                        format!("module metadata {files_name}"),
                        Confidence::Medium,
                    )
                    .consequence(
                        "the next build re-resolves module metadata -- needs network access",
                    )
                    .no_action_because("this metadata is shared by every Gradle build")
                    .build(),
            );
            continue;
        }
        if !files_name.starts_with("files-") {
            units.push(
                NestedUnitBuilder::new(container, ArtifactRole::Residual, files.path.clone())
                    .folded(files)
                    .unsupported_layout(format!(
                        "`modules-2/{files_name}` is not a module-cache directory this adapter \
                         identifies"
                    ))
                    .build(),
            );
            continue;
        }
        for group in ctx.folded().children(&files.path) {
            for artifact in ctx.folded().children(&group.path) {
                let rel = relative_path(&files.path, &artifact.path);
                let coords =
                    crate::build_adapters::jvm_common::coordinates_from_relative(&rel, true);
                let mut variant = ArtifactVariant::default();
                match &coords {
                    Some(c) => {
                        variant.package = Some(format!("{}:{}", c.group, c.artifact));
                        variant.version = c.version.clone();
                        if c.version.is_none() {
                            variant.unknowns.push("version".into());
                        }
                    }
                    None => {
                        variant.unknowns.push("package".into());
                        variant.unknowns.push("version".into());
                    }
                }
                let mut b = NestedUnitBuilder::new(
                    container,
                    ArtifactRole::SharedStoreEntry,
                    artifact.path.clone(),
                )
                .folded(artifact)
                .variant(variant)
                .no_action_because(
                    "a cached module may be a dependency of any project on this machine",
                );
                b = match &coords {
                    Some(c) => b
                        .supported_with_reason(format!(
                            "`modules-2` lays modules out by coordinate; this is {}",
                            c.display()
                        ))
                        .evidence("gradle-modules", c.display(), Confidence::High)
                        .consequence(
                            "the next build downloads this module again -- needs network access",
                        ),
                    None => b.unsupported_layout(format!(
                        "`{rel}` is not a coordinate layout this adapter can read"
                    )),
                };
                units.push(b.build());
            }
        }
    }
    units
}

/// `wrapper/dists/<distribution>-<hash>/` -- distributions the wrapper
/// downloaded, each one a full Gradle release.
fn identify_wrapper(
    container: &BuildContainer,
    ctx: &BuildCtx,
    wrapper: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for dists in ctx.folded().children(wrapper) {
        if dists.path.file_name().and_then(|n| n.to_str()) != Some("dists") {
            continue;
        }
        units.extend(identify_dists(container, ctx, &dists.path));
    }
    units
}

/// The distributions inside `wrapper/dists/`, each a full Gradle release.
fn identify_dists(container: &BuildContainer, ctx: &BuildCtx, dists: &Path) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    {
        for dist in ctx.folded().children(dists) {
            let name = dist
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let mut variant = ArtifactVariant::default();
            // `gradle-8.5-bin` names the version; the hash directory
            // below it does not. Two distributions differ by version,
            // never by basename alone.
            match name
                .strip_prefix("gradle-")
                .and_then(|r| r.split('-').next())
            {
                Some(v) if crate::build_adapters::jvm_common::looks_like_version(v) => {
                    variant.toolchain = Some(format!("gradle {v}"));
                }
                _ => variant.unknowns.push("toolchain".into()),
            }
            units.push(
                NestedUnitBuilder::new(
                    container,
                    ArtifactRole::SharedStoreEntry,
                    dist.path.clone(),
                )
                .folded(dist)
                .variant(variant)
                .supported_with_reason(format!(
                    "`wrapper/dists/{name}` is a Gradle distribution the wrapper downloaded"
                ))
                .evidence(
                    "gradle-wrapper",
                    format!("distribution {name}"),
                    Confidence::High,
                )
                .consequence(
                    "a project pinned to this distribution downloads it again on its next build \
                     -- needs network access",
                )
                .no_action_because("any project's wrapper may be pinned to this distribution")
                .build(),
            );
        }
    }
    units
}

/// `daemon/<gradle-version>/`: one Gradle version's daemon registry and
/// logs. The version keeps two daemons' logs distinct; nothing here says
/// which version any project still uses.
fn identify_daemons(
    container: &BuildContainer,
    ctx: &BuildCtx,
    daemon: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for dir in ctx.folded().children(daemon) {
        let name = dir
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let mut variant = ArtifactVariant::default();
        let b =
            NestedUnitBuilder::new(container, ArtifactRole::Metadata, dir.path.clone()).folded(dir);
        let b = if crate::build_adapters::jvm_common::looks_like_version(&name) {
            variant.toolchain = Some(format!("gradle {name}"));
            b.variant(variant)
                .supported_with_reason(format!(
                    "`daemon/{name}` holds the Gradle {name} daemon's registry and logs"
                ))
                .evidence(
                    "gradle-layout",
                    format!("daemon logs for gradle {name}"),
                    Confidence::Medium,
                )
                .consequence(format!(
                    "the next Gradle {name} build starts a new daemon and a new log"
                ))
        } else {
            variant.unknowns.push("toolchain".into());
            b.variant(variant).unsupported_layout(format!(
                "`daemon/{name}` is not a per-version daemon directory"
            ))
        };
        units.push(b.build());
    }
    units
}

/// `modules-2` -> (`modules`, `2`); `build-cache-1` -> (`build-cache`,
/// `1`); `native` -> (`native`, None).
fn split_versioned(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('-') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() => {
            (head, Some(tail))
        }
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::TimeSource;
    use crate::build_adapters::{ContainerCache, FoldedDir, FoldedIndex};
    use crate::fs_events::EventCoverage;
    use std::fs;

    fn index(entries: &[(&PathBuf, u64, u64)]) -> FoldedIndex {
        FoldedIndex::from_dirs(entries.iter().map(|(p, b, m)| FoldedDir {
            path: (*p).clone(),
            allocated_total: *b,
            mtime_max: *m,
            complete: true,
        }))
    }

    fn run(c: &BuildContainer, idx: &FoldedIndex) -> Vec<NestedArtifact> {
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        Adapter.identify(c, &BuildCtx::new(1_000, idx, &none, &cache))
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        let plugin = build.join("jacocoHtml");
        fs::create_dir_all(&plugin).unwrap();
        let c = BuildContainer::project("gradle", build.clone(), tmp.path().to_path_buf());
        let units = run(&c, &index(&[(&build, 100, 500), (&plugin, 60, 500)]));
        let u = units.iter().find(|u| u.path == plugin).expect("a row");
        assert!(!u.coverage.supported);
        assert_eq!(u.role, ArtifactRole::Residual);
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("never evaluates")),
            "the limit says why swamp cannot know: {:?}",
            u.coverage.limits
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        // Gradle identification is layout-only: it reads no manifests at
        // all, and the assertion is that the number stays zero rather
        // than merely under the cap.
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        fs::create_dir_all(build.join("libs")).unwrap();
        fs::write(build.join("libs/app.jar"), vec![b'x'; 300_000]).unwrap();
        let c = BuildContainer::project("gradle", build.clone(), tmp.path().to_path_buf());
        let idx = index(&[(&build, 100, 500), (&build.join("libs"), 90, 500)]);
        let (_u, counted) = crate::work_counters::measured(|| run(&c, &idx));
        assert!(
            counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64,
            "read {} bytes",
            counted.header_bytes_read
        );
        assert_eq!(
            counted.header_bytes_read, 0,
            "Gradle identification is layout-only and reads no file contents"
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("build.gradle"),
            b"task boom { throw new Exception() }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("build/classes")).unwrap();
        // The wrapper script, which is what an adapter would run.
        fs::write(root.join("gradlew"), b"#!/bin/sh\nexit 9\n").unwrap();
        let c = BuildContainer::project("gradle", root.join("build"), root.to_path_buf());
        let idx = index(&[
            (&root.join("build"), 100, 500),
            (&root.join("build/classes"), 90, 500),
        ]);
        let (_u, counted) = crate::work_counters::measured(|| run(&c, &idx));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "identification spawned a process; Gradle and its wrapper are never invoked"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // Two cache layout generations, and two wrapper distributions.
        // Both differ only in a suffix, and neither may collapse.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("gradle-home");
        let caches = home.join("caches");
        let m2 = caches.join("modules-2");
        let t3 = caches.join("transforms-3");
        let t4 = caches.join("transforms-4");
        for p in [&m2, &t3, &t4] {
            fs::create_dir_all(p).unwrap();
        }
        let c = BuildContainer::shared_store("gradle", home.clone());
        let idx = index(&[
            (&home, 1_000, 500),
            (&caches, 900, 500),
            (&m2, 400, 500),
            (&t3, 250, 500),
            (&t4, 250, 500),
        ]);
        let units = run(&c, &idx);
        let a = units.iter().find(|u| u.path == t3).expect("transforms-3");
        let b = units.iter().find(|u| u.path == t4).expect("transforms-4");
        assert_ne!(a.id, b.id);
        assert_eq!(
            a.variant.configuration.as_deref(),
            Some("transforms layout 3")
        );
        assert_eq!(
            b.variant.configuration.as_deref(),
            Some("transforms layout 4")
        );
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("gradle-home");
        let caches = home.join("caches");
        let m2 = caches.join("modules-2");
        fs::create_dir_all(&m2).unwrap();
        let c = BuildContainer::shared_store("gradle", home.clone());
        let units = run(
            &c,
            &index(&[(&home, 1_000, 1), (&caches, 900, 1), (&m2, 400, 1)]),
        );
        let u = units.iter().find(|u| u.path == m2).unwrap();
        assert_eq!(u.time_source, TimeSource::FoldedDirectoryModification);
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("no retention or last-read evidence")),
            "a very old cache entry says what its timestamp does *not* establish: {:?}",
            u.coverage.limits
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
    fn a_missing_gradle_marker_claims_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("build")).unwrap();
        assert!(
            Adapter.containers(tmp.path(), &[]).is_empty(),
            "a `build/` with no Gradle marker beside it is not Gradle's"
        );
    }

    #[test]
    fn two_gradle_versions_project_caches_stay_distinct() {
        // `.gradle/8.5` and `.gradle/8.9` are two tool versions' caches
        // side by side. Neither supersedes the other, and the version is
        // what keeps them apart.
        let tmp = tempfile::tempdir().unwrap();
        let dot = tmp.path().join(".gradle");
        let a = dot.join("8.5");
        let b = dot.join("8.9");
        for p in [&a, &b] {
            fs::create_dir_all(p).unwrap();
        }
        let c = BuildContainer::project("gradle", dot.clone(), tmp.path().to_path_buf());
        let units = run(
            &c,
            &index(&[(&dot, 600, 500), (&a, 300, 400), (&b, 300, 900)]),
        );
        let ua = units.iter().find(|u| u.path == a).expect("8.5");
        let ub = units.iter().find(|u| u.path == b).expect("8.9");
        assert_ne!(ua.id, ub.id);
        assert_eq!(ua.variant.toolchain.as_deref(), Some("gradle 8.5"));
        assert_eq!(ub.variant.toolchain.as_deref(), Some("gradle 8.9"));
        assert_eq!(
            ua.action,
            crate::artifact::NestedActionCapability::InspectionOnly,
            "the older version's cache gets no different treatment for being older"
        );
    }

    #[test]
    fn an_incompletely_measured_build_directory_says_so_and_stays_identified() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        fs::create_dir_all(&build).unwrap();
        let partial = FoldedIndex::from_dirs([FoldedDir {
            path: build.clone(),
            allocated_total: 4_000,
            mtime_max: 500,
            complete: false,
        }]);
        let units = run(
            &BuildContainer::project("gradle", build.clone(), tmp.path().to_path_buf()),
            &partial,
        );
        let u = &units[0];
        assert!(!u.coverage.complete);
        assert!(u.coverage.supported, "incomplete is not unsupported");
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("could not read all of this directory")),
            "{:?}",
            u.coverage.limits
        );
    }

    #[test]
    fn a_custom_gradle_user_home_is_identified_by_its_layout_not_its_name() {
        // `GRADLE_USER_HOME=/opt/ci/gradle-cache`: the adapter never
        // reads the variable (the detector resolves the home and hands
        // it in), and nothing in identification depends on the home
        // being called `.gradle`.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("ci/gradle-cache");
        let caches = home.join("caches");
        let modules = caches.join("modules-2");
        let meta = modules.join("metadata-2.106");
        let files = modules.join("files-2.1");
        let group = files.join("org.slf4j");
        let art = group.join("slf4j-api");
        let ver = art.join("2.0.9");
        let daemon = home.join("daemon");
        let d85 = daemon.join("8.5");
        let d89 = daemon.join("8.9");
        let odd = modules.join("journal-1");
        for p in [&meta, &ver, &d85, &d89, &odd] {
            fs::create_dir_all(p).unwrap();
        }
        let c = BuildContainer::shared_store("gradle", home.clone());
        let units = run(
            &c,
            &index(&[
                (&home, 1_000, 500),
                (&caches, 800, 500),
                (&modules, 800, 500),
                (&meta, 100, 500),
                (&files, 600, 500),
                (&group, 600, 500),
                (&art, 600, 500),
                (&ver, 600, 500),
                (&odd, 100, 500),
                (&daemon, 200, 500),
                (&d85, 100, 400),
                (&d89, 100, 900),
            ]),
        );
        let module = units.iter().find(|u| u.path == art).expect("module unit");
        assert_eq!(
            module.variant.package.as_deref(),
            Some("org.slf4j:slf4j-api")
        );
        let m = units
            .iter()
            .find(|u| u.path == meta)
            .expect("metadata unit");
        assert_eq!(m.role, ArtifactRole::Metadata);
        let j = units
            .iter()
            .find(|u| u.path == odd)
            .expect("an unknown modules-2 entry");
        assert!(
            !j.coverage.supported,
            "an unknown entry is a named residual"
        );
        let a = units.iter().find(|u| u.path == d85).unwrap();
        let b = units.iter().find(|u| u.path == d89).unwrap();
        assert_eq!(a.variant.toolchain.as_deref(), Some("gradle 8.5"));
        assert_eq!(b.variant.toolchain.as_deref(), Some("gradle 8.9"));
        assert_ne!(a.id, b.id, "two daemon versions' logs never collapse");
    }

    #[test]
    fn a_module_cache_entry_carries_its_coordinates() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("gradle-home");
        let files = home.join("caches/modules-2/files-2.1");
        let art = files.join("com.squareup.okhttp3/okhttp");
        fs::create_dir_all(&art).unwrap();
        let c = BuildContainer::shared_store("gradle", home.clone());
        let idx = index(&[
            (&home, 1_000, 500),
            (&home.join("caches"), 900, 500),
            (&home.join("caches/modules-2"), 800, 500),
            (&files, 800, 500),
            (&files.join("com.squareup.okhttp3"), 700, 500),
            (&art, 700, 500),
        ]);
        let units = run(&c, &idx);
        let u = units.iter().find(|u| u.path == art).expect("module entry");
        assert_eq!(
            u.variant.package.as_deref(),
            Some("com.squareup.okhttp3:okhttp")
        );
        assert!(matches!(
            u.action,
            crate::artifact::NestedActionCapability::Unsupported { .. }
        ));
    }
}
