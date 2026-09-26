//! Go build outputs, vendored modules, and the module and build caches
//! (#69).
//!
//! Go keeps three stores with three different meanings, all documented
//! by `go help cache`/`go help modules`:
//!
//! * `GOMODCACHE/cache/download` -- the raw downloads: one `@v/`
//!   directory per module holding each version's `.zip`, `.mod` and
//!   `.info`. Module identity is the escaped path (`!` before an
//!   upper-case letter), version from the file name.
//! * `GOMODCACHE` itself -- the extracted, read-only source trees,
//!   `<module>@<version>/`. The same identities, unpacked.
//! * `GOCACHE` -- the build cache: directories `00`..`ff` of entries
//!   named by a hash of an action's inputs. **An entry's name identifies
//!   no package, no test binary and no build**, and nothing on disk says
//!   whether a later build will read it; the adapter aggregates by bucket
//!   and says exactly that, and reports Go's own trim record (`trim.txt`)
//!   as the tool fact it is.
//!
//! # What is never done
//!
//! No `go` command runs -- not `go env`, not `go list`, never
//! `go generate`. Paths come from the detector, identities from names.

use super::layout::{name_of, versionish};
use super::{
    BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, FoldedDir, NestedUnitBuilder,
};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// How many modules in `cache/download` get their `@v/` directory
/// listed (to name versions), largest first. A cache with ten thousand
/// modules is ten thousand listings; the rest are sized and say so.
const MODULE_LISTING_BUDGET: usize = 2_000;

/// How deep a module path can go looking for its `@version` (or `@v`)
/// component. `github.com/org/repo/v2` is four.
const MAX_MODULE_DEPTH: usize = 8;

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "go"
    }

    fn name(&self) -> &'static str {
        "Go"
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
            BuildStoreKind::GoModuleCache,
            BuildStoreKind::GoModuleDownloads,
            BuildStoreKind::GoBuildCache,
        ]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        if !crate::fs_gate::is_file(project_root.join("go.mod")) {
            return Vec::new();
        }
        let goreleaser = [".goreleaser.yml", ".goreleaser.yaml"]
            .iter()
            .any(|m| crate::fs_gate::is_file(project_root.join(m)));
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "go",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        claim(project_root.join("vendor"));
        claim(project_root.join("bin"));
        if goreleaser {
            claim(project_root.join("dist"));
        }
        for c in candidates {
            if matches!(name_of(c), "vendor" | "bin") {
                claim(c.clone());
            }
        }
        out
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        if container.shared {
            return match container.store_kind {
                Some(BuildStoreKind::GoModuleDownloads) => identify_downloads(container, ctx),
                Some(BuildStoreKind::GoBuildCache) => identify_build_cache(container, ctx),
                _ => identify_module_cache(container, ctx),
            };
        }
        match name_of(&container.path) {
            "vendor" => identify_vendor(container, ctx),
            "dist" => identify_goreleaser_dist(container, ctx),
            _ => identify_bin(container, ctx),
        }
    }
}

/// Go's module path escaping: `!x` is an upper-case `X`
/// (`golang.org/x/mod/module.EscapePath`). Anything else unescaped
/// passes through; an invalid escape is `None`, never a guess.
fn unescape_module_path(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '!' {
            let next = chars.next()?;
            if !next.is_ascii_lowercase() {
                return None;
            }
            out.push(next.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// `golang.org/toolchain@v0.0.1-go1.22.1.darwin-arm64`: a Go toolchain
/// the `go` command downloaded (GOTOOLCHAIN), not a dependency.
fn toolchain_of(module: &str, version: &str) -> Option<(String, String)> {
    if module != "golang.org/toolchain" {
        return None;
    }
    // `v0.0.1-go1.22.1.darwin-arm64`: the version keeps its dots, the
    // platform is the trailing `<os>-<arch>`.
    let rest = version.split_once("-go")?.1;
    match rest.rsplit_once('.') {
        Some((v, platform)) if platform.contains('-') => {
            Some((format!("go{v}"), platform.to_string()))
        }
        _ => Some((format!("go{rest}"), String::new())),
    }
}

// ---------------------------------------------------------------------
// Project-local
// ---------------------------------------------------------------------

fn root_unit(
    container: &BuildContainer,
    ctx: &BuildCtx,
    role: ArtifactRole,
    reason: &str,
    consequence: &str,
) -> NestedArtifact {
    NestedUnitBuilder::container_root(container, ctx, role)
        .supported_with_reason(reason)
        .evidence("go-layout", reason.to_string(), Confidence::Medium)
        .consequence(consequence)
        .build()
}

/// `vendor/`: the module list from `vendor/modules.txt` (the file `go mod
/// vendor` writes, read bounded), and one unit per vendored module whose
/// directory the walk measured.
fn identify_vendor(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![root_unit(
        container,
        ctx,
        ArtifactRole::InstalledDependencies,
        "`vendor/` holds the module sources `go mod vendor` copied",
        "recreate with `go mod vendor` -- needs module proxy access",
    )];
    let modules: Vec<(String, String)> = match ctx.manifest(&container.path.join("modules.txt")) {
        Some(m) if !m.truncated => m
            .text
            .lines()
            .filter_map(|l| {
                let rest = l.strip_prefix("# ")?;
                let mut parts = rest.split_whitespace();
                let module = parts.next()?;
                let version = parts.next().filter(|v| versionish(v))?;
                Some((module.to_string(), version.to_string()))
            })
            .collect(),
        _ => {
            if let Some(root) = units.first_mut() {
                *root = NestedUnitBuilder::amend(root.clone())
                    .limit(
                        "vendor/modules.txt is missing, unreadable or larger than the manifest \
                         cap: the vendored modules are sized as one tree, not identified",
                    )
                    .build();
            }
            Vec::new()
        }
    };
    for (module, version) in modules {
        let dir = container.path.join(&module);
        let Some(d) = ctx.folded().get(&dir) else {
            continue;
        };
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::InstalledDependencies,
                d,
                format!("`vendor/{module}` is listed in vendor/modules.txt"),
                "recreate with `go mod vendor` -- needs module proxy access",
            )
            .variant(ArtifactVariant {
                package: Some(module.clone()),
                version: Some(version.clone()),
                unknowns: vec!["build-generation".into()],
                ..Default::default()
            })
            .evidence(
                "go-modules-txt",
                format!("{module}@{version}"),
                Confidence::High,
            )
            .build(),
        );
    }
    units
}

/// `bin/`: the binaries `go build -o bin/...` wrote, one unit per file,
/// named by the file (the output's own name, not a guessed package).
fn identify_bin(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![root_unit(
        container,
        ctx,
        ArtifactRole::Output,
        "`bin/` beside a go.mod is where this project's build writes its binaries",
        "rebuild with this project's build (commonly `go build -o bin/ ./...`)",
    )];
    let entries = ctx.list(&container.path);
    if entries.is_empty()
        && let Some(root) = units.first_mut()
    {
        *root = NestedUnitBuilder::amend(root.clone())
            .limit("no binaries are present: nothing has been built here, or it was removed")
            .build();
    }
    for e in entries {
        let path = container.path.join(&e.name);
        if e.is_dir {
            if let Some(d) = ctx.folded().get(&path) {
                units.push(
                    NestedUnitBuilder::unknown_dir(
                        container,
                        d,
                        format!("`bin/{}` is a directory, not a built binary", e.name),
                    )
                    .build(),
                );
            }
            continue;
        }
        let Some(meta) = ctx.stat(&path) else {
            continue;
        };
        units.push(
            NestedUnitBuilder::new(container, ArtifactRole::Output, path)
                .from_file_metadata(&meta)
                .supported_with_reason(format!(
                    "`bin/{}` is a file in the build's output directory",
                    e.name
                ))
                .variant(ArtifactVariant {
                    target: Some(e.name.clone()),
                    unknowns: vec!["build-generation".into()],
                    ..Default::default()
                })
                .limit("a binary does not record which build or commit produced it here")
                .consequence("rebuild with this project's build")
                .build(),
        );
    }
    units
}

/// goreleaser's `dist/<name>_<os>_<arch>[_<variant>]/`: one unit per
/// target, so two platforms never collapse.
fn identify_goreleaser_dist(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![root_unit(
        container,
        ctx,
        ArtifactRole::Output,
        "`dist/` beside a goreleaser config is goreleaser's output directory",
        "recreate with `goreleaser build` (or `release`)",
    )];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let parts: Vec<&str> = name.split('_').collect();
        let b = if parts.len() >= 3 {
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Output,
                child,
                format!("`dist/{name}` is one goreleaser build target"),
                "recreate with `goreleaser build`",
            )
            .variant(ArtifactVariant {
                target: Some(parts[0].to_string()),
                configuration: Some(parts[1].to_string()),
                architecture: Some(parts[2..].join("_")),
                ..Default::default()
            })
        } else {
            NestedUnitBuilder::unknown_dir(
                container,
                child,
                format!("`dist/{name}` is not named `<build>_<os>_<arch>`"),
            )
        };
        units.push(b.build());
    }
    units
}

// ---------------------------------------------------------------------
// The module cache
// ---------------------------------------------------------------------

/// Directories under `root` down to the first component holding `@`
/// (`<module>@<version>`), or named exactly `stop` (`@v`), within the
/// depth bound. Returns `(dir, path-from-root)`.
fn module_dirs<'a>(
    ctx: &'a BuildCtx,
    root: &Path,
    stop_at: &dyn Fn(&str) -> bool,
) -> (Vec<&'a FoldedDir>, Vec<&'a FoldedDir>) {
    let mut found = Vec::new();
    let mut too_deep = Vec::new();
    let mut frontier: Vec<(&FoldedDir, usize)> = ctx
        .folded()
        .children(root)
        .into_iter()
        .map(|d| (d, 1))
        .collect();
    while let Some((dir, depth)) = frontier.pop() {
        if stop_at(name_of(&dir.path)) {
            found.push(dir);
            continue;
        }
        if depth >= MAX_MODULE_DEPTH {
            too_deep.push(dir);
            continue;
        }
        frontier.extend(
            ctx.folded()
                .children(&dir.path)
                .into_iter()
                .map(|d| (d, depth + 1)),
        );
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    (found, too_deep)
}

fn identify_module_cache(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::SharedStoreEntry)
            .supported_with_reason(
                "Go's module cache (GOMODCACHE): extracted, read-only module sources",
            )
            .membership(Membership::Unknown)
            .consequence(
                "the next build downloads and extracts the modules it needs again -- needs module \
                 proxy access; Go makes these files read-only, so removal is `go clean -modcache`",
            )
            .no_action_because("the module cache is shared by every Go build on this machine")
            .build(),
    ];
    // `cache/` beside the modules: `cache/download` is its own store
    // (excluded from this measurement); `cache/vcs` holds VCS clones.
    if let Some(cache) = ctx.folded().get(&container.path.join("cache")) {
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Intermediate,
                cache,
                "`cache/` is the module cache's own bookkeeping (VCS clones, locks); its \
                 `download/` is measured as a separate store",
                "the next direct (non-proxy) fetch clones the repositories again",
            )
            .no_action_because("the module cache is shared by every Go build on this machine")
            .build(),
        );
    }
    let top: Vec<&FoldedDir> = ctx
        .folded()
        .children(&container.path)
        .into_iter()
        .filter(|d| name_of(&d.path) != "cache")
        .collect();
    let mut found: Vec<&FoldedDir> = Vec::new();
    let mut too_deep: Vec<&FoldedDir> = Vec::new();
    for t in top {
        if name_of(&t.path).contains('@') {
            found.push(t);
            continue;
        }
        let (f, d) = module_dirs(ctx, &t.path, &|n: &str| n.contains('@'));
        found.extend(f);
        too_deep.extend(d);
    }
    for dir in found {
        let rel = relative_path(&container.path, &dir.path);
        let Some((escaped, version)) = rel.rsplit_once('@') else {
            continue;
        };
        let b = match unescape_module_path(escaped) {
            Some(module) => {
                let (role, variant, consequence) = match toolchain_of(&module, version) {
                    Some((toolchain, platform)) => (
                        ArtifactRole::Installation,
                        ArtifactVariant {
                            package: Some(module.clone()),
                            version: Some(version.to_string()),
                            toolchain: Some(toolchain),
                            architecture: (!platform.is_empty()).then_some(platform),
                            ..Default::default()
                        },
                        "the go command downloads this toolchain again when a module's `toolchain` \
                         line asks for it -- needs network access",
                    ),
                    None => (
                        ArtifactRole::SharedStoreEntry,
                        ArtifactVariant {
                            package: Some(module.clone()),
                            version: Some(version.to_string()),
                            unknowns: vec!["build-generation".into()],
                            ..Default::default()
                        },
                        "the next build that needs this version downloads it again -- needs \
                         module proxy access",
                    ),
                };
                NestedUnitBuilder::known_dir(
                    container,
                    role,
                    dir,
                    format!("`{rel}` is an extracted module version (`<module>@<version>`)"),
                    consequence,
                )
                .variant(variant)
                .evidence(
                    "go-module-path",
                    format!("{module}@{version}"),
                    Confidence::High,
                )
            }
            None => NestedUnitBuilder::unknown_dir(
                container,
                dir,
                format!("`{rel}` is not a validly escaped module path"),
            ),
        };
        units.push(
            b.membership(Membership::Unknown)
                .no_action_because("the module cache is shared by every Go build on this machine")
                .build(),
        );
    }
    for dir in too_deep {
        units.push(
            NestedUnitBuilder::unknown_dir(
                container,
                dir,
                format!(
                    "`{}` has no `@version` component within {MAX_MODULE_DEPTH} levels",
                    relative_path(&container.path, &dir.path)
                ),
            )
            .build(),
        );
    }
    units
}

/// `cache/download/<escaped module>/@v/<version>.{zip,mod,info,ziphash}`:
/// one unit per downloaded version, from its `.zip` (or, for a version
/// only its `go.mod` was fetched for, its `.mod`).
fn identify_downloads(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::SharedStoreEntry)
            .supported_with_reason("Go's module download cache (GOMODCACHE/cache/download)")
            .membership(Membership::Unknown)
            .consequence(
                "the next build downloads the modules it needs again -- needs module proxy access",
            )
            .no_action_because("the download cache is shared by every Go build on this machine")
            .build(),
    ];
    if let Some(sumdb) = ctx.folded().get(&container.path.join("sumdb")) {
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Metadata,
                sumdb,
                "`sumdb/` caches checksum-database tiles",
                "the go command fetches the checksum tiles it needs again",
            )
            .build(),
        );
    }
    let (mut modules, too_deep) = module_dirs(ctx, &container.path, &|n: &str| n == "@v");
    modules.retain(|d| !d.path.starts_with(container.path.join("sumdb")));
    modules.sort_by_key(|a| std::cmp::Reverse(a.allocated_total));
    let budget = MODULE_LISTING_BUDGET.min(modules.len());
    for (i, at_v) in modules.iter().enumerate() {
        let Some(module_dir) = at_v.path.parent() else {
            continue;
        };
        let rel = relative_path(&container.path, module_dir);
        let Some(module) = unescape_module_path(&rel) else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    at_v,
                    format!("`{rel}` is not a validly escaped module path"),
                )
                .build(),
            );
            continue;
        };
        if i >= budget {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::SharedStoreEntry,
                    at_v,
                    format!("`{rel}/@v` holds the downloads of one module"),
                    "the next build that needs these versions downloads them again -- needs \
                     module proxy access",
                )
                .variant(ArtifactVariant {
                    package: Some(module.clone()),
                    unknowns: vec!["version".into()],
                    ..Default::default()
                })
                .limit(format!(
                    "versions not listed: only the {MODULE_LISTING_BUDGET} largest modules get a \
                     listing"
                ))
                .membership(Membership::Unknown)
                .no_action_because("the download cache is shared by every Go build on this machine")
                .build(),
            );
            continue;
        }
        let entries = ctx.list(&at_v.path);
        let mut versions: Vec<String> = entries
            .iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| {
                e.name
                    .strip_suffix(".mod")
                    .or_else(|| e.name.strip_suffix(".zip"))
                    .or_else(|| e.name.strip_suffix(".info"))
                    .map(str::to_string)
            })
            .collect();
        versions.sort();
        versions.dedup();
        for v in versions {
            let has_zip = entries.iter().any(|e| e.name == format!("{v}.zip"));
            let file = at_v.path.join(if has_zip {
                format!("{v}.zip")
            } else {
                format!("{v}.mod")
            });
            let Some(meta) = ctx.stat(&file) else {
                continue;
            };
            let companions: Vec<String> = entries
                .iter()
                .filter(|e| e.name.starts_with(&format!("{v}.")))
                .map(|e| e.name.clone())
                .collect();
            let (role, what, consequence) = if has_zip {
                (
                    ArtifactRole::SharedStoreEntry,
                    "the downloaded source archive",
                    "the next build that needs this version downloads it again -- needs module \
                     proxy access",
                )
            } else {
                (
                    ArtifactRole::Metadata,
                    "only the module's go.mod (the source was never needed)",
                    "the next module-graph load fetches this go.mod again",
                )
            };
            units.push(
                NestedUnitBuilder::new(container, role, file)
                    .from_file_metadata(&meta)
                    .supported_with_reason(format!("`{rel}/@v/{v}` is {what}"))
                    .variant(ArtifactVariant {
                        package: Some(module.clone()),
                        version: Some(v.clone()),
                        ..Default::default()
                    })
                    .evidence("go-module-path", format!("{module}@{v}"), Confidence::High)
                    .limit(format!(
                        "sized from its `{}` alone; its companions ({}) are counted in the module \
                         directory's total",
                        if has_zip { ".zip" } else { ".mod" },
                        companions.join(", ")
                    ))
                    .action_group(format!("{rel}/@v/{v}"))
                    .membership(Membership::Unknown)
                    .consequence(consequence)
                    .no_action_because(
                        "the download cache is shared by every Go build on this machine",
                    )
                    .build(),
            );
        }
    }
    for dir in too_deep {
        units.push(
            NestedUnitBuilder::unknown_dir(
                container,
                dir,
                format!(
                    "`{}` has no `@v` directory within {MAX_MODULE_DEPTH} levels",
                    relative_path(&container.path, &dir.path)
                ),
            )
            .build(),
        );
    }
    units
}

// ---------------------------------------------------------------------
// The build cache
// ---------------------------------------------------------------------

/// GOCACHE: the `00`..`ff` buckets, each opaque; Go's own `trim.txt`
/// and `testexpire.txt` as the tool facts they are.
fn identify_build_cache(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut root = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Intermediate)
        .supported_with_reason("Go's build cache (GOCACHE)")
        .membership(Membership::Unknown)
        .limit(
            "entries are named by a hash of an action's inputs: an entry identifies no package, \
             no test binary and no build, and nothing on disk says whether a later build will \
             read it",
        )
        .consequence(
            "the next builds recompile what they would have reused, and cached test results are \
             run again",
        )
        .no_action_because("the build cache is shared by every Go build on this machine");
    for (file, what) in [
        (
            "trim.txt",
            "the go command last trimmed entries it had not used recently at",
        ),
        (
            "testexpire.txt",
            "`go clean -testcache` last expired cached test results at",
        ),
    ] {
        if let Some(m) = ctx.manifest(&container.path.join(file))
            && !m.truncated
            && let Ok(secs) = m.text.trim().parse::<u64>()
        {
            root = root.evidence(
                "go-build-cache",
                format!("{what} {secs} (unix time, from {file})"),
                Confidence::High,
            );
        }
    }
    let mut units = vec![root.build()];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let bucket = name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit());
        let b = if bucket {
            NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::Intermediate,
                child,
                format!("`{name}/` is one of the build cache's 256 hash-prefix buckets"),
                "the builds whose entries hash here recompile what they would have reused",
            )
            .variant(ArtifactVariant {
                configuration: Some(format!("key prefix {name}")),
                unknowns: vec!["package".into(), "target".into(), "build-generation".into()],
                ..Default::default()
            })
            .limit("a bucket's newest modification is its newest entry's write, not a read")
        } else {
            NestedUnitBuilder::unknown_dir(
                container,
                child,
                format!("`{name}` is not a build-cache bucket"),
            )
        };
        units.push(
            b.membership(Membership::Unknown)
                .no_action_because("the build cache is shared by every Go build on this machine")
                .build(),
        );
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::NestedActionCapability;
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
        Adapter.identify(c, &BuildCtx::new(1_000, idx, &none, &cache))
    }

    fn put(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn store(path: &Path, kind: BuildStoreKind) -> BuildContainer {
        BuildContainer::shared_store_of("go", path.to_path_buf(), kind)
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let gocache = tmp.path().join("go-build");
        fs::create_dir_all(gocache.join("0a")).unwrap();
        fs::create_dir_all(gocache.join("not-a-bucket")).unwrap();
        let units = run(
            &store(&gocache, BuildStoreKind::GoBuildCache),
            &index_of(&gocache, 5),
        );
        let odd = units
            .iter()
            .find(|u| u.path == gocache.join("not-a-bucket"))
            .expect("still a row");
        assert!(!odd.coverage.supported);
        assert!(
            odd.coverage
                .limits
                .iter()
                .any(|l| l.contains("not-a-bucket"))
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(&root.join("go.mod"), "module example.com/app\n");
        let vendor = root.join("vendor");
        put(
            &vendor.join("modules.txt"),
            &format!(
                "# example.com/dep v1.0.0\n{}",
                "## explicit\n".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES / 4)
            ),
        );
        fs::create_dir_all(vendor.join("example.com/dep")).unwrap();
        let c = BuildContainer::project("go", vendor.clone(), root.to_path_buf());
        let (units, counted) = crate::work_counters::measured(|| run(&c, &index_of(&vendor, 5)));
        assert!(counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64);
        assert!(
            !units
                .iter()
                .any(|u| u.variant.package.as_deref() == Some("example.com/dep")),
            "a modules.txt that hit the cap identifies nothing from its prefix"
        );
        assert!(
            units[0]
                .coverage
                .limits
                .iter()
                .any(|l| l.contains("manifest cap"))
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(&root.join("go.mod"), "module example.com/app\n");
        put(
            &root.join("gen.go"),
            "//go:generate sh -c 'exit 7'\npackage main\n",
        );
        put(&root.join("bin/app"), "#!/bin/sh\nexit 7\n");
        let c = BuildContainer::project("go", root.join("bin"), root.to_path_buf());
        let (_u, counted) =
            crate::work_counters::measured(|| run(&c, &index_of(&root.join("bin"), 5)));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "no go command, no go generate, no binary"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // Two versions of one module, and the same version of two
        // modules whose paths differ only by case (Go escapes the upper
        // case as `!x`).
        let tmp = tempfile::tempdir().unwrap();
        let modcache = tmp.path().join("mod");
        fs::create_dir_all(modcache.join("github.com/org/lib@v1.2.0")).unwrap();
        fs::create_dir_all(modcache.join("github.com/org/lib@v1.3.0")).unwrap();
        fs::create_dir_all(modcache.join("github.com/!org/lib@v1.2.0")).unwrap();
        let units = run(
            &store(&modcache, BuildStoreKind::GoModuleCache),
            &index_of(&modcache, 5),
        );
        let mut ids: Vec<(Option<String>, Option<String>)> = units
            .iter()
            .filter(|u| u.variant.package.is_some())
            .map(|u| (u.variant.package.clone(), u.variant.version.clone()))
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                (Some("github.com/Org/lib".into()), Some("v1.2.0".into())),
                (Some("github.com/org/lib".into()), Some("v1.2.0".into())),
                (Some("github.com/org/lib".into()), Some("v1.3.0".into())),
            ]
        );
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let gocache = tmp.path().join("go-build");
        fs::create_dir_all(gocache.join("ff")).unwrap();
        put(&gocache.join("trim.txt"), "1700000000\n");
        let units = run(
            &store(&gocache, BuildStoreKind::GoBuildCache),
            &index_of(&gocache, 1),
        );
        let bucket = units.iter().find(|u| u.path == gocache.join("ff")).unwrap();
        assert_eq!(bucket.mtime_max, 1);
        assert!(
            bucket.variant.unknowns.iter().any(|x| x == "target"),
            "an opaque key never names a test binary: {:?}",
            bucket.variant
        );
        assert!(matches!(
            bucket.action,
            NestedActionCapability::Unsupported { .. }
        ));
        let root = &units[0];
        assert!(
            root.producer_evidence
                .iter()
                .any(|e| e.detail.contains("1700000000") && e.detail.contains("trim")),
            "Go's own trim record is carried as a tool fact: {:?}",
            root.producer_evidence
        );
    }

    #[test]
    fn a_download_version_is_sized_from_its_archive_and_a_mod_only_fetch_is_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("download");
        let at_v = dl.join("golang.org/x/text/@v");
        put(&at_v.join("v0.14.0.zip"), &"z".repeat(9000));
        put(&at_v.join("v0.14.0.mod"), "module golang.org/x/text\n");
        put(&at_v.join("v0.14.0.info"), "{}");
        put(&at_v.join("v0.3.0.mod"), "module golang.org/x/text\n");
        put(&at_v.join("list"), "v0.3.0\nv0.14.0\n");
        let units = run(
            &store(&dl, BuildStoreKind::GoModuleDownloads),
            &index_of(&dl, 5),
        );
        let full = units
            .iter()
            .find(|u| u.variant.version.as_deref() == Some("v0.14.0"))
            .unwrap();
        assert_eq!(full.role, ArtifactRole::SharedStoreEntry);
        assert!(full.path.ends_with("v0.14.0.zip"));
        let mod_only = units
            .iter()
            .find(|u| u.variant.version.as_deref() == Some("v0.3.0"))
            .unwrap();
        assert_eq!(mod_only.role, ArtifactRole::Metadata);
        assert_eq!(
            mod_only.variant.package.as_deref(),
            Some("golang.org/x/text")
        );
    }

    #[test]
    fn a_downloaded_toolchain_is_an_installation_not_a_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let modcache = tmp.path().join("mod");
        fs::create_dir_all(modcache.join("golang.org/toolchain@v0.0.1-go1.22.1.darwin-arm64"))
            .unwrap();
        let units = run(
            &store(&modcache, BuildStoreKind::GoModuleCache),
            &index_of(&modcache, 5),
        );
        let tc = units
            .iter()
            .find(|u| u.variant.package.as_deref() == Some("golang.org/toolchain"))
            .unwrap();
        assert_eq!(tc.role, ArtifactRole::Installation);
        assert_eq!(tc.variant.toolchain.as_deref(), Some("go1.22.1"));
        assert_eq!(tc.variant.architecture.as_deref(), Some("darwin-arm64"));
    }

    #[test]
    fn missing_binaries_are_said_and_present_ones_are_named_by_their_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(&root.join("go.mod"), "module example.com/app\n");
        fs::create_dir_all(root.join("bin")).unwrap();
        let c = BuildContainer::project("go", root.join("bin"), root.to_path_buf());
        let empty = run(&c, &index_of(&root.join("bin"), 5));
        assert!(
            empty[0]
                .coverage
                .limits
                .iter()
                .any(|l| l.contains("no binaries"))
        );
        put(&root.join("bin/server"), "\x7fELF");
        put(&root.join("bin/cli"), "\x7fELF");
        let units = run(&c, &index_of(&root.join("bin"), 5));
        let targets: Vec<Option<String>> = units
            .iter()
            .skip(1)
            .map(|u| u.variant.target.clone())
            .collect();
        assert_eq!(targets, vec![Some("cli".into()), Some("server".into())]);
    }

    #[test]
    fn module_paths_unescape_or_refuse() {
        assert_eq!(
            unescape_module_path("github.com/!burnt!sushi/toml").as_deref(),
            Some("github.com/BurntSushi/toml")
        );
        assert_eq!(unescape_module_path("bad!"), None);
        assert_eq!(unescape_module_path("bad!X"), None);
    }
}
