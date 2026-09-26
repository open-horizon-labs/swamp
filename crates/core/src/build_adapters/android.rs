//! Android module build directories, NDK intermediates, the Android SDK
//! and virtual devices (#70).
//!
//! An Android module's `build/` is a Gradle build directory with more in
//! it, so the conventional Gradle entries come from the same neutral
//! table the Gradle adapter uses ([`super::jvm_common::GRADLE_BUILD_ENTRIES`];
//! #70: "reuse Gradle support where appropriate"), and the Android Gradle
//! plugin's own layout adds what matters most: `outputs/apk/<flavor>/<buildType>/`,
//! `outputs/bundle/<variant>/`, `outputs/mapping/<variant>/`, and
//! `intermediates/<task>/<variant>/`. The variant (build type, product
//! flavor, test source set) comes from those directory names -- the one
//! place the plugin records it on disk -- never from evaluating
//! `build.gradle`.
//!
//! The SDK and AVDs are kept apart from build output: an SDK package is
//! an installation (a reinstall is `sdkmanager`, a download), and an AVD
//! is a device's mutable state (its installed apps and data are nowhere
//! else).
//!
//! # What is never done
//!
//! No Gradle task, no `sdkmanager`, no `avdmanager`, no emulator.
//! `source.properties` and `config.ini` are read bounded.

use super::layout::{name_of, properties};
use super::{
    BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, FoldedDir, NestedUnitBuilder,
};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// Build types the Android Gradle plugin defines by default. A variant
/// name ending in one of these is `<flavor><BuildType>`; anything else is
/// kept whole with the split unknown, never guessed.
const BUILD_TYPES: &[&str] = &["debug", "release", "benchmark", "profile", "staging"];

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "android"
    }

    fn name(&self) -> &'static str {
        "Android"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: false,
            actions_available: true,
        }
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[
            BuildStoreKind::AndroidSdkPackages,
            BuildStoreKind::AndroidVirtualDevices,
        ]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        let gradle = [
            "settings.gradle",
            "settings.gradle.kts",
            "build.gradle",
            "build.gradle.kts",
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
                    "android",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        for c in candidates {
            match name_of(c) {
                // A module's `build/` is Android's only when the module is
                // an Android module: its manifest is where the plugin
                // requires it.
                "build" => {
                    if c.parent().is_some_and(|m| {
                        crate::fs_gate::is_file(m.join("src/main/AndroidManifest.xml"))
                    }) {
                        claim(c.clone());
                    }
                }
                ".cxx" | ".externalNativeBuild" => claim(c.clone()),
                _ => {}
            }
        }
        for module_build in [project_root.join("app/build"), project_root.join("build")] {
            if module_build
                .parent()
                .is_some_and(|m| crate::fs_gate::is_file(m.join("src/main/AndroidManifest.xml")))
            {
                claim(module_build);
            }
        }
        out
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        match container.store_kind {
            Some(BuildStoreKind::AndroidSdkPackages) => {
                return identify_sdk_packages(container, ctx);
            }
            Some(BuildStoreKind::AndroidVirtualDevices) => return identify_avds(container, ctx),
            _ => {}
        }
        match name_of(&container.path) {
            ".cxx" | ".externalNativeBuild" => identify_native(container, ctx),
            _ => identify_module_build(container, ctx),
        }
    }
}

/// `freeDebug` -> (Some("free"), Some("debug")); `debugAndroidTest` ->
/// (None, Some("debug")) with the test set noted; `staging` ->
/// (None, Some("staging")); `weirdThing` -> (None, None).
fn split_variant(name: &str) -> (Option<String>, Option<String>, Option<&'static str>) {
    let (base, test) = if let Some(b) = name.strip_suffix("AndroidTest") {
        (b, Some("androidTest"))
    } else if let Some(b) = name.strip_suffix("UnitTest") {
        (b, Some("unitTest"))
    } else {
        (name, None)
    };
    let lower = base.to_ascii_lowercase();
    for bt in BUILD_TYPES {
        if lower == *bt {
            return (None, Some(bt.to_string()), test);
        }
        if lower.ends_with(bt) && base.len() > bt.len() {
            let (flavor, rest) = base.split_at(base.len() - bt.len());
            if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                return (Some(flavor.to_string()), Some(bt.to_string()), test);
            }
        }
    }
    (None, None, test)
}

fn variant_of(name: &str) -> ArtifactVariant {
    let (flavor, build_type, test) = split_variant(name);
    let mut v = ArtifactVariant {
        profile: build_type.clone(),
        features: flavor,
        configuration: Some(name.to_string()),
        target: test.map(str::to_string),
        ..Default::default()
    };
    if build_type.is_none() {
        v.unknowns.push("profile".into());
    }
    v.unknowns.push("build-generation".into());
    v
}

fn identify_module_build(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let module = container
        .path
        .parent()
        .map(name_of)
        .unwrap_or_default()
        .to_string();
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Output)
            .supported_with_reason(format!(
                "the build directory of the Android module `{module}` (its manifest is at \
                 src/main/AndroidManifest.xml)"
            ))
            .evidence(
                "android-layout",
                format!("module {module}"),
                Confidence::Medium,
            )
            .consequence("the next Gradle build of this module regenerates it")
            .build(),
    ];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        match name.as_str() {
            "outputs" => units.extend(identify_outputs(container, ctx, child)),
            "intermediates" => units.extend(identify_per_task(
                container,
                ctx,
                child,
                ArtifactRole::Intermediate,
                "the Android Gradle plugin's intermediates for one task type",
                "the next build of those variants regenerates them (a slower build)",
            )),
            "test-results" | "reports" => units.extend(identify_test_dirs(container, ctx, child)),
            _ => {
                let known = super::jvm_common::GRADLE_BUILD_ENTRIES
                    .iter()
                    .find(|(n, _, _)| *n == name);
                let b = match known {
                    Some((_, role, consequence)) => NestedUnitBuilder::known_dir(
                        container,
                        role.clone(),
                        child,
                        format!("`build/{name}` is part of Gradle's conventional build layout"),
                        *consequence,
                    ),
                    None => NestedUnitBuilder::unknown_dir(
                        container,
                        child,
                        format!(
                            "`build/{name}` is not part of the Gradle or Android plugin layout; a \
                             plugin chose it, and swamp never evaluates build scripts"
                        ),
                    ),
                };
                units.push(b.build());
            }
        }
    }
    units
}

/// `outputs/apk/<flavor>/<buildType>/`, `outputs/bundle/<variant>/`,
/// `outputs/mapping/<variant>/`, `outputs/aar`, `outputs/logs`.
fn identify_outputs(
    container: &BuildContainer,
    ctx: &BuildCtx,
    outputs: &FoldedDir,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for kind in ctx.folded().children(&outputs.path) {
        let k = name_of(&kind.path).to_string();
        match k.as_str() {
            "apk" | "bundle" | "native-debug-symbols" | "mapping" => {
                let (role, what, consequence) = match k.as_str() {
                    "apk" => (
                        ArtifactRole::Output,
                        "an APK",
                        "the next assemble of this variant builds it again",
                    ),
                    "bundle" => (
                        ArtifactRole::Output,
                        "an app bundle",
                        "the next bundle task for this variant builds it again",
                    ),
                    "mapping" => (
                        ArtifactRole::Metadata,
                        "R8/ProGuard mapping files",
                        "the next minified build writes a new mapping, which matches only that \
                         build; a mapping for a build already shipped is not regenerated",
                    ),
                    _ => (
                        ArtifactRole::Metadata,
                        "native debug symbols",
                        "the next build writes new symbols, matching only that build",
                    ),
                };
                // apk: `<flavor>/<buildType>` or `<buildType>`; others:
                // `<variant>`.
                for v in ctx.folded().children(&kind.path) {
                    let vname = name_of(&v.path).to_string();
                    let nested: Vec<&FoldedDir> = ctx
                        .folded()
                        .children(&v.path)
                        .into_iter()
                        .filter(|d| BUILD_TYPES.contains(&name_of(&d.path)))
                        .collect();
                    if k == "apk" && !nested.is_empty() {
                        for bt in nested {
                            let btname = name_of(&bt.path).to_string();
                            let mut variant = variant_of(&btname);
                            variant.features = Some(vname.clone());
                            variant.configuration = Some(format!("{vname}/{btname}"));
                            units.push(
                                NestedUnitBuilder::known_dir(
                                    container,
                                    role.clone(),
                                    bt,
                                    format!("`outputs/{k}/{vname}/{btname}` holds {what} for flavor {vname}"),
                                    consequence,
                                )
                                .variant(variant)
                                .build(),
                            );
                        }
                        continue;
                    }
                    units.push(
                        NestedUnitBuilder::known_dir(
                            container,
                            role.clone(),
                            v,
                            format!("`outputs/{k}/{vname}` holds {what} for variant {vname}"),
                            consequence,
                        )
                        .variant(variant_of(&vname))
                        .build(),
                    );
                }
            }
            "aar" => units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Output,
                    kind,
                    "`outputs/aar` holds the library's packaged archives",
                    "the next assemble packages them again",
                )
                .build(),
            ),
            "logs" | "sdk-dependencies" => units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Metadata,
                    kind,
                    format!("`outputs/{k}` is the plugin's own bookkeeping"),
                    "the next build rewrites it",
                )
                .build(),
            ),
            _ => units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    kind,
                    format!("`outputs/{k}` is not an output type this adapter identifies"),
                )
                .build(),
            ),
        }
    }
    units
}

/// `intermediates/<task>/`: one unit per task type, naming the variants
/// it holds. Per-(task, variant) units would be hundreds of rows saying
/// the same thing; the variants are the unit's configuration instead.
fn identify_per_task(
    container: &BuildContainer,
    ctx: &BuildCtx,
    dir: &FoldedDir,
    role: ArtifactRole,
    what: &str,
    consequence: &str,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for task in ctx.folded().children(&dir.path) {
        let tname = name_of(&task.path).to_string();
        let variants: Vec<String> = ctx
            .folded()
            .children(&task.path)
            .iter()
            .map(|v| name_of(&v.path).to_string())
            .collect();
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                role.clone(),
                task,
                format!(
                    "`{}` holds {what}",
                    relative_path(&container.path, &task.path)
                ),
                consequence,
            )
            .variant(ArtifactVariant {
                target: Some(tname),
                configuration: (!variants.is_empty()).then(|| variants.join(", ")),
                ..Default::default()
            })
            .build(),
        );
    }
    units
}

/// `test-results/<task>` and `reports/<kind>`: test output, with the
/// variant from a `test<Variant>UnitTest` task name when it is one.
fn identify_test_dirs(
    container: &BuildContainer,
    ctx: &BuildCtx,
    dir: &FoldedDir,
) -> Vec<NestedArtifact> {
    let parent = name_of(&dir.path).to_string();
    let mut units = Vec::new();
    for t in ctx.folded().children(&dir.path) {
        let name = name_of(&t.path).to_string();
        let variant = name
            .strip_prefix("test")
            .map(variant_of)
            .unwrap_or_else(|| ArtifactVariant {
                target: Some(name.clone()),
                ..Default::default()
            });
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::TestOutput,
                t,
                format!("`{parent}/{name}` is test output"),
                "the next test run for this variant regenerates it",
            )
            .variant(variant)
            .build(),
        );
    }
    units
}

/// `.cxx/<Variant>/<hash>/<abi>` (AGP 7+) or
/// `.cxx/cmake/<variant>/<abi>`, and `.externalNativeBuild/cmake/<variant>/<abi>`:
/// one unit per (variant, ABI).
fn identify_native(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Intermediate)
            .supported_with_reason("the NDK/CMake intermediates the Android Gradle plugin keeps")
            .consequence("the next native build reconfigures and recompiles (a slower build)")
            .build(),
    ];
    const ABIS: &[&str] = &["arm64-v8a", "armeabi-v7a", "x86", "x86_64", "riscv64"];
    let mut frontier: Vec<(&FoldedDir, Vec<String>)> = ctx
        .folded()
        .children(&container.path)
        .into_iter()
        .map(|d| (d, vec![name_of(&d.path).to_string()]))
        .collect();
    while let Some((dir, trail)) = frontier.pop() {
        let name = name_of(&dir.path).to_string();
        if ABIS.contains(&name.as_str()) {
            let variant_name = trail
                .iter()
                .find(|t| t.as_str() != "cmake" && !t.chars().all(|c| c.is_ascii_hexdigit()))
                .cloned()
                .unwrap_or_default();
            let mut v = variant_of(&variant_name.to_ascii_lowercase());
            v.architecture = Some(name.clone());
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Intermediate,
                    dir,
                    format!("`{}` holds one variant's native build for {name}", relative_path(&container.path, &dir.path)),
                    "the next native build of this variant and ABI reconfigures and recompiles",
                )
                .variant(v)
                .limit("a hash component in this path is the plugin's configuration key; its inputs are not recorded")
                .build(),
            );
            continue;
        }
        if trail.len() >= 4 {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    dir,
                    format!(
                        "`{}` holds no ABI directory this adapter identifies",
                        relative_path(&container.path, &dir.path)
                    ),
                )
                .build(),
            );
            continue;
        }
        for c in ctx.folded().children(&dir.path) {
            let mut t = trail.clone();
            t.push(name_of(&c.path).to_string());
            frontier.push((c, t));
        }
    }
    units
}

// ---------------------------------------------------------------------
// SDK and AVDs
// ---------------------------------------------------------------------

/// One SDK package directory (`platforms`, `build-tools`,
/// `system-images`, `emulator`, `ndk`, ...), each child an installed
/// package with its revision from its own `source.properties`.
fn identify_sdk_packages(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let category = name_of(&container.path).to_string();
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Installation)
            .supported_with_reason(format!("the Android SDK's `{category}` packages"))
            .membership(Membership::Unknown)
            .consequence(
                "reinstall with `sdkmanager` (or Android Studio's SDK Manager) -- a download",
            )
            .no_action_because("SDK packages are shared by every Android project on this machine")
            .build(),
    ];
    let package = |dir: &FoldedDir, label: String| -> NestedArtifact {
        let props = ctx
            .manifest(&dir.path.join("source.properties"))
            .filter(|m| !m.truncated)
            .map(|m| properties(&m.text, false));
        let revision = props.as_ref().and_then(|p| p.get("Pkg.Revision").cloned());
        let mut b = NestedUnitBuilder::known_dir(
            container,
            ArtifactRole::Installation,
            dir,
            format!("`{category}/{label}` is an installed SDK package"),
            format!(
                "reinstall `{category};{}` with sdkmanager -- a download",
                label.replace('/', ";")
            ),
        )
        .variant(ArtifactVariant {
            toolchain: Some(label.clone()),
            version: revision.clone(),
            ..Default::default()
        })
        .no_action_because("SDK packages are shared by every Android project on this machine");
        b = match &revision {
            Some(r) => b.evidence(
                "android-source-properties",
                format!("Pkg.Revision={r}"),
                Confidence::High,
            ),
            None => b.limit("no readable source.properties: the package revision is unknown"),
        };
        b.build()
    };
    // These package directories are one package each, not a list of
    // versions: the root row is the unit.
    if matches!(category.as_str(), "emulator" | "platform-tools") {
        return units;
    }
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        if category == "system-images" {
            // `system-images/android-34/google_apis/arm64-v8a`.
            for tag in ctx.folded().children(&child.path) {
                for abi in ctx.folded().children(&tag.path) {
                    let label = format!("{name}/{}/{}", name_of(&tag.path), name_of(&abi.path));
                    let mut u = package(abi, label);
                    u = NestedUnitBuilder::amend(u)
                        .variant(ArtifactVariant {
                            toolchain: Some(name.clone()),
                            configuration: Some(name_of(&tag.path).to_string()),
                            architecture: Some(name_of(&abi.path).to_string()),
                            ..Default::default()
                        })
                        .build();
                    units.push(u);
                }
            }
            continue;
        }
        units.push(package(child, name));
    }
    units
}

/// `~/.android/avd/<name>.avd/`: each virtual device's mutable state,
/// its target and ABI from `config.ini`, and the emulator's lock files
/// as active-writer evidence.
fn identify_avds(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::DeviceState)
            .supported_with_reason("Android Virtual Devices: each emulator's disk images and state")
            .membership(Membership::Unknown)
            .consequence(
                "a removed AVD loses every app and every byte of data installed in it; a \
                 recreated one starts empty",
            )
            .no_action_because("an AVD holds data nothing regenerates")
            .build(),
    ];
    for avd in ctx.folded().children(&container.path) {
        let name = name_of(&avd.path).to_string();
        let Some(label) = name.strip_suffix(".avd") else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    avd,
                    format!("`{name}` is not an .avd directory"),
                )
                .build(),
            );
            continue;
        };
        let cfg = ctx
            .manifest(&avd.path.join("config.ini"))
            .filter(|m| !m.truncated)
            .map(|m| properties(&m.text, false));
        let mut b = NestedUnitBuilder::known_dir(
            container,
            ArtifactRole::DeviceState,
            avd,
            "an Android Virtual Device",
            "this emulator's apps and data are gone; a recreated AVD starts empty",
        )
        .variant(ArtifactVariant {
            configuration: Some(label.to_string()),
            toolchain: cfg.as_ref().and_then(|c| {
                c.get("image.sysdir.1")
                    .cloned()
                    .or_else(|| c.get("target").cloned())
            }),
            architecture: cfg.as_ref().and_then(|c| c.get("abi.type").cloned()),
            ..Default::default()
        })
        .no_action_because("an AVD holds data nothing regenerates");
        if cfg.is_none() {
            b = b.limit("no readable config.ini: the device's system image is unknown");
        }
        for lock in ["hardware-qemu.ini.lock", "multiinstance.lock"] {
            let p = avd.path.join(lock);
            if ctx.stat(&p).is_some() {
                b = b.writer_lock(p, "the emulator");
                break;
            }
        }
        units.push(b.build());
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

    fn run(c: &BuildContainer, idx: &FoldedIndex) -> Vec<NestedArtifact> {
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        Adapter.identify(c, &BuildCtx::new(1_000_000, idx, &none, &cache))
    }

    fn put(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// An Android project with one app module.
    fn project(tmp: &Path) -> (PathBuf, BuildContainer) {
        let root = tmp.join("App");
        put(&root.join("settings.gradle"), "include ':app'\n");
        put(
            &root.join("app/src/main/AndroidManifest.xml"),
            "<manifest/>",
        );
        let build = root.join("app/build");
        fs::create_dir_all(&build).unwrap();
        let c = BuildContainer::project("android", build.clone(), root.clone());
        (build, c)
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, c) = project(tmp.path());
        fs::create_dir_all(build.join("some-plugin-dir")).unwrap();
        let units = run(&c, &index_of(&build, 5));
        let odd = units
            .iter()
            .find(|u| u.path == build.join("some-plugin-dir"))
            .unwrap();
        assert!(!odd.coverage.supported);
        assert!(
            odd.coverage
                .limits
                .iter()
                .any(|l| l.contains("some-plugin-dir"))
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let platforms = tmp.path().join("sdk/platforms");
        put(
            &platforms.join("android-34/source.properties"),
            &format!(
                "Pkg.Revision=3\n{}",
                "#".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES)
            ),
        );
        let c = BuildContainer::shared_store_of(
            "android",
            platforms.clone(),
            BuildStoreKind::AndroidSdkPackages,
        );
        let (units, counted) = crate::work_counters::measured(|| run(&c, &index_of(&platforms, 5)));
        assert!(counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64);
        let p = units
            .iter()
            .find(|u| u.path == platforms.join("android-34"))
            .unwrap();
        assert_eq!(
            p.variant.version, None,
            "a truncated source.properties names no revision"
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, c) = project(tmp.path());
        put(
            &build.parent().unwrap().join("build.gradle"),
            "throw new GradleException('no')\n",
        );
        fs::create_dir_all(build.join("outputs/apk/debug")).unwrap();
        let (_u, counted) = crate::work_counters::measured(|| run(&c, &index_of(&build, 5)));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "no Gradle task, no sdkmanager, no emulator"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, c) = project(tmp.path());
        for p in [
            "outputs/apk/free/debug",
            "outputs/apk/paid/debug",
            "outputs/apk/free/release",
            "outputs/bundle/freeRelease",
        ] {
            fs::create_dir_all(build.join(p)).unwrap();
        }
        let units = run(&c, &index_of(&build, 5));
        let apks: Vec<(Option<String>, Option<String>)> = units
            .iter()
            .filter(|u| u.relative_path.contains("outputs/apk/"))
            .map(|u| (u.variant.features.clone(), u.variant.profile.clone()))
            .collect();
        assert_eq!(apks.len(), 3, "{apks:?}");
        assert!(apks.contains(&(Some("free".into()), Some("debug".into()))));
        assert!(apks.contains(&(Some("paid".into()), Some("debug".into()))));
        assert!(apks.contains(&(Some("free".into()), Some("release".into()))));
        let bundle = units
            .iter()
            .find(|u| u.relative_path.ends_with("bundle/freeRelease"))
            .unwrap();
        assert_eq!(bundle.variant.features.as_deref(), Some("free"));
        assert_eq!(bundle.variant.profile.as_deref(), Some("release"));
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, c) = project(tmp.path());
        fs::create_dir_all(build.join("intermediates/dex/debug")).unwrap();
        let units = run(&c, &index_of(&build, 1));
        let dex = units
            .iter()
            .find(|u| u.relative_path.ends_with("intermediates/dex"))
            .unwrap();
        assert_eq!(dex.time_source, TimeSource::FoldedDirectoryModification);
        assert_eq!(dex.action, NestedActionCapability::InspectionOnly);
        assert_eq!(dex.variant.configuration.as_deref(), Some("debug"));
        assert!(dex.consequence.as_deref().unwrap().contains("regenerates"));
    }

    #[test]
    fn gradle_conventions_are_shared_and_android_layout_added() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, c) = project(tmp.path());
        for p in [
            "tmp/kotlin-classes",
            "generated/source",
            "outputs/mapping/release",
            "test-results/testDebugUnitTest",
        ] {
            fs::create_dir_all(build.join(p)).unwrap();
        }
        let units = run(&c, &index_of(&build, 5));
        let role = |suffix: &str| {
            units
                .iter()
                .find(|u| u.relative_path.ends_with(suffix))
                .unwrap()
                .role
                .clone()
        };
        assert_eq!(
            role("tmp"),
            ArtifactRole::Intermediate,
            "from the shared Gradle table"
        );
        assert_eq!(
            role("generated"),
            ArtifactRole::Output,
            "from the shared Gradle table"
        );
        assert_eq!(role("mapping/release"), ArtifactRole::Metadata);
        let t = units
            .iter()
            .find(|u| u.relative_path.ends_with("testDebugUnitTest"))
            .unwrap();
        assert_eq!(t.role, ArtifactRole::TestOutput);
        assert_eq!(t.variant.profile.as_deref(), Some("debug"));
    }

    #[test]
    fn a_non_android_gradle_module_is_not_claimed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Lib");
        put(&root.join("settings.gradle"), "");
        fs::create_dir_all(root.join("core/build")).unwrap();
        let claimed = Adapter.containers(&root, &[root.join("core/build")]);
        assert!(
            claimed.is_empty(),
            "no AndroidManifest.xml, so `core/build` stays Gradle's: {claimed:?}"
        );
    }

    #[test]
    fn sdk_packages_and_avds_are_installations_and_state_never_output() {
        let tmp = tempfile::tempdir().unwrap();
        let images = tmp.path().join("sdk/system-images");
        put(
            &images.join("android-34/google_apis/arm64-v8a/source.properties"),
            "Pkg.Revision=12\n",
        );
        let c = BuildContainer::shared_store_of(
            "android",
            images.clone(),
            BuildStoreKind::AndroidSdkPackages,
        );
        let units = run(&c, &index_of(&images, 5));
        let img = units
            .iter()
            .find(|u| u.path.ends_with("arm64-v8a"))
            .unwrap();
        assert_eq!(img.role, ArtifactRole::Installation);
        assert_eq!(img.variant.architecture.as_deref(), Some("arm64-v8a"));
        assert_eq!(img.variant.configuration.as_deref(), Some("google_apis"));

        let avd_root = tmp.path().join("avd");
        let avd = avd_root.join("Pixel_8.avd");
        put(
            &avd.join("config.ini"),
            "abi.type=arm64-v8a\nimage.sysdir.1=system-images/android-34/google_apis/arm64-v8a/\n",
        );
        put(&avd.join("multiinstance.lock"), "");
        let c = BuildContainer::shared_store_of(
            "android",
            avd_root.clone(),
            BuildStoreKind::AndroidVirtualDevices,
        );
        let units = run(&c, &index_of(&avd_root, 5));
        let a = units.iter().find(|u| u.path == avd).unwrap();
        assert_eq!(a.role.family(), RoleFamily::State);
        assert!(
            a.writer_lock.is_some(),
            "the emulator's lock is active-writer evidence"
        );
        assert!(units.iter().all(|u| u.role.family() != RoleFamily::Outputs));
    }

    #[test]
    fn variant_names_split_only_on_known_build_types() {
        assert_eq!(
            split_variant("freeDebug"),
            (Some("free".into()), Some("debug".into()), None)
        );
        assert_eq!(
            split_variant("debugAndroidTest"),
            (None, Some("debug".into()), Some("androidTest"))
        );
        assert_eq!(split_variant("somethingelse"), (None, None, None));
        assert_eq!(
            split_variant("rerelease"),
            (None, None, None),
            "no capital boundary, no split"
        );
    }
}
