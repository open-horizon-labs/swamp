//! Xcode and SwiftPM build storage, and the Apple developer stores beside
//! it (#70).
//!
//! What is inside a `DerivedData/<Project>-<hash>/` folder is documented
//! by Xcode's own layout: `Build/Products/<Configuration>-<platform>/`
//! (the products, `.xctest` bundles, `.dSYM`s), `Build/Intermediates.noindex`
//! (object files, build database), `Index.noindex` (the index store),
//! `Logs/Test/*.xcresult` (test results), `SourcePackages` (Swift package
//! checkouts), and an `info.plist` whose `WorkspacePath` names the
//! project that produced it. SwiftPM's `.build/` is the same idea for a
//! package: `<triple>/<configuration>/`, `checkouts/`, `repositories/`,
//! `artifacts/`, and a `workspace-state.json` naming each dependency.
//!
//! # Kept apart, on purpose
//!
//! Archives, device support symbols, simulator runtimes and simulator
//! devices are **not build output** and never get an output role: an
//! archive is a retained release (its dSYMs may be the only symbols for
//! a shipped build), a runtime is an installation, a device holds
//! whatever was installed and written in it. Each gets its own role and
//! its own consequence, and none of it is folded into "Developer".
//!
//! # What is never done
//!
//! No `xcodebuild`, no `swift build`, no `xcrun`, no build phase or
//! package plugin. Property lists and JSON are read bounded; a binary
//! property list is decoded by [`super::layout::plist_scalar`], never by
//! spawning `plutil`.

use super::layout::{name_of, plist_scalar};
use super::{
    BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, FoldedDir, NestedUnitBuilder,
};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// A build folder modified this recently before the observation may
/// have a build writing into it now. Evidence, stated as such; never a
/// claim either way.
const RECENT_WRITE_SECS: u64 = 10 * 60;

/// How many archives get their `Info.plist` read.
const ARCHIVE_BUDGET: usize = 200;

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "xcode-swift"
    }

    fn name(&self) -> &'static str {
        "Xcode / Swift"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: true,
            actions_available: true,
        }
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[
            BuildStoreKind::XcodeDerivedData,
            BuildStoreKind::XcodeArchives,
            BuildStoreKind::XcodeDeviceSupport,
            BuildStoreKind::SimulatorDevices,
            BuildStoreKind::SimulatorRuntimes,
            BuildStoreKind::SimulatorCaches,
        ]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        let package = crate::fs_gate::is_file(project_root.join("Package.swift"));
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "xcode-swift",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        if package {
            claim(project_root.join(".build"));
        }
        for c in candidates {
            match name_of(c) {
                ".build"
                    if package
                        || c.parent()
                            .is_some_and(|p| crate::fs_gate::is_file(p.join("Package.swift"))) =>
                {
                    claim(c.clone())
                }
                // A project-local DerivedData (`-derivedDataPath`, or a
                // workspace setting "relative to workspace"): the walk
                // classified it next to an Xcode project marker.
                "DerivedData" => claim(c.clone()),
                _ => {}
            }
        }
        out
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        match container.store_kind {
            Some(BuildStoreKind::XcodeArchives) => return identify_archives(container, ctx),
            Some(BuildStoreKind::XcodeDeviceSupport) => {
                return identify_device_support(container, ctx);
            }
            Some(BuildStoreKind::SimulatorDevices) => return identify_devices(container, ctx),
            Some(BuildStoreKind::SimulatorRuntimes) => return identify_runtimes(container, ctx),
            Some(BuildStoreKind::SimulatorCaches) => {
                return identify_simulator_caches(container, ctx);
            }
            Some(BuildStoreKind::XcodeDerivedData) => return identify_derived_data(container, ctx),
            _ => {}
        }
        match name_of(&container.path) {
            ".build" => identify_swiftpm(container, ctx),
            _ => identify_derived_data(container, ctx),
        }
    }
}

fn derived_root(container: &BuildContainer, ctx: &BuildCtx) -> NestedArtifact {
    let b = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Output)
        .supported_with_reason("Xcode's DerivedData: per-project build folders")
        .consequence(
            "Xcode rebuilds and re-indexes each project the next time it is opened or built",
        );
    if container.shared {
        b.membership(Membership::Unknown)
            .no_action_because("DerivedData holds every Xcode project's builds on this machine")
            .build()
    } else {
        b.build()
    }
}

/// DerivedData: its project folders, plus the caches Xcode keeps beside
/// them. A `-derivedDataPath` folder *is* one project folder, with no
/// project folders inside it.
fn identify_derived_data(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    if ctx.folded().get(&container.path.join("Build")).is_some() {
        // The container is itself a project folder; its first unit is
        // the container's own row.
        return identify_project_folder(container, ctx, &container.path);
    }
    let mut units = vec![derived_root(container, ctx)];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        if name.ends_with(".noindex") {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Intermediate,
                    child,
                    format!("`{name}` is a cache Xcode shares across projects in this DerivedData"),
                    "Xcode rebuilds it on the next build (a slower build, same result)",
                )
                .build(),
            );
            continue;
        }
        if ctx.folded().get(&child.path.join("Build")).is_some()
            || ctx.stat(&child.path.join("info.plist")).is_some()
        {
            units.extend(identify_project_folder(container, ctx, &child.path));
        } else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!(
                        "`{name}` has no `Build/` or `info.plist`: not a DerivedData project folder"
                    ),
                )
                .build(),
            );
        }
    }
    units
}

/// One `<Project>-<hash>` folder (or a `-derivedDataPath` root).
fn identify_project_folder(
    container: &BuildContainer,
    ctx: &BuildCtx,
    folder: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    let Some(dir) = ctx.folded().get(folder) else {
        return units;
    };
    let name = name_of(folder).to_string();
    // The source project, from `info.plist`'s `WorkspacePath`. The
    // folder's name is `<Name>-<hash>` and the hash is not reversible,
    // so the plist is the only evidence of which project this is.
    let workspace = ctx
        .manifest(&folder.join("info.plist"))
        .filter(|m| !m.truncated)
        .and_then(|m| plist_scalar(&m.raw, &["WorkspacePath"]));
    let mut variant = ArtifactVariant::default();
    let mut b =
        NestedUnitBuilder::new(container, ArtifactRole::Output, folder.to_path_buf()).folded(dir);
    b = match &workspace {
        Some(ws) => {
            variant.package = Path::new(ws)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned());
            let b = b
                .supported_with_reason(
                    "a DerivedData project folder whose info.plist names its workspace",
                )
                .evidence(
                    "xcode-info-plist",
                    format!("WorkspacePath {ws}"),
                    Confidence::High,
                );
            if ctx.stat(Path::new(ws)).is_none() {
                b.limit(format!(
                    "the workspace this build came from is not at `{ws}` now (moved, renamed or \
                     removed); its build output is still here"
                ))
            } else {
                b
            }
        }
        None => {
            variant.unknowns.push("package".into());
            b.supported_with_reason("a DerivedData project folder")
                .limit(format!(
                    "`{name}/info.plist` is missing or names no WorkspacePath: which project built \
                     this is unknown, and not guessed from the folder name"
                ))
        }
    };
    b = b
        .variant(variant.clone())
        .consequence("Xcode rebuilds and re-indexes this project the next time it is built");
    b = active_writer(b, dir, ctx.observed_at);
    units.push(b.build());

    let known: &[(&str, ArtifactRole, &str, &str)] = &[
        (
            "Build/Intermediates.noindex",
            ArtifactRole::Intermediate,
            "object files, build database and per-target intermediates",
            "the next build recompiles every target (a slower build, same result)",
        ),
        (
            "Index.noindex",
            ArtifactRole::Intermediate,
            "the index store (code completion, jump-to-definition)",
            "Xcode re-indexes the project when it is next opened",
        ),
        (
            "Index",
            ArtifactRole::Intermediate,
            "the index store (older Xcode layout)",
            "Xcode re-indexes the project when it is next opened",
        ),
        (
            "ModuleCache.noindex",
            ArtifactRole::Intermediate,
            "precompiled Clang/Swift modules",
            "the next build recompiles the modules it imports",
        ),
        (
            "TextIndex",
            ArtifactRole::Intermediate,
            "the text search index",
            "Xcode rebuilds it on demand",
        ),
        (
            "Logs/Build",
            ArtifactRole::Metadata,
            "build logs",
            "the next build writes new logs; these ones are gone",
        ),
        (
            "Logs/Launch",
            ArtifactRole::Metadata,
            "launch logs",
            "the next run writes new logs",
        ),
        (
            "Logs/Debug",
            ArtifactRole::Metadata,
            "debugger logs",
            "the next debug session writes new logs",
        ),
        (
            "SourcePackages/repositories",
            ArtifactRole::Intermediate,
            "bare clones of Swift package repositories",
            "Xcode clones the package repositories again -- needs access to them",
        ),
        (
            "SourcePackages/artifacts",
            ArtifactRole::InstalledDependencies,
            "downloaded binary targets of Swift packages",
            "Xcode downloads the binary targets again -- needs network access",
        ),
    ];
    for (rel, role, what, consequence) in known {
        if let Some(d) = ctx.folded().get(&folder.join(rel)) {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    role.clone(),
                    d,
                    format!("`{rel}` holds {what}"),
                    *consequence,
                )
                .variant(variant.clone())
                .build(),
            );
        }
    }
    units.extend(identify_products(container, ctx, folder, &variant));
    units.extend(identify_test_results(container, ctx, folder, &variant));
    units.extend(identify_checkouts(
        container,
        ctx,
        &folder.join("SourcePackages/checkouts"),
        &folder.join("SourcePackages/workspace-state.json"),
    ));
    units
}

/// A build folder modified within [`RECENT_WRITE_SECS`] of the
/// observation: a build may be writing it.
fn active_writer(b: NestedUnitBuilder, dir: &FoldedDir, observed_at: u64) -> NestedUnitBuilder {
    if dir.mtime_max > 0
        && observed_at >= dir.mtime_max
        && observed_at - dir.mtime_max <= RECENT_WRITE_SECS
    {
        b.limit(format!(
            "modified {} seconds before this observation: a build may be writing here now",
            observed_at - dir.mtime_max
        ))
        .no_action_because("a build may be writing here now")
    } else {
        b
    }
}

/// `Build/Products/<Configuration>-<platform>/` and what is inside it.
fn identify_products(
    container: &BuildContainer,
    ctx: &BuildCtx,
    folder: &Path,
    project: &ArtifactVariant,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    let products = folder.join("Build/Products");
    for config_dir in ctx.folded().children(&products) {
        let name = name_of(&config_dir.path).to_string();
        // `Debug-iphonesimulator`, `Release-iphoneos`, `Debug` (macOS).
        let (config, platform) = match name.split_once('-') {
            Some((c, p)) => (c.to_string(), p.to_string()),
            None => (name.clone(), "macosx".to_string()),
        };
        let variant = ArtifactVariant {
            package: project.package.clone(),
            profile: Some(config.clone()),
            configuration: Some(platform.clone()),
            unknowns: vec!["build-generation".into()],
            ..Default::default()
        };
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Output,
                config_dir,
                format!("`Build/Products/{name}` holds the {config} products for {platform}"),
                format!("the next {config} build for {platform} produces them again"),
            )
            .variant(variant.clone())
            .build(),
        );
        for item in ctx.folded().children(&config_dir.path) {
            let item_name = name_of(&item.path).to_string();
            let (role, what, consequence) = if item_name.ends_with(".xctest") {
                (
                    ArtifactRole::TestOutput,
                    "a test bundle",
                    "the next test build produces it again",
                )
            } else if item_name.ends_with(".dSYM") {
                (
                    ArtifactRole::Metadata,
                    "debug symbols for this build",
                    "the next build writes new symbols; these match only this build's binary",
                )
            } else if item_name.ends_with(".swiftmodule") {
                (
                    ArtifactRole::Intermediate,
                    "a compiled Swift module interface",
                    "the next build compiles it again",
                )
            } else if [".app", ".appex", ".framework", ".bundle", ".xpc", ".docc"]
                .iter()
                .any(|s| item_name.ends_with(s))
            {
                (
                    ArtifactRole::Output,
                    "a built product",
                    "the next build produces it again",
                )
            } else {
                units.push(
                    NestedUnitBuilder::unknown_dir(
                        container,
                        item,
                        format!("`{item_name}` is not a product type this adapter identifies"),
                    )
                    .build(),
                );
                continue;
            };
            let mut v = variant.clone();
            v.target = Some(item_name.clone());
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    role,
                    item,
                    format!("`{item_name}` is {what}"),
                    consequence,
                )
                .variant(v)
                .build(),
            );
        }
    }
    units
}

/// `Logs/Test/*.xcresult`: one unit per result bundle.
fn identify_test_results(
    container: &BuildContainer,
    ctx: &BuildCtx,
    folder: &Path,
    project: &ArtifactVariant,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for r in ctx.folded().children(&folder.join("Logs/Test")) {
        let name = name_of(&r.path).to_string();
        let b = if name.ends_with(".xcresult") {
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::TestOutput,
                r,
                "an `.xcresult` bundle: one test run's results, logs and attachments",
                "these results are gone; a test rerun produces new ones, not these",
            )
            .variant(ArtifactVariant {
                package: project.package.clone(),
                target: Some(name.clone()),
                ..Default::default()
            })
        } else {
            NestedUnitBuilder::unknown_dir(
                container,
                r,
                format!("`Logs/Test/{name}` is not an .xcresult bundle"),
            )
        };
        units.push(b.build());
    }
    units
}

/// Swift package checkouts, with identity and version from SwiftPM's
/// `workspace-state.json` (read bounded). A checkout the state file does
/// not describe keeps an unknown identity.
fn identify_checkouts(
    container: &BuildContainer,
    ctx: &BuildCtx,
    checkouts: &Path,
    state_file: &Path,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    let dirs = ctx.folded().children(checkouts);
    if dirs.is_empty() {
        return units;
    }
    // subpath -> (identity, version-or-revision)
    let mut known: HashMap<String, (String, Option<String>)> = HashMap::new();
    let state = ctx.manifest(state_file).filter(|m| !m.truncated);
    if let Some(m) = &state
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&m.text)
        && let Some(deps) = v
            .get("object")
            .and_then(|o| o.get("dependencies"))
            .and_then(|d| d.as_array())
    {
        for d in deps {
            let identity = d
                .get("packageRef")
                .and_then(|p| p.get("identity"))
                .and_then(|i| i.as_str());
            let subpath = d.get("subpath").and_then(|s| s.as_str()).or(identity);
            let checkout = d.get("state").and_then(|s| s.get("checkoutState"));
            let version = checkout
                .and_then(|c| c.get("version").and_then(|v| v.as_str()))
                .or_else(|| checkout.and_then(|c| c.get("revision").and_then(|v| v.as_str())))
                .map(str::to_string);
            if let (Some(identity), Some(subpath)) = (identity, subpath) {
                known.insert(subpath.to_string(), (identity.to_string(), version));
            }
        }
    }
    for d in dirs {
        let name = name_of(&d.path).to_string();
        let mut b = NestedUnitBuilder::known_dir(
            container,
            ArtifactRole::InstalledDependencies,
            d,
            "a Swift package checkout",
            "the package is checked out again on the next resolve -- needs access to its repository",
        );
        let mut variant = ArtifactVariant::default();
        match known.get(&name) {
            Some((identity, version)) => {
                variant.package = Some(identity.clone());
                variant.version = version.clone();
                b = b.evidence(
                    "swiftpm-workspace-state",
                    format!("{identity} {}", version.as_deref().unwrap_or("(unpinned)")),
                    Confidence::High,
                );
            }
            None => {
                variant.unknowns.push("package".into());
                variant.unknowns.push("version".into());
                b = b.limit(if state.is_some() {
                    "workspace-state.json does not describe this checkout; its identity is not \
                     guessed from the directory name"
                } else {
                    "workspace-state.json is missing or unreadable: the checkout's identity is \
                     unknown"
                });
            }
        }
        units.push(b.variant(variant).build());
    }
    units
}

/// SwiftPM's `.build/`: per-triple, per-configuration outputs, the
/// dependency checkouts, and the lock SwiftPM holds while it runs.
fn identify_swiftpm(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut root = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Output)
        .supported_with_reason("SwiftPM's `.build/` directory")
        .consequence("the next `swift build` rebuilds it and resolves dependencies again");
    let lock = container.path.join(".lock");
    if ctx.stat(&lock).is_some() {
        root = root.writer_lock(lock, "SwiftPM");
    }
    if let Some(d) = ctx.folded().get(&container.path) {
        root = active_writer(root, d, ctx.observed_at);
    }
    let mut units = vec![root.build()];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        match name.as_str() {
            "checkouts" => units.extend(identify_checkouts(
                container,
                ctx,
                &child.path,
                &container.path.join("workspace-state.json"),
            )),
            "repositories" => units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Intermediate,
                    child,
                    "`.build/repositories` holds bare clones of dependency repositories",
                    "SwiftPM clones them again -- needs access to them",
                )
                .build(),
            ),
            "artifacts" => units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::InstalledDependencies,
                    child,
                    "`.build/artifacts` holds downloaded binary targets",
                    "SwiftPM downloads them again -- needs network access",
                )
                .build(),
            ),
            "plugins" | "index-build" | "prebuilts" => units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Intermediate,
                    child,
                    format!("`.build/{name}` is SwiftPM's own build state"),
                    "SwiftPM regenerates it on the next build",
                )
                .build(),
            ),
            _ => {
                // `<triple>/<configuration>`: `arm64-apple-macosx/debug`.
                let configs: Vec<&FoldedDir> = ctx
                    .folded()
                    .children(&child.path)
                    .into_iter()
                    .filter(|c| matches!(name_of(&c.path), "debug" | "release"))
                    .collect();
                if name.contains('-') && !configs.is_empty() {
                    let arch = name.split('-').next().map(str::to_string);
                    for c in configs {
                        let config = name_of(&c.path).to_string();
                        units.push(
                            NestedUnitBuilder::known_dir(
                                container,
                                ArtifactRole::Output,
                                c,
                                format!(
                                    "`.build/{name}/{config}` holds the {config} build for {name}"
                                ),
                                format!("the next `swift build -c {config}` rebuilds it"),
                            )
                            .variant(ArtifactVariant {
                                profile: Some(config.clone()),
                                target: Some(name.clone()),
                                architecture: arch.clone(),
                                unknowns: vec!["build-generation".into()],
                                ..Default::default()
                            })
                            .build(),
                        );
                    }
                } else {
                    units.push(
                        NestedUnitBuilder::unknown_dir(
                            container,
                            child,
                            format!(
                                "`.build/{name}` is not a SwiftPM directory this adapter identifies"
                            ),
                        )
                        .build(),
                    );
                }
            }
        }
    }
    units
}

// ---------------------------------------------------------------------
// Stores that are not build output
// ---------------------------------------------------------------------

fn store_root(
    container: &BuildContainer,
    ctx: &BuildCtx,
    role: ArtifactRole,
    what: &str,
    consequence: &str,
    why_no_action: &str,
) -> NestedArtifact {
    NestedUnitBuilder::container_root(container, ctx, role)
        .supported_with_reason(what.to_string())
        .membership(Membership::Unknown)
        .consequence(consequence)
        .no_action_because(why_no_action.to_string())
        .build()
}

/// `Archives/<date>/<Name> <date>.xcarchive`: retained releases. Name
/// and version from each archive's own `Info.plist`.
fn identify_archives(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        ArtifactRole::Archive,
        "Xcode's archives: builds a person archived for distribution",
        "an archive holds the exact binary and debug symbols that were distributed; a rebuild \
         makes a different binary, and these symbols may be the only ones for that release",
        "an archive may be the only copy of a released build's symbols",
    )];
    let mut read = 0usize;
    for day in ctx.folded().children(&container.path) {
        for a in ctx.folded().children(&day.path) {
            let name = name_of(&a.path).to_string();
            if !name.ends_with(".xcarchive") {
                units.push(
                    NestedUnitBuilder::unknown_dir(
                        container,
                        a,
                        format!(
                            "`{}` is not an .xcarchive",
                            relative_path(&container.path, &a.path)
                        ),
                    )
                    .build(),
                );
                continue;
            }
            let mut variant = ArtifactVariant::default();
            let mut b = NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Archive,
                a,
                "an Xcode archive",
                "this archived build and its symbols are not regenerated by anything; a rebuild \
                 makes a different binary",
            )
            .no_action_because("an archive may be the only copy of a released build's symbols");
            if read < ARCHIVE_BUDGET {
                read += 1;
                let plist = ctx
                    .manifest(&a.path.join("Info.plist"))
                    .filter(|m| !m.truncated);
                let get = |p: &[&str]| plist.as_ref().and_then(|m| plist_scalar(&m.raw, p));
                variant.package = get(&["Name"]);
                variant.version = get(&["ApplicationProperties", "CFBundleShortVersionString"]);
                variant.target = get(&["ApplicationProperties", "CFBundleIdentifier"]);
                if variant.package.is_none() {
                    variant.unknowns.push("package".into());
                    b = b.limit("the archive's Info.plist is missing or names no Name");
                }
            } else {
                variant.unknowns.push("package".into());
                b = b.limit(format!(
                    "identity not read: only {ARCHIVE_BUDGET} archives get an Info.plist read"
                ));
            }
            units.push(b.variant(variant).build());
        }
    }
    units
}

/// `iOS DeviceSupport/<version (build)>`: symbols Xcode copied from a
/// connected device. An installation, by OS version.
fn identify_device_support(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        ArtifactRole::Installation,
        "device support: OS symbols Xcode copied from connected devices",
        "Xcode copies the symbols again the next time a device on that OS version is connected -- \
         needs the device",
        "device support is shared by every project debugging on that OS version",
    )];
    for v in ctx.folded().children(&container.path) {
        let name = name_of(&v.path).to_string();
        // `17.2 (21C62)`, `iPhone15,2 17.2 (21C62)`, `17.2 (21C62) arm64e`.
        let version = name
            .split_whitespace()
            .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
            .map(str::to_string);
        let build = name
            .split_once('(')
            .and_then(|(_, r)| r.split_once(')'))
            .map(|(b, _)| b.to_string());
        let b = match &version {
            Some(ver) => NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Installation,
                v,
                format!("`{name}` holds the symbols of one device OS version"),
                "Xcode copies these again the next time such a device is connected",
            )
            .variant(ArtifactVariant {
                toolchain: Some(ver.clone()),
                version: build,
                ..Default::default()
            }),
            None => NestedUnitBuilder::unknown_dir(
                container,
                v,
                format!("`{name}` is not named `<version> (<build>)`"),
            ),
        };
        units.push(b.build());
    }
    units
}

/// `CoreSimulator/Devices/<UDID>/`: mutable device state. Name, runtime
/// and state from each device's `device.plist`.
fn identify_devices(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        ArtifactRole::DeviceState,
        "simulator devices: each device's installed apps and data",
        "a removed simulator device loses every app and every byte of data installed in it; \
         recreating it gives an empty device",
        "simulator devices hold data nothing regenerates",
    )];
    for d in ctx.folded().children(&container.path) {
        let udid = name_of(&d.path).to_string();
        let plist = ctx
            .manifest(&d.path.join("device.plist"))
            .filter(|m| !m.truncated);
        let Some(m) = plist else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    d,
                    format!("`{udid}` has no readable device.plist: not identified as a device"),
                )
                .build(),
            );
            continue;
        };
        let name = plist_scalar(&m.raw, &["name"]);
        let runtime = plist_scalar(&m.raw, &["runtime"]);
        let state = plist_scalar(&m.raw, &["state"]);
        let mut b = NestedUnitBuilder::known_dir(
            container,
            ArtifactRole::DeviceState,
            d,
            "a simulator device, identified by its device.plist",
            "this device's apps and data are gone; a recreated device starts empty",
        )
        .variant(ArtifactVariant {
            configuration: name.clone(),
            toolchain: runtime
                .as_deref()
                .and_then(|r| r.rsplit('.').next())
                .map(str::to_string),
            target: Some(udid.clone()),
            ..Default::default()
        })
        .no_action_because("a simulator device holds data nothing regenerates");
        // CoreSimulator's `state` 3 is Booted.
        if state.as_deref() == Some("3") {
            b = b.limit("device.plist records this device as booted: it may be running now");
        }
        units.push(b.build());
    }
    units
}

/// Simulator runtimes: installations, by name.
fn identify_runtimes(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        ArtifactRole::Installation,
        "simulator runtimes: installed simulator OS versions",
        "a removed runtime is downloaded again from Xcode's platform settings -- a multi-gigabyte \
         download; simulators on it stop booting until then",
        "simulator devices boot from these runtimes",
    )];
    for r in ctx.folded().children(&container.path) {
        let name = name_of(&r.path).to_string();
        let label = name.trim_end_matches(".simruntime").to_string();
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Installation,
                r,
                format!("`{name}` is an installed simulator runtime"),
                format!("{label} is downloaded again when a simulator needs it"),
            )
            .variant(ArtifactVariant {
                toolchain: Some(label),
                ..Default::default()
            })
            .no_action_because("simulator devices boot from this runtime")
            .build(),
        );
    }
    units
}

/// CoreSimulator's own cache (dyld shared caches per runtime build).
fn identify_simulator_caches(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Intermediate)
            .supported_with_reason("CoreSimulator's cache")
            .consequence("the next simulator boot rebuilds what it needs (a slower boot)")
            .build(),
    ];
    for c in ctx.folded().children(&container.path) {
        let name = name_of(&c.path).to_string();
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Intermediate,
                c,
                format!("`{name}` is part of CoreSimulator's cache"),
                "the next simulator boot rebuilds it",
            )
            .build(),
        );
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{NestedActionCapability, RoleFamily, TimeSource};
    use crate::build_adapters::{ContainerCache, FoldedIndex};
    use crate::fs_events::EventCoverage;
    use std::fs;

    fn index_of(root: &Path, mtime: u64) -> FoldedIndex {
        fn walk(p: &Path, out: &mut Vec<FoldedDir>, mtime: u64) -> u64 {
            let mut total = 4096;
            for e in fs::read_dir(p).unwrap().flatten() {
                if e.file_type().unwrap().is_dir() {
                    total += walk(&e.path(), out, mtime);
                } else {
                    total += e.metadata().unwrap().len().max(1);
                }
            }
            out.push(FoldedDir {
                path: p.to_path_buf(),
                allocated_total: total,
                mtime_max: mtime,
                complete: true,
            });
            total
        }
        let mut out = Vec::new();
        walk(root, &mut out, mtime);
        FoldedIndex::from_dirs(out)
    }

    fn run_at(c: &BuildContainer, idx: &FoldedIndex, now: u64) -> Vec<NestedArtifact> {
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        Adapter.identify(c, &BuildCtx::new(now, idx, &none, &cache))
    }

    fn run(c: &BuildContainer, idx: &FoldedIndex) -> Vec<NestedArtifact> {
        run_at(c, idx, 1_000_000)
    }

    fn put(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn info_plist(ws: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>WorkspacePath</key>\
             <string>{ws}</string></dict></plist>"
        )
    }

    fn derived(root: &Path) -> BuildContainer {
        BuildContainer::shared_store_of(
            "xcode-swift",
            root.to_path_buf(),
            BuildStoreKind::XcodeDerivedData,
        )
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        fs::create_dir_all(dd.join("Stray")).unwrap();
        let units = run(&derived(&dd), &index_of(&dd, 5));
        let stray = units.iter().find(|u| u.path == dd.join("Stray")).unwrap();
        assert!(!stray.coverage.supported);
        assert!(stray.coverage.limits.iter().any(|l| l.contains("Stray")));
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        let folder = dd.join("App-abcdefghijklmnopqrstuvwxyzab");
        fs::create_dir_all(folder.join("Build")).unwrap();
        put(
            &folder.join("info.plist"),
            &format!(
                "{}{}",
                info_plist("/w/App.xcworkspace"),
                " ".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES)
            ),
        );
        let (units, counted) =
            crate::work_counters::measured(|| run(&derived(&dd), &index_of(&dd, 5)));
        assert!(counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64);
        let f = units.iter().find(|u| u.path == folder).unwrap();
        assert_eq!(
            f.variant.package, None,
            "a plist that hit the cap is not parsed from its prefix"
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            &root.join("Package.swift"),
            "// swift-tools-version:5.9\nimport PackageDescription\n",
        );
        fs::create_dir_all(root.join(".build/arm64-apple-macosx/debug")).unwrap();
        put(
            &root.join(".build/arm64-apple-macosx/debug/tool"),
            "#!/bin/sh\nexit 7\n",
        );
        let c = BuildContainer::project("xcode-swift", root.join(".build"), root.to_path_buf());
        let (_u, counted) =
            crate::work_counters::measured(|| run(&c, &index_of(&root.join(".build"), 5)));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "no xcodebuild, swift, xcrun or plutil"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        let folder = dd.join("App-abcdefghijklmnopqrstuvwxyzab");
        put(
            &folder.join("info.plist"),
            &info_plist("/w/App.xcworkspace"),
        );
        for cfg in ["Debug-iphonesimulator", "Release-iphoneos", "Debug"] {
            fs::create_dir_all(folder.join(format!("Build/Products/{cfg}/App.app"))).unwrap();
        }
        let units = run(&derived(&dd), &index_of(&dd, 5));
        let apps: Vec<(Option<String>, Option<String>)> = units
            .iter()
            .filter(|u| u.variant.target.as_deref() == Some("App.app"))
            .map(|u| (u.variant.profile.clone(), u.variant.configuration.clone()))
            .collect();
        assert_eq!(
            apps.len(),
            3,
            "three App.app products, three units: {apps:?}"
        );
        assert!(apps.contains(&(Some("Debug".into()), Some("iphonesimulator".into()))));
        assert!(apps.contains(&(Some("Release".into()), Some("iphoneos".into()))));
        assert!(apps.contains(&(Some("Debug".into()), Some("macosx".into()))));
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        let folder = dd.join("Old-abcdefghijklmnopqrstuvwxyzab");
        put(
            &folder.join("info.plist"),
            &info_plist("/nowhere/Old.xcodeproj"),
        );
        fs::create_dir_all(folder.join("Build/Intermediates.noindex")).unwrap();
        let units = run(&derived(&dd), &index_of(&dd, 1));
        let f = units.iter().find(|u| u.path == folder).unwrap();
        assert_eq!(f.time_source, TimeSource::FoldedDirectoryModification);
        assert!(
            f.coverage
                .limits
                .iter()
                .any(|l| l.contains("is not at `/nowhere/Old.xcodeproj` now")),
            "a missing project is a fact about the project, not a verdict on the build: {:?}",
            f.coverage.limits
        );
        assert!(f.coverage.supported);
        assert_eq!(f.role, ArtifactRole::Output);
    }

    #[test]
    fn a_derived_data_folder_splits_products_tests_intermediates_index_and_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        let folder = dd.join("App-abcdefghijklmnopqrstuvwxyzab");
        let ws = tmp.path().join("App.xcworkspace");
        fs::create_dir_all(&ws).unwrap();
        put(
            &folder.join("info.plist"),
            &info_plist(&ws.display().to_string()),
        );
        fs::create_dir_all(folder.join("Build/Products/Debug-iphonesimulator/AppTests.xctest"))
            .unwrap();
        fs::create_dir_all(folder.join("Build/Products/Debug-iphonesimulator/App.app.dSYM"))
            .unwrap();
        fs::create_dir_all(folder.join("Build/Intermediates.noindex/App.build")).unwrap();
        fs::create_dir_all(folder.join("Index.noindex/DataStore")).unwrap();
        fs::create_dir_all(folder.join("Logs/Test/Run-App-2024.xcresult")).unwrap();
        fs::create_dir_all(folder.join("SourcePackages/checkouts/swift-log")).unwrap();
        put(
            &folder.join("SourcePackages/workspace-state.json"),
            r#"{"object":{"dependencies":[{"packageRef":{"identity":"swift-log"},"subpath":"swift-log","state":{"checkoutState":{"version":"1.5.4"}}}]}}"#,
        );
        let units = run(&derived(&dd), &index_of(&dd, 5));
        let role = |suffix: &str| {
            units
                .iter()
                .find(|u| u.path.ends_with(suffix))
                .map(|u| u.role.clone())
                .unwrap_or_else(|| panic!("no unit ends with {suffix}"))
        };
        assert_eq!(role("AppTests.xctest"), ArtifactRole::TestOutput);
        assert_eq!(role("App.app.dSYM"), ArtifactRole::Metadata);
        assert_eq!(
            role("Build/Intermediates.noindex"),
            ArtifactRole::Intermediate
        );
        assert_eq!(role("Index.noindex"), ArtifactRole::Intermediate);
        assert_eq!(role("Run-App-2024.xcresult"), ArtifactRole::TestOutput);
        let log = units
            .iter()
            .find(|u| u.path.ends_with("checkouts/swift-log"))
            .unwrap();
        assert_eq!(log.variant.package.as_deref(), Some("swift-log"));
        assert_eq!(log.variant.version.as_deref(), Some("1.5.4"));
        let f = units.iter().find(|u| u.path == folder).unwrap();
        assert_eq!(f.variant.package.as_deref(), Some("App"));
    }

    #[test]
    fn archives_runtimes_and_devices_are_never_build_output() {
        let tmp = tempfile::tempdir().unwrap();
        let archives = tmp.path().join("Archives");
        let a = archives.join("2024-01-15/App 1-15-24, 10.32.xcarchive");
        put(
            &a.join("Info.plist"),
            "<plist><dict><key>Name</key><string>App</string><key>ApplicationProperties</key>\
             <dict><key>CFBundleShortVersionString</key><string>2.4</string></dict></dict></plist>",
        );
        let units = run(
            &BuildContainer::shared_store_of(
                "xcode-swift",
                archives.clone(),
                BuildStoreKind::XcodeArchives,
            ),
            &index_of(&archives, 5),
        );
        let arch = units.iter().find(|u| u.path == a).unwrap();
        assert_eq!(arch.role, ArtifactRole::Archive);
        assert_eq!(arch.role.family(), RoleFamily::State);
        assert_eq!(arch.variant.version.as_deref(), Some("2.4"));
        assert!(matches!(
            arch.action,
            NestedActionCapability::Unsupported { .. }
        ));

        let devices = tmp.path().join("Devices");
        let dev = devices.join("0A1B2C3D-0000-0000-0000-000000000000");
        put(
            &dev.join("device.plist"),
            "<plist><dict><key>name</key><string>iPhone 15</string><key>runtime</key>\
             <string>com.apple.CoreSimulator.SimRuntime.iOS-17-2</string><key>state</key>\
             <integer>3</integer></dict></plist>",
        );
        let units = run(
            &BuildContainer::shared_store_of(
                "xcode-swift",
                devices.clone(),
                BuildStoreKind::SimulatorDevices,
            ),
            &index_of(&devices, 5),
        );
        let d = units.iter().find(|u| u.path == dev).unwrap();
        assert_eq!(d.role, ArtifactRole::DeviceState);
        assert_eq!(d.variant.toolchain.as_deref(), Some("iOS-17-2"));
        assert!(d.coverage.limits.iter().any(|l| l.contains("booted")));

        let runtimes = tmp.path().join("Runtimes");
        fs::create_dir_all(runtimes.join("iOS 17.2.simruntime")).unwrap();
        let units = run(
            &BuildContainer::shared_store_of(
                "xcode-swift",
                runtimes.clone(),
                BuildStoreKind::SimulatorRuntimes,
            ),
            &index_of(&runtimes, 5),
        );
        assert!(
            units.iter().all(|u| u.role == ArtifactRole::Installation),
            "a runtime is an installation: {:?}",
            units.iter().map(|u| &u.role).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_active_build_is_evidence_not_a_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(&root.join("Package.swift"), "// swift-tools-version:5.9\n");
        put(&root.join(".build/.lock"), "");
        fs::create_dir_all(root.join(".build/arm64-apple-macosx/release")).unwrap();
        let c = BuildContainer::project("xcode-swift", root.join(".build"), root.to_path_buf());
        let now = 5_000;
        let units = run_at(&c, &index_of(&root.join(".build"), now - 30), now);
        let r = &units[0];
        assert!(
            r.writer_lock.is_some(),
            "the lock SwiftPM holds while it runs is recorded"
        );
        assert!(
            r.coverage
                .limits
                .iter()
                .any(|l| l.contains("30 seconds before"))
        );
        assert!(matches!(
            r.action,
            NestedActionCapability::Unsupported { .. }
        ));
        let rel = units
            .iter()
            .find(|u| u.variant.profile.as_deref() == Some("release"))
            .unwrap();
        assert_eq!(rel.variant.architecture.as_deref(), Some("arm64"));
    }

    #[test]
    fn a_project_folder_without_info_plist_keeps_an_unknown_project() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = tmp.path().join("DerivedData");
        let folder = dd.join("Mystery-abcdefghijklmnopqrstuvwxyzab");
        fs::create_dir_all(folder.join("Build/Products/Debug")).unwrap();
        let units = run(&derived(&dd), &index_of(&dd, 5));
        let f = units.iter().find(|u| u.path == folder).unwrap();
        assert_eq!(f.variant.package, None);
        assert!(f.variant.unknowns.iter().any(|x| x == "package"));
        assert!(
            f.coverage.supported,
            "partial metadata: still a DerivedData folder, still inspectable"
        );
    }
}
