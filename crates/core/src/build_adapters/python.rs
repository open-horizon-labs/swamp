//! Python generated artifacts, installed environments and shared package
//! caches (#69).
//!
//! Three things that all look like "Python storage" are kept apart,
//! because removing each costs something different:
//!
//! * **generated output** -- `dist/` distributions, a setuptools
//!   `build/` tree, `*.egg-info` metadata, `__pycache__` bytecode, test
//!   and type-checker caches: a rebuild, a re-run, a slower next run;
//! * **installed environments** -- a `.venv`, a tox/nox env, uv tool
//!   environments: a reinstall, which needs the index (or the local
//!   source a package was installed from);
//! * **shared caches** -- pip's and uv's caches, and uv-managed Python
//!   installations: a re-download, and other environments may link to
//!   the same bytes.
//!
//! # What is never done
//!
//! No package is imported, no `setup.py` is run, no build hook or
//! backend is evaluated, and no interpreter is started -- not even to ask
//! its version. Identity comes from names the packaging specs define
//! (a wheel's filename, a `*.dist-info` directory's name) and from small
//! metadata files read bounded (`PKG-INFO`, `pyvenv.cfg`,
//! `top_level.txt`, `direct_url.json`). Where that runs out, the unit
//! says so.
//!
//! # Sharing with uv's cache
//!
//! uv installs a package into an environment by linking it from its
//! cache (hardlink, or a copy-on-write clone on APFS, per its link
//! mode). The walk charges each inode once, wherever it first met it, so
//! an environment and the cache are never both charged for the same
//! bytes -- and an environment uv created says that, rather than
//! presenting its total as bytes removing it would free.

use super::layout::{headers, name_of, properties, split_name_version, versionish};
use super::{
    BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, FoldedDir, NestedUnitBuilder,
};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// How many `*.dist-info` directories in one environment get their
/// `top_level.txt`/`direct_url.json` read. Each is a few hundred bytes;
/// the budget keeps a 3,000-package environment from costing 6,000
/// reads, and the rest say so.
const DIST_INFO_BUDGET: usize = 400;

/// Files at a project root that mark a Python project.
const MARKERS: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "requirements-dev.txt",
    "requirements.in",
    "Pipfile",
    "poetry.lock",
    "uv.lock",
    "tox.ini",
    "noxfile.py",
    "environment.yml",
];

/// Directory names this adapter identifies inside a Python project.
struct Known {
    name: &'static str,
    role: ArtifactRole,
    tool: &'static str,
    consequence: &'static str,
    /// Also produced by another ecosystem (`build`, `dist`): claimed
    /// only when no other ecosystem's marker is at the root.
    ambiguous: bool,
}

const KNOWN: &[Known] = &[
    Known {
        name: "dist",
        role: ArtifactRole::Output,
        tool: "a build frontend (python -m build, uv build, hatch, poetry)",
        consequence: "rebuild the distributions with this project's build frontend",
        ambiguous: true,
    },
    Known {
        name: "build",
        role: ArtifactRole::Intermediate,
        tool: "setuptools",
        consequence: "the next setuptools build recompiles and recopies what it holds",
        ambiguous: true,
    },
    Known {
        name: "__pycache__",
        role: ArtifactRole::Intermediate,
        tool: "the CPython interpreter",
        consequence: "the interpreter recompiles the bytecode on the next import",
        ambiguous: false,
    },
    Known {
        name: ".pytest_cache",
        role: ArtifactRole::Intermediate,
        tool: "pytest",
        consequence: "pytest forgets its last-failed set (`--lf`, `--ff` start from scratch)",
        ambiguous: false,
    },
    Known {
        name: ".mypy_cache",
        role: ArtifactRole::Intermediate,
        tool: "mypy",
        consequence: "the next mypy run type-checks every module instead of the changed ones",
        ambiguous: false,
    },
    Known {
        name: ".ruff_cache",
        role: ArtifactRole::Intermediate,
        tool: "Ruff",
        consequence: "the next ruff run re-lints every file instead of the changed ones",
        ambiguous: false,
    },
    Known {
        name: ".pytype",
        role: ArtifactRole::Intermediate,
        tool: "pytype",
        consequence: "the next pytype run re-analyses every module",
        ambiguous: false,
    },
    Known {
        name: ".pyre",
        role: ArtifactRole::Intermediate,
        tool: "Pyre",
        consequence: "the next Pyre run rebuilds its state",
        ambiguous: false,
    },
    Known {
        name: ".hypothesis",
        role: ArtifactRole::Intermediate,
        tool: "Hypothesis",
        consequence: "Hypothesis forgets the failing examples it saved and must find them again",
        ambiguous: false,
    },
    Known {
        name: "htmlcov",
        role: ArtifactRole::TestOutput,
        tool: "coverage.py",
        consequence: "a test run with `coverage html` regenerates it",
        ambiguous: false,
    },
    Known {
        name: ".tox",
        role: ArtifactRole::InstalledDependencies,
        tool: "tox",
        consequence: "tox recreates each environment on its next run -- needs index access",
        ambiguous: false,
    },
    Known {
        name: ".nox",
        role: ArtifactRole::InstalledDependencies,
        tool: "nox",
        consequence: "nox recreates each session environment on its next run -- needs index access",
        ambiguous: false,
    },
    Known {
        name: ".eggs",
        role: ArtifactRole::InstalledDependencies,
        tool: "setuptools (setup_requires)",
        consequence: "setuptools fetches the build requirements again -- needs index access",
        ambiguous: false,
    },
    Known {
        name: "__pypackages__",
        role: ArtifactRole::InstalledDependencies,
        tool: "PEP 582 (pdm)",
        consequence: "reinstall with the project's installer -- needs index access",
        ambiguous: false,
    },
];

/// The consequence of losing an installed environment.
const ENV_CONSEQUENCE: &str = "recreate the environment and reinstall its packages -- needs index access, or the local \
     sources editable packages were installed from";

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "python"
    }

    fn name(&self) -> &'static str {
        "Python"
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
            BuildStoreKind::PipCache,
            BuildStoreKind::UvCache,
            BuildStoreKind::UvPythonInstallations,
            BuildStoreKind::UvToolEnvironments,
        ]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        let marked = MARKERS
            .iter()
            .any(|m| crate::fs_gate::is_file(project_root.join(m)));
        // `build`/`dist` belong to whichever ecosystem's marker is at the
        // root when there is more than one (the catalog's own rule).
        let other = [
            "package.json",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "Cargo.toml",
        ]
        .iter()
        .any(|m| crate::fs_gate::is_file(project_root.join(m)));
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "python",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        let claims_name = |name: &str| {
            KNOWN
                .iter()
                .any(|k| k.name == name && !(k.ambiguous && other))
                || name.ends_with(".egg-info")
        };
        if marked {
            for k in KNOWN.iter().filter(|k| !(k.ambiguous && other)) {
                claim(project_root.join(k.name));
            }
        }
        for c in candidates {
            let name = name_of(c);
            // An environment is claimed by its own `pyvenv.cfg`, marker
            // or not: a `.venv` is a Python environment wherever it is.
            if crate::fs_gate::is_file(c.join("pyvenv.cfg")) {
                claim(c.clone());
                continue;
            }
            if marked && claims_name(name) {
                claim(c.clone());
            }
        }
        for name in [".venv", "venv", "env"] {
            let p = project_root.join(name);
            if crate::fs_gate::is_file(p.join("pyvenv.cfg")) {
                claim(p);
            }
        }
        // Loose metadata at the root (`.coverage`), claimed only when
        // present so an ordinary project gets no empty container.
        if marked && crate::fs_gate::is_file(project_root.join(".coverage")) {
            out.push(BuildContainer::project(
                "python",
                project_root.to_path_buf(),
                project_root.to_path_buf(),
            ));
        }
        out
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        if container.shared {
            return match container.store_kind {
                Some(BuildStoreKind::PipCache) => identify_pip_cache(container, ctx),
                Some(BuildStoreKind::UvPythonInstallations) => identify_uv_pythons(container, ctx),
                Some(BuildStoreKind::UvToolEnvironments) => identify_uv_tools(container, ctx),
                _ => identify_uv_cache(container, ctx),
            };
        }
        if container.project_root.as_deref() == Some(container.path.as_path()) {
            return identify_loose_metadata(container, ctx);
        }
        let name = name_of(&container.path);
        if ctx
            .stat(&container.path.join("pyvenv.cfg"))
            .is_some_and(|m| m.is_file())
        {
            return identify_env(container, ctx, &container.path, true);
        }
        match name {
            "dist" => identify_dist(container, ctx),
            "build" => identify_setuptools_build(container, ctx),
            ".mypy_cache" | ".ruff_cache" => identify_versioned_cache(container, ctx, name),
            ".tox" | ".nox" => identify_env_dir(container, ctx, name),
            ".eggs" => identify_eggs(container, ctx),
            n if n.ends_with(".egg-info") => identify_egg_info(container, ctx),
            _ => vec![project_root_unit(container, ctx)],
        }
    }
}

fn known_for(name: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.name == name)
}

/// The container's own row, identified by its name.
fn project_root_unit(container: &BuildContainer, ctx: &BuildCtx) -> NestedArtifact {
    let name = name_of(&container.path);
    let known = known_for(name);
    let b = NestedUnitBuilder::container_root(
        container,
        ctx,
        known
            .map(|k| k.role.clone())
            .unwrap_or(ArtifactRole::Metadata),
    );
    match known {
        Some(k) => b
            .supported_with_reason(format!("`{}` is {}'s documented directory", k.name, k.tool))
            .evidence(
                "python-layout",
                format!("produced by {}", k.tool),
                Confidence::Medium,
            )
            .consequence(k.consequence),
        None if name.ends_with(".egg-info") => b
            .supported_with_reason("`*.egg-info` is setuptools' project metadata directory")
            .consequence("the next setuptools build or editable install rewrites it"),
        None => b.unsupported_layout(format!(
            "`{name}` is not a directory name this adapter identifies"
        )),
    }
    .build()
}

/// `.coverage` at the project root.
fn identify_loose_metadata(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let path = container.path.join(".coverage");
    let Some(meta) = ctx.stat(&path) else {
        return Vec::new();
    };
    if !meta.is_file() {
        return Vec::new();
    }
    vec![
        NestedUnitBuilder::new(container, ArtifactRole::TestOutput, path)
            .from_file_metadata(&meta)
            .supported_with_reason("`.coverage` is coverage.py's data file")
            .evidence("python-layout", "written by coverage.py", Confidence::High)
            .consequence("a test run under coverage writes it again")
            .build(),
    ]
}

// ---------------------------------------------------------------------
// dist/: distributions, named by the packaging specs' own filenames
// ---------------------------------------------------------------------

/// What a distribution's filename establishes, per the wheel filename
/// convention (PEP 427 / binary distribution format) and the sdist name
/// (PEP 625): `{name}-{version}(-{build})?-{python}-{abi}-{platform}.whl`
/// and `{name}-{version}.tar.gz`. Nothing else is read from the name.
#[derive(Debug, PartialEq, Eq)]
struct DistName {
    name: String,
    version: String,
    kind: &'static str,
    python: Option<String>,
    abi: Option<String>,
    platform: Option<String>,
}

fn parse_distribution(file: &str) -> Option<DistName> {
    if let Some(stem) = file.strip_suffix(".whl") {
        let parts: Vec<&str> = stem.split('-').collect();
        if parts.len() != 5 && parts.len() != 6 {
            return None;
        }
        let n = parts.len();
        if !versionish(parts[1]) {
            return None;
        }
        return Some(DistName {
            name: parts[0].to_string(),
            version: parts[1].to_string(),
            kind: "wheel",
            python: Some(parts[n - 3].to_string()),
            abi: Some(parts[n - 2].to_string()),
            platform: Some(parts[n - 1].to_string()),
        });
    }
    let stem = file
        .strip_suffix(".tar.gz")
        .or_else(|| file.strip_suffix(".zip"))?;
    let (name, version) = split_name_version(stem)?;
    Some(DistName {
        name: name.to_string(),
        version: version.to_string(),
        kind: "sdist",
        python: None,
        abi: None,
        platform: None,
    })
}

fn identify_dist(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![project_root_unit(container, ctx)];
    for entry in ctx.list(&container.path) {
        let path = container.path.join(&entry.name);
        if entry.is_dir {
            if let Some(d) = ctx.folded().get(&path) {
                units.push(
                    NestedUnitBuilder::unknown_dir(
                        container,
                        d,
                        format!("`dist/{}` is not a distribution file", entry.name),
                    )
                    .build(),
                );
            }
            continue;
        }
        let Some(meta) = ctx.stat(&path) else {
            continue;
        };
        let b =
            NestedUnitBuilder::new(container, ArtifactRole::Output, path).from_file_metadata(&meta);
        let unit = match parse_distribution(&entry.name) {
            Some(d) => {
                let variant = ArtifactVariant {
                    package: Some(d.name.clone()),
                    version: Some(d.version.clone()),
                    toolchain: d.python.clone(),
                    architecture: d.platform.clone(),
                    configuration: Some(match &d.abi {
                        Some(abi) => format!("{} ({abi})", d.kind),
                        None => d.kind.to_string(),
                    }),
                    unknowns: vec!["build-generation".into()],
                    ..Default::default()
                };
                b.variant(variant)
                    .supported_with_reason(format!(
                        "`{}` is a {} named by the packaging convention",
                        entry.name, d.kind
                    ))
                    .evidence(
                        "python-distribution-name",
                        format!("{}=={} ({})", d.name, d.version, d.kind),
                        Confidence::High,
                    )
                    .limit(
                        "identity is read from the filename the packaging convention defines; \
                         the archive's own METADATA is inside it and is not opened",
                    )
                    .consequence("rebuild with this project's build frontend")
            }
            None => b.unsupported_layout(format!(
                "`{}` is not named like a wheel or a source distribution",
                entry.name
            )),
        };
        units.push(unit.build());
    }
    units
}

// ---------------------------------------------------------------------
// build/: setuptools' build tree
// ---------------------------------------------------------------------

/// `lib`, `lib.<platform>-<python>`, `temp.<platform>-<python>`,
/// `bdist.<platform>`: setuptools' own names, whose suffix is the
/// variant. Two interpreters' `lib.*` never collapse.
fn identify_setuptools_build(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![project_root_unit(container, ctx)];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let (head, tag) = match name.split_once('.') {
            Some((h, t)) => (h, Some(t)),
            None => (name.as_str(), None),
        };
        let (role, what, consequence) = match head {
            "lib" => (
                ArtifactRole::Output,
                "the pure and compiled modules staged for a wheel",
                "the next build stages them again",
            ),
            "temp" => (
                ArtifactRole::Intermediate,
                "object files from compiling extension modules",
                "the next build recompiles the extension modules",
            ),
            "bdist" => (
                ArtifactRole::Intermediate,
                "the staging tree of a binary distribution",
                "the next bdist build restages it",
            ),
            "scripts" => (
                ArtifactRole::Output,
                "console scripts staged for installation",
                "the next build stages them again",
            ),
            _ => {
                units.push(
                    NestedUnitBuilder::unknown_dir(
                        container,
                        child,
                        format!("`build/{name}` is not a setuptools build directory"),
                    )
                    .build(),
                );
                continue;
            }
        };
        let mut variant = ArtifactVariant::default();
        match tag {
            Some(t) => {
                // `macosx-11.0-arm64-cpython-311`: the interpreter tag is
                // the trailing `<impl>-<nodot>` pair when present.
                let pieces: Vec<&str> = t.split('-').collect();
                if pieces.len() >= 2 && pieces[pieces.len() - 1].chars().all(|c| c.is_ascii_digit())
                {
                    variant.toolchain = Some(pieces[pieces.len() - 2..].join("-"));
                    variant.architecture = Some(pieces[..pieces.len() - 2].join("-"));
                } else {
                    variant.architecture = Some(t.to_string());
                    variant.unknowns.push("toolchain".into());
                }
            }
            None => variant.unknowns.push("toolchain".into()),
        }
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                role,
                child,
                format!("`build/{name}` holds {what}"),
                consequence,
            )
            .variant(variant)
            .build(),
        );
    }
    units
}

// ---------------------------------------------------------------------
// Tool caches keyed by a version directory
// ---------------------------------------------------------------------

/// `.mypy_cache/<python-version>/`, `.ruff_cache/<ruff-version>/`: one
/// unit per version, so two interpreters' or two tools' caches never
/// collapse into one.
fn identify_versioned_cache(
    container: &BuildContainer,
    ctx: &BuildCtx,
    name: &str,
) -> Vec<NestedArtifact> {
    let mut units = vec![project_root_unit(container, ctx)];
    let tool = if name == ".mypy_cache" {
        "python"
    } else {
        "ruff"
    };
    for child in ctx.folded().children(&container.path) {
        let v = name_of(&child.path).to_string();
        if versionish(&v) {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Intermediate,
                    child,
                    format!("`{name}/{v}` is the cache for {tool} {v}"),
                    known_for(name).map(|k| k.consequence).unwrap_or_default(),
                )
                .variant(ArtifactVariant {
                    toolchain: Some(format!("{tool} {v}")),
                    ..Default::default()
                })
                .build(),
            );
        } else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!("`{name}/{v}` is not a per-version cache directory"),
                )
                .build(),
            );
        }
    }
    units
}

// ---------------------------------------------------------------------
// Environments
// ---------------------------------------------------------------------

/// What `pyvenv.cfg` says about an environment, read bounded.
struct EnvFacts {
    version: Option<String>,
    implementation: Option<String>,
    home: Option<String>,
    uv: Option<String>,
    unreadable: bool,
}

fn env_facts(ctx: &BuildCtx, env: &Path) -> EnvFacts {
    let Some(m) = ctx.manifest(&env.join("pyvenv.cfg")) else {
        return EnvFacts {
            version: None,
            implementation: None,
            home: None,
            uv: None,
            unreadable: true,
        };
    };
    if m.truncated {
        return EnvFacts {
            version: None,
            implementation: None,
            home: None,
            uv: None,
            unreadable: true,
        };
    }
    let p = properties(&m.text, false);
    EnvFacts {
        version: p.get("version").or_else(|| p.get("version_info")).cloned(),
        implementation: p.get("implementation").cloned(),
        home: p.get("home").cloned(),
        uv: p.get("uv").cloned(),
        unreadable: false,
    }
}

/// `.tox/` / `.nox/`: one environment per child, each identified the
/// same way as a `.venv`.
fn identify_env_dir(container: &BuildContainer, ctx: &BuildCtx, name: &str) -> Vec<NestedArtifact> {
    let mut units = vec![project_root_unit(container, ctx)];
    for child in ctx.folded().children(&container.path) {
        let child_name = name_of(&child.path).to_string();
        if ctx
            .stat(&child.path.join("pyvenv.cfg"))
            .is_some_and(|m| m.is_file())
        {
            units.extend(identify_env(container, ctx, &child.path, false));
        } else if child_name == "log" || child_name == ".pkg" || child_name == ".tmp" {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Metadata,
                    child,
                    format!("`{name}/{child_name}` is {name}'s own bookkeeping"),
                    "the next run rewrites it",
                )
                .build(),
            );
        } else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!("`{name}/{child_name}` has no pyvenv.cfg: not an environment"),
                )
                .build(),
            );
        }
    }
    units
}

/// One environment: its interpreter from `pyvenv.cfg`, and its
/// `site-packages` entries with identity from `*.dist-info`.
fn identify_env(
    container: &BuildContainer,
    ctx: &BuildCtx,
    env: &Path,
    is_container: bool,
) -> Vec<NestedArtifact> {
    let facts = env_facts(ctx, env);
    let mut variant = ArtifactVariant::default();
    match (&facts.implementation, &facts.version) {
        (Some(i), Some(v)) => variant.toolchain = Some(format!("{} {v}", i.to_lowercase())),
        (None, Some(v)) => variant.toolchain = Some(format!("python {v}")),
        _ => variant.unknowns.push("toolchain".into()),
    }
    variant.unknowns.push("build-generation".into());
    let mut root = if is_container {
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::InstalledDependencies)
    } else {
        match ctx.folded().get(env) {
            Some(d) => NestedUnitBuilder::new(
                container,
                ArtifactRole::InstalledDependencies,
                env.to_path_buf(),
            )
            .folded(d),
            None => NestedUnitBuilder::new(
                container,
                ArtifactRole::InstalledDependencies,
                env.to_path_buf(),
            )
            .is_dir(true)
            .limit("this environment was not measured by the walk this pass"),
        }
    }
    .variant(variant)
    .supported_with_reason("a virtual environment, identified by its own `pyvenv.cfg`")
    .consequence(ENV_CONSEQUENCE);
    if facts.unreadable {
        root = root.limit(
            "`pyvenv.cfg` could not be read (or is larger than the manifest cap): the \
             interpreter is unknown",
        );
    } else {
        root = root.evidence(
            "python-pyvenv-cfg",
            match &facts.version {
                Some(v) => format!("created with Python {v}"),
                None => "pyvenv.cfg names no version".to_string(),
            },
            Confidence::High,
        );
        if facts.version.is_none() {
            root = root.limit("`pyvenv.cfg` names no interpreter version");
        }
    }
    if let Some(home) = &facts.home {
        // The interpreter this environment runs is outside it. Missing
        // means the environment no longer starts -- a fact to show, not
        // a reason to call it disposable.
        if ctx.stat(Path::new(home)).is_none() {
            root = root.limit(format!(
                "the interpreter this environment was created from is not at `{home}` now; the \
                 environment cannot start until it is recreated"
            ));
        }
    }
    if let Some(uv) = &facts.uv {
        root = root
            .evidence(
                "python-pyvenv-cfg",
                format!("created by uv {uv}"),
                Confidence::High,
            )
            .membership(Membership::Unknown)
            .limit(
                "uv links packages from its cache into this environment (hardlink or \
                 copy-on-write clone, per its link mode); bytes shared with the cache are \
                 charged once, where the walk first met them",
            );
    }
    let mut units = vec![root.build()];
    units.extend(identify_site_packages(container, ctx, env));
    units
}

/// `site-packages`: one unit per top-level entry. A `*.dist-info`
/// directory carries its distribution's name and version in its own name
/// (the installed-distribution format); a package directory gets that
/// identity only through the distribution's `top_level.txt`, never by
/// matching its name to one.
fn identify_site_packages(
    container: &BuildContainer,
    ctx: &BuildCtx,
    env: &Path,
) -> Vec<NestedArtifact> {
    let mut site_dirs: Vec<&FoldedDir> = Vec::new();
    for lib in ["lib", "Lib", "lib64"] {
        let lib = env.join(lib);
        if let Some(d) = ctx.folded().get(&lib.join("site-packages")) {
            site_dirs.push(d);
        }
        for py in ctx.folded().children(&lib) {
            if let Some(d) = ctx.folded().get(&py.path.join("site-packages")) {
                site_dirs.push(d);
            }
        }
    }
    let mut units = Vec::new();
    for site in site_dirs {
        let python = site
            .path
            .parent()
            .map(name_of)
            .filter(|n| n.starts_with("python"))
            .map(str::to_string);
        let entries = ctx.folded().children(&site.path);
        // Distribution identities, and which top-level names each owns.
        let mut owner: HashMap<String, (String, String)> = HashMap::new();
        let mut local: HashMap<String, String> = HashMap::new();
        let mut dist_infos: Vec<&FoldedDir> = entries
            .iter()
            .copied()
            .filter(|e| name_of(&e.path).ends_with(".dist-info"))
            .collect();
        dist_infos.sort_by(|a, b| a.path.cmp(&b.path));
        let skipped = dist_infos.len().saturating_sub(DIST_INFO_BUDGET);
        for (i, di) in dist_infos.iter().enumerate() {
            let stem = name_of(&di.path).trim_end_matches(".dist-info");
            let Some((name, version)) = split_name_version(stem) else {
                continue;
            };
            if i >= DIST_INFO_BUDGET {
                continue;
            }
            if let Some(m) = ctx.manifest(&di.path.join("top_level.txt"))
                && !m.truncated
            {
                for top in m.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    owner.insert(top.to_string(), (name.to_string(), version.to_string()));
                }
            }
            if let Some(m) = ctx.manifest(&di.path.join("direct_url.json"))
                && !m.truncated
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&m.text)
                && let Some(url) = v.get("url").and_then(|u| u.as_str())
                && url.starts_with("file://")
            {
                local.insert(
                    name.to_string(),
                    url.trim_start_matches("file://").to_string(),
                );
            }
        }
        for e in entries {
            let entry = name_of(&e.path).to_string();
            if entry == "__pycache__" {
                units.push(
                    NestedUnitBuilder::known_dir(
                        container,
                        ArtifactRole::Intermediate,
                        e,
                        "`site-packages/__pycache__` is bytecode for top-level modules",
                        "the interpreter recompiles it on the next import",
                    )
                    .build(),
                );
                continue;
            }
            let mut variant = ArtifactVariant {
                toolchain: python.clone(),
                ..Default::default()
            };
            let mut b = NestedUnitBuilder::new(
                container,
                ArtifactRole::InstalledDependencies,
                e.path.clone(),
            )
            .folded(e)
            .consequence("reinstall this distribution into the environment -- needs index access")
            .supported_with_reason("a top-level entry of an environment's site-packages");
            let identity = if let Some(stem) = entry.strip_suffix(".dist-info") {
                split_name_version(stem).map(|(n, v)| (n.to_string(), v.to_string()))
            } else {
                owner.get(&entry).cloned()
            };
            match &identity {
                Some((name, version)) => {
                    variant.package = Some(name.clone());
                    variant.version = Some(version.clone());
                    b = b.evidence(
                        "python-dist-info",
                        format!("{name}=={version}"),
                        Confidence::High,
                    );
                    if let Some(src) = local.get(name) {
                        b = b
                            .evidence(
                                "python-direct-url",
                                format!("installed from the local source `{src}`"),
                                Confidence::High,
                            )
                            .consequence(format!(
                                "reinstall from the local source at `{src}` (for example \
                                 `pip install -e {src}`)"
                            ));
                    }
                }
                None => {
                    variant.unknowns.push("package".into());
                    variant.unknowns.push("version".into());
                    b = b.limit(if skipped > 0 {
                        format!(
                            "no distribution's top_level.txt names this entry (only {DIST_INFO_BUDGET} \
                             of {} dist-info directories were read)",
                            dist_infos.len()
                        )
                    } else {
                        "no distribution's top_level.txt names this entry; its identity is not \
                         guessed from the directory name"
                            .to_string()
                    });
                }
            }
            variant.unknowns.push("build-generation".into());
            units.push(b.variant(variant).build());
        }
    }
    units
}

/// `.eggs/<name>-<version>-py<X.Y>.egg`: setup_requires fetches.
fn identify_eggs(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![project_root_unit(container, ctx)];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let parsed = name.strip_suffix(".egg").and_then(|stem| {
            let mut parts = stem.splitn(3, '-');
            let n = parts.next()?;
            let v = parts.next().filter(|v| versionish(v))?;
            Some((
                n.to_string(),
                v.to_string(),
                parts.next().map(str::to_string),
            ))
        });
        let b = match parsed {
            Some((n, v, py)) => NestedUnitBuilder::known_dir(
                container,
                ArtifactRole::InstalledDependencies,
                child,
                format!("`.eggs/{name}` is an egg setuptools fetched"),
                "setuptools fetches it again -- needs index access",
            )
            .variant(ArtifactVariant {
                package: Some(n),
                version: Some(v),
                toolchain: py,
                ..Default::default()
            }),
            None => NestedUnitBuilder::unknown_dir(
                container,
                child,
                format!("`.eggs/{name}` is not named like an egg"),
            ),
        };
        units.push(b.build());
    }
    units
}

/// `*.egg-info`: identity from its own `PKG-INFO`, read bounded.
fn identify_egg_info(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut b = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Metadata)
        .supported_with_reason("`*.egg-info` is setuptools' project metadata directory")
        .consequence("the next setuptools build or editable install rewrites it");
    let mut variant = ArtifactVariant::default();
    match ctx.manifest(&container.path.join("PKG-INFO")) {
        Some(m) if !m.truncated => {
            let h = headers(&m.text);
            variant.package = h.get("Name").cloned();
            variant.version = h.get("Version").cloned();
            if variant.package.is_none() {
                variant.unknowns.push("package".into());
                b = b.limit("PKG-INFO names no distribution");
            }
            if variant.version.is_none() {
                variant.unknowns.push("version".into());
                b = b.limit("PKG-INFO names no version");
            }
            if let (Some(n), Some(v)) = (&variant.package, &variant.version) {
                b = b.evidence("python-pkg-info", format!("{n}=={v}"), Confidence::High);
            }
        }
        _ => {
            variant.unknowns.push("package".into());
            variant.unknowns.push("version".into());
            b = b.limit(
                "PKG-INFO is missing, unreadable or larger than the manifest cap: the \
                 distribution is unknown, and not guessed from the directory name",
            );
        }
    }
    vec![b.variant(variant).build()]
}

// ---------------------------------------------------------------------
// Shared stores
// ---------------------------------------------------------------------

fn store_root(
    container: &BuildContainer,
    ctx: &BuildCtx,
    what: &str,
    consequence: &str,
) -> NestedArtifact {
    NestedUnitBuilder::container_root(container, ctx, ArtifactRole::SharedStoreEntry)
        .supported_with_reason(format!("{what}, shared by every project on this machine"))
        .evidence("python-store", what.to_string(), Confidence::Medium)
        .membership(Membership::Unknown)
        .consequence(consequence)
        .no_action_because("this store is shared across projects and environments")
        .build()
}

/// A cache category directory name with its layout version split off:
/// `wheels-v5` -> (`wheels`, `5`), `http-v2` -> (`http`, `2`).
fn layout_version(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once("-v") {
        Some((h, v)) if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) => (h, Some(v)),
        _ => (name, None),
    }
}

/// pip's cache: `http`/`http-v2` (responses, i.e. downloaded files),
/// `wheels` (wheels pip built locally, keyed by a hash of their link),
/// `selfcheck`.
fn identify_pip_cache(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        "pip's download and wheel cache",
        "pip downloads (and builds) packages again on the next install -- needs index access",
    )];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let (category, version) = layout_version(&name);
        let known = match category {
            "http" => Some((
                ArtifactRole::SharedStoreEntry,
                "cached index responses and downloaded distributions",
                "pip downloads these again -- needs index access",
            )),
            "wheels" => Some((
                ArtifactRole::SharedStoreEntry,
                "wheels pip built locally from source distributions, keyed by a hash of their link",
                "pip rebuilds these wheels from source on the next install",
            )),
            "selfcheck" => Some((
                ArtifactRole::Metadata,
                "pip's own version-check state",
                "pip rewrites it",
            )),
            _ => None,
        };
        let b = match known {
            Some((role, what, consequence)) => NestedUnitBuilder::known_dir(
                container,
                role,
                child,
                format!("`{name}` holds {what}"),
                consequence,
            )
            .variant(ArtifactVariant {
                configuration: version.map(|v| format!("{category} layout {v}")),
                ..Default::default()
            })
            .limit("entries are keyed by hashes: which project needed one is not recorded")
            .membership(Membership::Unknown)
            .no_action_because("the cache is shared by every pip install on this machine"),
            None => NestedUnitBuilder::unknown_dir(
                container,
                child,
                format!("`{name}` is not a pip cache directory this adapter identifies"),
            ),
        };
        units.push(b.build());
    }
    units
}

/// uv's cache. Buckets are versioned (`wheels-v5`, `sdists-v9`); within
/// the package buckets, `pypi/<name>/` carries a package name (the
/// distribution identity uv keyed it by) and `path/<hash>/` is a build
/// from a local source tree.
fn identify_uv_cache(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![store_root(
        container,
        ctx,
        "uv's cache",
        "uv downloads, builds and unpacks packages again on the next sync -- needs index access; \
         environments that link into the cache keep working until they are recreated",
    )];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let (bucket, version) = layout_version(&name);
        let known: Option<(ArtifactRole, &str, &str)> = match bucket {
            "archive" => Some((
                ArtifactRole::SharedStoreEntry,
                "unpacked wheels, content-addressed: what environments link to",
                "uv unpacks these again from downloaded wheels",
            )),
            "wheels" | "built-wheels" => Some((
                ArtifactRole::SharedStoreEntry,
                "downloaded and locally built wheels",
                "uv downloads or rebuilds these -- needs index access or the source",
            )),
            "sdists" => Some((
                ArtifactRole::SharedStoreEntry,
                "source distributions and the wheels uv built from them",
                "uv downloads and rebuilds these -- needs index access or the source",
            )),
            "simple" => Some((
                ArtifactRole::Metadata,
                "cached index (simple API) responses",
                "uv queries the index again",
            )),
            "git" => Some((
                ArtifactRole::Intermediate,
                "git checkouts of git dependencies",
                "uv clones these repositories again -- needs access to them",
            )),
            "interpreter" => Some((
                ArtifactRole::Metadata,
                "facts uv recorded about interpreters it inspected",
                "uv re-inspects the interpreters",
            )),
            "environments" => Some((
                ArtifactRole::InstalledDependencies,
                "cached environments for `uv run` scripts and `uvx` tools",
                "uv recreates them on the next run -- needs index access",
            )),
            "builds" => Some((
                ArtifactRole::Intermediate,
                "isolated build environments",
                "uv recreates them for the next build",
            )),
            _ => None,
        };
        let Some((role, what, consequence)) = known else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!("`{name}` is not a uv cache bucket this adapter identifies"),
                )
                .build(),
            );
            continue;
        };
        units.push(
            NestedUnitBuilder::known_dir(
                container,
                role,
                child,
                format!("`{name}` holds {what}"),
                consequence,
            )
            .variant(ArtifactVariant {
                configuration: version.map(|v| format!("{bucket} layout {v}")),
                ..Default::default()
            })
            .membership(Membership::Unknown)
            .no_action_because("the cache is shared by every uv environment on this machine")
            .build(),
        );
        if matches!(bucket, "wheels" | "built-wheels" | "sdists") {
            units.extend(identify_uv_packages(container, ctx, &child.path, &name));
        }
    }
    units
}

/// `<bucket>/pypi/<name>/` and `<bucket>/path/<hash>/`.
fn identify_uv_packages(
    container: &BuildContainer,
    ctx: &BuildCtx,
    bucket: &Path,
    bucket_name: &str,
) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for index in ctx.folded().children(bucket) {
        let index_name = name_of(&index.path).to_string();
        match index_name.as_str() {
            "pypi" | "index" => {
                for pkg in ctx.folded().children(&index.path) {
                    let pkg_name = name_of(&pkg.path).to_string();
                    units.push(
                        NestedUnitBuilder::known_dir(
                            container,
                            ArtifactRole::SharedStoreEntry,
                            pkg,
                            format!(
                                "`{bucket_name}/{index_name}/{pkg_name}` is uv's cache for one \
                                 package, keyed by its normalized name"
                            ),
                            "uv downloads this package again -- needs index access",
                        )
                        .variant(ArtifactVariant {
                            package: (index_name == "pypi").then(|| pkg_name.clone()),
                            unknowns: if index_name == "pypi" {
                                vec!["version".into()]
                            } else {
                                vec!["package".into(), "version".into()]
                            },
                            ..Default::default()
                        })
                        .membership(Membership::Unknown)
                        .no_action_because(
                            "the cache is shared by every uv environment on this machine",
                        )
                        .build(),
                    );
                }
            }
            "path" | "git" | "url" => {
                units.push(
                    NestedUnitBuilder::known_dir(
                        container,
                        ArtifactRole::SharedStoreEntry,
                        index,
                        format!(
                            "`{bucket_name}/{index_name}` holds builds from {} sources",
                            if index_name == "path" {
                                "local"
                            } else {
                                "non-index"
                            }
                        ),
                        if index_name == "path" {
                            "uv rebuilds these from the local source trees they came from"
                        } else {
                            "uv fetches and rebuilds these from their source"
                        },
                    )
                    .limit(
                        "entries are keyed by a hash of the source; the source location is \
                         recorded inside the entry, which is not read",
                    )
                    .membership(Membership::Unknown)
                    .no_action_because(
                        "the cache is shared by every uv environment on this machine",
                    )
                    .build(),
                );
            }
            _ => units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    index,
                    format!("`{bucket_name}/{index_name}` is not an index directory"),
                )
                .build(),
            ),
        }
    }
    units
}

/// `cpython-3.12.4-macos-aarch64-none`: uv's managed interpreters, one
/// unit per installation, its version and platform from its own name.
fn identify_uv_pythons(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Installation)
            .supported_with_reason("uv's managed Python installations")
            .consequence("uv downloads the interpreters again when a project asks for them")
            .no_action_because("environments created from these interpreters depend on them")
            .build(),
    ];
    for child in ctx.folded().children(&container.path) {
        let name = name_of(&child.path).to_string();
        let parts: Vec<&str> = name.split('-').collect();
        if parts.len() >= 4 && versionish(parts[1]) {
            units.push(
                NestedUnitBuilder::known_dir(
                    container,
                    ArtifactRole::Installation,
                    child,
                    format!("`{name}` is a uv-managed {} installation", parts[0]),
                    format!(
                        "uv downloads {} {} again when a project asks for it; environments \
                         created from it stop starting until then",
                        parts[0], parts[1]
                    ),
                )
                .variant(ArtifactVariant {
                    toolchain: Some(format!("{} {}", parts[0], parts[1])),
                    version: Some(parts[1].to_string()),
                    configuration: Some(parts[2].to_string()),
                    architecture: Some(parts[3].to_string()),
                    ..Default::default()
                })
                .no_action_because("environments created from this interpreter depend on it")
                .build(),
            );
        } else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!("`{name}` is not named like a uv Python installation"),
                )
                .build(),
            );
        }
    }
    units
}

/// `uv tool install` environments: one environment per tool.
fn identify_uv_tools(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![
        NestedUnitBuilder::container_root(container, ctx, ArtifactRole::InstalledDependencies)
            .supported_with_reason("uv's tool environments")
            .consequence("reinstall each tool with `uv tool install` -- needs index access")
            .build(),
    ];
    for child in ctx.folded().children(&container.path) {
        if ctx
            .stat(&child.path.join("pyvenv.cfg"))
            .is_some_and(|m| m.is_file())
        {
            let tool = name_of(&child.path).to_string();
            let mut env = identify_env(container, ctx, &child.path, false);
            if let Some(first) = env.first_mut() {
                let mut v = first.variant.clone();
                v.package = Some(tool.clone());
                *first = NestedUnitBuilder::amend(first.clone())
                    .variant(v)
                    .consequence(format!(
                        "reinstall with `uv tool install {tool}` -- needs index access"
                    ))
                    .build();
            }
            units.extend(env);
        } else {
            units.push(
                NestedUnitBuilder::unknown_dir(
                    container,
                    child,
                    format!(
                        "`{}` has no pyvenv.cfg: not a tool environment",
                        relative_path(&container.path, &child.path)
                    ),
                )
                .build(),
            );
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{NestedActionCapability, TimeSource};
    use crate::build_adapters::{ContainerCache, FoldedIndex};
    use crate::fs_events::EventCoverage;
    use std::fs;

    /// Folded rows for every directory under `root`, one byte per level
    /// so totals differ, with the given newest-modification time.
    fn index_of(root: &Path, mtime: u64) -> FoldedIndex {
        fn walk(p: &Path, out: &mut Vec<FoldedDir>, mtime: u64) -> u64 {
            let mut total = 4096;
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if e.file_type().unwrap().is_dir() {
                        total += walk(&path, out, mtime);
                    } else {
                        total += e.metadata().unwrap().len().max(1);
                    }
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

    fn project(root: &Path, name: &str) -> BuildContainer {
        BuildContainer::project("python", root.join(name), root.to_path_buf())
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        fs::create_dir_all(build.join("lib")).unwrap();
        fs::create_dir_all(build.join("weird-plugin-output")).unwrap();
        let units = run(&project(tmp.path(), "build"), &index_of(&build, 500));
        let odd = units
            .iter()
            .find(|u| u.path == build.join("weird-plugin-output"))
            .expect("an unrecognised directory is still a row");
        assert!(!odd.coverage.supported);
        assert_eq!(odd.role, ArtifactRole::Residual);
        assert!(
            odd.coverage
                .limits
                .iter()
                .any(|l| l.contains("weird-plugin-output")),
            "{:?}",
            odd.coverage.limits
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let egg = tmp.path().join("pkg.egg-info");
        put(
            &egg.join("PKG-INFO"),
            &format!(
                "Name: pkg\nVersion: 1.0\n{}",
                "X".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES * 2)
            ),
        );
        let (units, counted) = crate::work_counters::measured(|| {
            run(&project(tmp.path(), "pkg.egg-info"), &index_of(&egg, 500))
        });
        assert!(
            counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64,
            "read {} bytes",
            counted.header_bytes_read
        );
        assert_eq!(
            units[0].variant.package, None,
            "a PKG-INFO that hit the cap is an unknown identity, never a prefix parse"
        );
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(&root.join("setup.py"), "import sys; sys.exit(7)\n");
        put(
            &root.join("pyproject.toml"),
            "[build-system]\nrequires=['x']\n",
        );
        let venv = root.join(".venv");
        put(
            &venv.join("pyvenv.cfg"),
            "home = /nowhere/bin\nversion = 3.11.4\n",
        );
        put(&venv.join("bin/python"), "#!/bin/sh\nexit 7\n");
        let (_units, counted) =
            crate::work_counters::measured(|| run(&project(root, ".venv"), &index_of(&venv, 500)));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "no interpreter is started, not even to ask its version"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // Two interpreters' setuptools trees and two mypy caches: a
        // name-only grouper reports one `lib` and one cache.
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        fs::create_dir_all(build.join("lib.macosx-11.0-arm64-cpython-311")).unwrap();
        fs::create_dir_all(build.join("lib.macosx-11.0-arm64-cpython-312")).unwrap();
        let units = run(&project(tmp.path(), "build"), &index_of(&build, 500));
        let libs: Vec<&NestedArtifact> = units
            .iter()
            .filter(|u| u.path.parent() == Some(build.as_path()))
            .collect();
        assert_eq!(libs.len(), 2);
        assert_ne!(libs[0].id, libs[1].id);
        let tools: Vec<Option<String>> = libs.iter().map(|u| u.variant.toolchain.clone()).collect();
        assert!(tools.contains(&Some("cpython-311".into())), "{tools:?}");
        assert!(tools.contains(&Some("cpython-312".into())), "{tools:?}");

        let mypy = tmp.path().join(".mypy_cache");
        fs::create_dir_all(mypy.join("3.11")).unwrap();
        fs::create_dir_all(mypy.join("3.12")).unwrap();
        let units = run(&project(tmp.path(), ".mypy_cache"), &index_of(&mypy, 500));
        let versions: Vec<Option<String>> = units
            .iter()
            .filter(|u| u.path != mypy)
            .map(|u| u.variant.toolchain.clone())
            .collect();
        assert_eq!(
            versions,
            vec![Some("python 3.11".into()), Some("python 3.12".into())]
        );
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("__pycache__");
        fs::create_dir_all(&cache).unwrap();
        let units = run(&project(tmp.path(), "__pycache__"), &index_of(&cache, 1));
        let u = &units[0];
        assert_eq!(u.time_source, TimeSource::FoldedDirectoryModification);
        assert_eq!(u.mtime_max, 1, "an ancient time is reported as what it is");
        assert_eq!(u.action, NestedActionCapability::InspectionOnly);
        assert!(u.consequence.as_deref().unwrap().contains("recompiles"));
    }

    #[test]
    fn a_wheel_and_an_sdist_are_named_by_their_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        put(
            &dist.join("demo_pkg-1.4.0-cp312-cp312-macosx_11_0_arm64.whl"),
            "w",
        );
        put(&dist.join("demo_pkg-1.4.0.tar.gz"), "s");
        put(&dist.join("notes.txt"), "n");
        let units = run(&project(tmp.path(), "dist"), &index_of(&dist, 500));
        let wheel = units
            .iter()
            .find(|u| u.relative_path.ends_with(".whl"))
            .unwrap();
        assert_eq!(wheel.variant.package.as_deref(), Some("demo_pkg"));
        assert_eq!(wheel.variant.version.as_deref(), Some("1.4.0"));
        assert_eq!(wheel.variant.toolchain.as_deref(), Some("cp312"));
        assert_eq!(
            wheel.variant.architecture.as_deref(),
            Some("macosx_11_0_arm64")
        );
        let sdist = units
            .iter()
            .find(|u| u.relative_path.ends_with(".tar.gz"))
            .unwrap();
        assert_eq!(sdist.variant.configuration.as_deref(), Some("sdist"));
        assert_ne!(
            wheel.id, sdist.id,
            "one version, two distributions, two units"
        );
        let notes = units
            .iter()
            .find(|u| u.relative_path.ends_with("notes.txt"))
            .unwrap();
        assert!(!notes.coverage.supported);
    }

    #[test]
    fn an_environment_names_its_interpreter_packages_and_local_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join(".venv");
        let python_home = tmp.path().join("python/bin");
        fs::create_dir_all(&python_home).unwrap();
        put(
            &venv.join("pyvenv.cfg"),
            &format!(
                "home = {}\nimplementation = CPython\nversion_info = 3.12.4.final.0\nuv = 0.4.18\n",
                python_home.display()
            ),
        );
        let site = venv.join("lib/python3.12/site-packages");
        put(&site.join("yaml/__init__.py"), "x");
        put(
            &site.join("PyYAML-6.0.1.dist-info/top_level.txt"),
            "_yaml\nyaml\n",
        );
        put(&site.join("mylib/__init__.py"), "x");
        put(
            &site.join("mylib-0.1.0.dist-info/direct_url.json"),
            r#"{"url": "file:///src/mylib", "dir_info": {"editable": true}}"#,
        );
        put(&site.join("mylib-0.1.0.dist-info/top_level.txt"), "mylib\n");
        put(&site.join("orphan/__init__.py"), "x");
        let units = run(&project(tmp.path(), ".venv"), &index_of(&venv, 500));
        let root = units.iter().find(|u| u.path == venv).unwrap();
        assert_eq!(
            root.variant.toolchain.as_deref(),
            Some("cpython 3.12.4.final.0")
        );
        assert!(
            root.coverage
                .limits
                .iter()
                .any(|l| l.contains("uv links packages")),
            "a uv environment says its bytes may be shared with the cache: {:?}",
            root.coverage.limits
        );
        let yaml = units.iter().find(|u| u.path == site.join("yaml")).unwrap();
        assert_eq!(
            yaml.variant.package.as_deref(),
            Some("PyYAML"),
            "the directory is named by the distribution whose top_level.txt lists it"
        );
        let mylib = units.iter().find(|u| u.path == site.join("mylib")).unwrap();
        assert!(
            mylib.consequence.as_deref().unwrap().contains("/src/mylib"),
            "an editable install's cost is its local source, not the index: {:?}",
            mylib.consequence
        );
        let orphan = units
            .iter()
            .find(|u| u.path == site.join("orphan"))
            .unwrap();
        assert_eq!(
            orphan.variant.package, None,
            "no distribution claims it, and its name is not a guess"
        );
    }

    #[test]
    fn a_missing_interpreter_and_incomplete_metadata_are_said_not_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join("venv");
        put(
            &venv.join("pyvenv.cfg"),
            "home = /definitely/not/here/bin\n",
        );
        let units = run(&project(tmp.path(), "venv"), &index_of(&venv, 500));
        let root = &units[0];
        assert!(
            root.coverage
                .limits
                .iter()
                .any(|l| l.contains("is not at `/definitely/not/here/bin`")),
            "{:?}",
            root.coverage.limits
        );
        assert!(
            root.coverage
                .limits
                .iter()
                .any(|l| l.contains("names no interpreter version"))
        );
        assert!(root.variant.unknowns.iter().any(|x| x == "toolchain"));
        assert!(
            root.coverage.supported,
            "an environment with gaps is still an environment"
        );
    }

    #[test]
    fn uv_cache_buckets_and_local_source_builds_are_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("uv");
        fs::create_dir_all(cache.join("archive-v0/abc123")).unwrap();
        fs::create_dir_all(cache.join("wheels-v5/pypi/requests")).unwrap();
        fs::create_dir_all(cache.join("sdists-v9/path/9f8e7d")).unwrap();
        fs::create_dir_all(cache.join("simple-v16/pypi")).unwrap();
        fs::create_dir_all(cache.join("mystery")).unwrap();
        let c = BuildContainer::shared_store_of("python", cache.clone(), BuildStoreKind::UvCache);
        let units = run(&c, &index_of(&cache, 500));
        let archive = units
            .iter()
            .find(|u| u.path == cache.join("archive-v0"))
            .unwrap();
        assert_eq!(
            archive.variant.configuration.as_deref(),
            Some("archive layout 0")
        );
        let req = units
            .iter()
            .find(|u| u.path == cache.join("wheels-v5/pypi/requests"))
            .unwrap();
        assert_eq!(req.variant.package.as_deref(), Some("requests"));
        let local = units
            .iter()
            .find(|u| u.path == cache.join("sdists-v9/path"))
            .unwrap();
        assert!(
            local
                .consequence
                .as_deref()
                .unwrap()
                .contains("local source")
        );
        let mystery = units
            .iter()
            .find(|u| u.path == cache.join("mystery"))
            .unwrap();
        assert!(!mystery.coverage.supported);
        assert!(
            units
                .iter()
                .filter(|u| u.path != cache && u.coverage.supported)
                .all(|u| matches!(u.action, NestedActionCapability::Unsupported { .. })),
            "every identified cache entry withholds actions: other environments link to it"
        );
    }

    #[test]
    fn uv_managed_interpreters_are_installations_by_version() {
        let tmp = tempfile::tempdir().unwrap();
        let py = tmp.path().join("python");
        fs::create_dir_all(py.join("cpython-3.12.4-macos-aarch64-none")).unwrap();
        fs::create_dir_all(py.join("cpython-3.11.9-macos-aarch64-none")).unwrap();
        let c = BuildContainer::shared_store_of(
            "python",
            py.clone(),
            BuildStoreKind::UvPythonInstallations,
        );
        let units = run(&c, &index_of(&py, 500));
        let installs: Vec<&NestedArtifact> = units.iter().filter(|u| u.path != py).collect();
        assert_eq!(installs.len(), 2);
        assert!(
            installs
                .iter()
                .all(|u| u.role == ArtifactRole::Installation)
        );
        assert!(
            installs
                .iter()
                .all(|u| u.role.family() != crate::artifact::RoleFamily::Outputs),
            "an interpreter is never build output"
        );
        let versions: Vec<Option<String>> =
            installs.iter().map(|u| u.variant.version.clone()).collect();
        assert!(
            versions.contains(&Some("3.11.9".into())) && versions.contains(&Some("3.12.4".into()))
        );
    }

    #[test]
    fn distribution_names_parse_only_in_the_specified_shapes() {
        assert_eq!(
            parse_distribution("pkg-1.0-py3-none-any.whl").map(|d| d.version),
            Some("1.0".into())
        );
        assert_eq!(
            parse_distribution("pkg-1.0-1-py3-none-any.whl").map(|d| d.python),
            Some(Some("py3".into())),
            "an optional build tag keeps the python/abi/platform positions"
        );
        assert_eq!(parse_distribution("pkg.whl"), None);
        assert_eq!(parse_distribution("pkg-latest.tar.gz"), None);
    }
}
