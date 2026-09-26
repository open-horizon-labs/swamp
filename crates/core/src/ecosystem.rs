//! Which ecosystems a checkout belongs to, from the marker files at its
//! root, and which artifact directories each ecosystem generates — the
//! per-language detection clean-dev-dirs and kondo run per project, kept
//! here as facts: a project row wears its tags/glyphs and can be filtered
//! `type:rust`; an artifact row knows which ecosystem produced it; and an
//! ambiguous directory name (`build`, `dist`, `vendor`, `bin`) counts as
//! an artifact only next to the marker of an ecosystem that generates it.
//! A checkout can be several ecosystems at once (a Tauri app is Rust and
//! Node); every match is kept.

use crate::report::ArtifactKind;
use serde_json::Value;
use std::path::Path;

pub struct Ecosystem {
    /// Short tag shown in brackets: `rs`, `js`, `py`, …
    pub tag: &'static str,
    /// Human name for help text and JSON output.
    pub name: &'static str,
    /// Root markers. `*.ext` matches by extension; anything else is an
    /// exact file or directory name.
    pub markers: &'static [&'static str],
    /// Directories this ecosystem generates inside a project, and what
    /// kind each is. `*.suffix` entries match by suffix (`*.egg-info`).
    pub cleans: &'static [(&'static str, ArtifactKind)],
    /// One glyph for the TUI badge column. Emoji here all have default
    /// emoji presentation (display width 2); the two text glyphs (λ, ⬢)
    /// are width 1. Padding is by display width, never by char count.
    pub glyph: &'static str,
    /// Where the project's own name lives, for a checkout with no remote:
    /// (file, how to read it).
    pub name_source: Option<(&'static str, NameField)>,
}

/// How to pull a project name out of a manifest without executing it.
#[derive(Clone, Copy)]
pub enum NameField {
    /// `name = "x"` in a TOML `[package]`/`[project]`/`[tool.poetry]` table.
    TomlName,
    /// `"name": "x"` in JSON.
    JsonName,
    /// `module x` in go.mod.
    GoModule,
    /// `<artifactId>x</artifactId>` in pom.xml.
    PomArtifactId,
    /// `name: x` in YAML (pubspec, package.yaml).
    YamlName,
    /// `name := "x"` in build.sbt.
    SbtName,
    /// `name: "x"` in Package.swift.
    SwiftName,
    /// `app: :x` in mix.exs.
    MixApp,
    /// `project(x ...)` in CMakeLists.txt.
    CmakeProject,
    /// `rootProject.name = "x"` in settings.gradle or settings.gradle.kts.
    GradleRootProject,
    /// `name: x` in a Cabal package description.
    CabalName,
    /// `name = x` in the `[metadata]` section of setup.cfg.
    SetupCfgMetadataName,
    /// The file's own stem (`Foo.csproj` → `Foo`).
    FileStem,
}

use ArtifactKind::{BuildOutput as Build, Cache, DependencyTree as Deps};

pub const ECOSYSTEMS: &[Ecosystem] = &[
    Ecosystem {
        tag: "rs",
        name: "Rust",
        markers: &["Cargo.toml"],
        cleans: &[("target", Build)],
        glyph: "🦀",
        name_source: Some(("Cargo.toml", NameField::TomlName)),
    },
    Ecosystem {
        tag: "js",
        name: "Node.js",
        markers: &["package.json"],
        cleans: &[
            ("node_modules", Deps),
            ("dist", Build),
            ("build", Build),
            ("out", Build),
            (".next", Build),
            (".nuxt", Build),
            (".svelte-kit", Build),
            (".output", Build),
            (".turbo", Cache),
            (".parcel-cache", Cache),
            (".angular", Cache),
            (".expo", Cache),
            (".metro", Cache),
            ("coverage", Build),
            // github/gitignore Node, Angular, Firebase, Nextjs, bun.
            (".vite", Cache),
            (".fusebox", Cache),
            (".rpt2_cache", Cache),
            (".rts2_cache_cjs", Cache),
            (".rts2_cache_es", Cache),
            (".rts2_cache_umd", Cache),
            (".serverless", Build),
            (".dynamodb", Cache),
            (".firebase", Cache),
            (".ng", Cache),
            ("out-tsc", Build),
            ("bower_components", Deps),
            ("jspm_packages", Deps),
            ("web_modules", Deps),
            ("typings", Deps),
        ],
        glyph: "⬢",
        name_source: Some(("package.json", NameField::JsonName)),
    },
    Ecosystem {
        tag: "deno",
        name: "Deno",
        markers: &["deno.json", "deno.jsonc"],
        cleans: &[
            ("vendor", Deps),
            ("node_modules", Deps),
            // github/gitignore Deno.
            (".deno", Cache),
        ],
        glyph: "🦕",
        name_source: Some(("deno.json", NameField::JsonName)),
    },
    Ecosystem {
        tag: "py",
        name: "Python",
        markers: &[
            "pyproject.toml",
            // `requirements.txt`, `requirements-dev.txt`, `requirements/`:
            // a repo whose only marker was `requirements-dev.txt` kept a
            // 255 MB `build/` in its Source total.
            "requirements*",
            "setup.py",
            "setup.cfg",
            "Pipfile",
            "poetry.lock",
            "environment.yml",
        ],
        cleans: &[
            ("__pycache__", Build),
            (".pytest_cache", Cache),
            (".mypy_cache", Cache),
            (".ruff_cache", Cache),
            ("venv", Deps),
            (".venv", Deps),
            ("build", Build),
            ("dist", Build),
            (".eggs", Build),
            ("*.egg-info", Build),
            (".tox", Cache),
            (".nox", Cache),
            (".coverage", Cache),
            ("__pypackages__", Deps),
            (".pixi", Deps),
            // github/gitignore Python. Service data it also lists (var, instance, lib, mnesia, rabbitmq) is authored or live state and is deliberately absent.
            (".hypothesis", Cache),
            (".pytype", Cache),
            (".pyre", Cache),
            (".pdm-build", Build),
            (".pybuilder", Build),
            ("cython_debug", Build),
            ("develop-eggs", Build),
            ("eggs", Build),
            ("sdist", Build),
            ("wheels", Build),
            ("downloads", Build),
            ("parts", Build),
            ("htmlcov", Build),
            ("cover", Build),
            ("profile_default", Cache),
            ("__marimo__", Cache),
            (".abstra", Cache),
            ("env.bak", Deps),
            ("venv.bak", Deps),
        ],
        glyph: "🐍",
        name_source: Some(("pyproject.toml", NameField::TomlName)),
    },
    Ecosystem {
        tag: "go",
        name: "Go",
        markers: &["go.mod"],
        // `bin/` beside go.mod is where `go build -o bin/` writes; the Go
        // build adapter identifies each binary in it (#69).
        cleans: &[("vendor", Deps), ("bin", Build)],
        glyph: "🐹",
        name_source: Some(("go.mod", NameField::GoModule)),
    },
    Ecosystem {
        tag: "java",
        name: "JVM",
        markers: &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        cleans: &[
            ("target", Build),
            ("build", Build),
            (".gradle", Cache),
            // github/gitignore Java, Kotlin, Gradle, Android.
            (".kotlin", Cache),
            (".mtj.tmp", Cache),
            (".cxx", Build),
            (".externalNativeBuild", Build),
            ("captures", Build),
        ],
        glyph: "☕",
        name_source: Some(("pom.xml", NameField::PomArtifactId)),
    },
    Ecosystem {
        tag: "scala",
        name: "Scala",
        markers: &["build.sbt"],
        cleans: &[("target", Build)],
        glyph: "🔺",
        name_source: Some(("build.sbt", NameField::SbtName)),
    },
    Ecosystem {
        tag: "cpp",
        name: "C/C++",
        markers: &["CMakeLists.txt", "meson.build", "Makefile"],
        cleans: &[
            ("build", Build),
            // ESP-IDF and other CMake projects build one directory per
            // target or variant (`build-dial`, `build-stackchan`) and
            // vendor their dependencies into `managed_components`. Both
            // sat in the Source total until now: on one ESP-IDF repo
            // that was 2.1 GB of "source".
            ("build-*", Build),
            ("cmake-build-*", Build),
            ("cmake-build-debug", Build),
            ("cmake-build-release", Build),
            ("managed_components", Deps),
            // github/gitignore C, C++, CMake, Autotools.
            ("CMakeFiles", Build),
            ("Testing", Build),
            (".deps", Build),
            (".libs", Build),
            (".tmp_versions", Build),
            ("vcpkg_installed", Deps),
        ],
        glyph: "🔧",
        name_source: Some(("CMakeLists.txt", NameField::CmakeProject)),
    },
    Ecosystem {
        tag: "idf",
        name: "ESP-IDF",
        // An ESP-IDF component or app directory need not have its own
        // `CMakeLists.txt`; `sdkconfig` and the component manifest are
        // what identify it.
        markers: &[
            "sdkconfig",
            "sdkconfig.defaults",
            "idf_component.yml",
            "partitions.csv",
        ],
        cleans: &[
            ("build", Build),
            ("build-*", Build),
            ("managed_components", Deps),
        ],
        glyph: "📟",
        name_source: None,
    },
    Ecosystem {
        tag: "swift",
        name: "Swift",
        markers: &["Package.swift", "*.xcodeproj", "*.xcworkspace", "Podfile"],
        cleans: &[
            (".build", Build),
            (".swiftpm", Cache),
            ("DerivedData", Build),
            ("Pods", Deps),
            // github/gitignore Swift, Objective-C.
            ("xcuserdata", Cache),
        ],
        glyph: "🐦",
        name_source: Some(("Package.swift", NameField::SwiftName)),
    },
    Ecosystem {
        tag: "net",
        name: ".NET",
        markers: &["*.csproj", "*.fsproj", "*.vbproj", "*.sln"],
        cleans: &[
            ("bin", Build),
            ("obj", Build),
            // github/gitignore Dotnet, VisualStudio. Its case-insensitive globs ([Bb]in, [Oo]bj) are the same directories our marker-gated bin/obj already cover; its *Backup names are data.
            (".vs", Cache),
            ("ipch", Cache),
            ("bld", Build),
            ("AppPackages", Build),
            ("ClientBin", Build),
            ("BundleArtifacts", Build),
            ("BenchmarkDotNet.Artifacts", Build),
            ("CodeCoverage", Build),
            ("FakesAssemblies", Build),
            ("MSBuild_Logs", Build),
            ("OpenCover", Build),
            ("_UpgradeReport_Files", Build),
            ("paket-files", Deps),
        ],
        glyph: "🟣",
        name_source: Some(("*.csproj", NameField::FileStem)),
    },
    Ecosystem {
        tag: "rb",
        name: "Ruby",
        markers: &["Gemfile"],
        cleans: &[
            (".bundle", Deps),
            // github/gitignore Ruby, Rails.
            (".yardoc", Build),
            ("_yardoc", Build),
            ("rdoc", Build),
            ("pkg", Build),
        ],
        glyph: "💎",
        name_source: Some(("*.gemspec", NameField::FileStem)),
    },
    Ecosystem {
        tag: "ex",
        name: "Elixir",
        markers: &["mix.exs"],
        cleans: &[
            ("_build", Build),
            ("deps", Deps),
            (".elixir-tools", Cache),
            (".elixir_ls", Cache),
            (".lexical", Cache),
        ],
        glyph: "💧",
        name_source: Some(("mix.exs", NameField::MixApp)),
    },
    Ecosystem {
        tag: "php",
        name: "PHP",
        markers: &["composer.json"],
        cleans: &[("vendor", Deps)],
        glyph: "🐘",
        name_source: Some(("composer.json", NameField::JsonName)),
    },
    Ecosystem {
        tag: "hs",
        name: "Haskell",
        markers: &["stack.yaml", "*.cabal", "cabal.project", "package.yaml"],
        cleans: &[
            (".stack-work", Build),
            ("dist-newstyle", Build),
            // github/gitignore Haskell.
            (".cabal-sandbox", Deps),
            (".HTF", Build),
        ],
        glyph: "λ",
        name_source: Some(("package.yaml", NameField::YamlName)),
    },
    Ecosystem {
        tag: "dart",
        name: "Dart/Flutter",
        markers: &["pubspec.yaml"],
        cleans: &[
            (".dart_tool", Cache),
            ("build", Build),
            // github/gitignore Dart, Flutter.
            (".pub", Cache),
            (".pub-preload-cache", Cache),
            (".buildlog", Cache),
        ],
        glyph: "🎯",
        name_source: Some(("pubspec.yaml", NameField::YamlName)),
    },
    Ecosystem {
        tag: "zig",
        name: "Zig",
        markers: &["build.zig"],
        cleans: &[
            ("zig-cache", Cache),
            (".zig-cache", Cache),
            ("zig-out", Build),
        ],
        glyph: "⚡",
        name_source: None,
    },
    Ecosystem {
        tag: "tf",
        name: "Terraform",
        markers: &["*.tf"],
        cleans: &[(".terraform", Deps)],
        glyph: "🌍",
        name_source: None,
    },
    Ecosystem {
        tag: "godot",
        name: "Godot",
        markers: &["project.godot"],
        cleans: &[(".godot", Cache), (".import", Cache), (".mono", Build)],
        glyph: "🤖",
        name_source: None,
    },
    Ecosystem {
        tag: "jekyll",
        name: "Jekyll",
        markers: &["_config.yml", "_config.toml"],
        cleans: &[
            ("_site", Build),
            (".jekyll-cache", Cache),
            (".jekyll-metadata", Cache),
            (".sass-cache", Cache),
        ],
        glyph: "📄",
        name_source: None,
    },
    Ecosystem {
        tag: "elm",
        name: "Elm",
        markers: &["elm.json"],
        cleans: &[("elm-stuff", Deps)],
        glyph: "🌳",
        name_source: None,
    },
    Ecosystem {
        tag: "erl",
        name: "Erlang",
        markers: &["rebar.config", "erlang.mk"],
        cleans: &[("_build", Build), (".rebar3", Cache), ("_checkouts", Deps)],
        glyph: "☎️",
        name_source: None,
    },
    Ecosystem {
        tag: "ml",
        name: "OCaml",
        markers: &["dune-project", "*.opam"],
        cleans: &[("_build", Build), ("_opam", Deps)],
        glyph: "🐫",
        name_source: None,
    },
    Ecosystem {
        tag: "clj",
        name: "Clojure",
        markers: &["deps.edn", "project.clj", "shadow-cljs.edn"],
        cleans: &[
            (".cpcache", Cache),
            (".lein-plugins", Cache),
            (".shadow-cljs", Cache),
            ("classes", Build),
        ],
        glyph: "🔮",
        name_source: None,
    },
    Ecosystem {
        tag: "nim",
        name: "Nim",
        markers: &["*.nimble", "nim.cfg"],
        cleans: &[
            ("nimcache", Cache),
            ("nimblecache", Cache),
            ("htmldocs", Build),
        ],
        glyph: "👑",
        name_source: None,
    },
    Ecosystem {
        tag: "docker",
        name: "Docker",
        markers: &[
            "Dockerfile",
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
        ],
        cleans: &[],
        glyph: "🐳",
        name_source: None,
    },
    Ecosystem {
        tag: "unity",
        name: "Unity",
        markers: &["ProjectSettings"],
        cleans: &[
            ("Library", Cache),
            ("Temp", Cache),
            ("Obj", Build),
            ("Logs", Cache),
            ("MemoryCaptures", Cache),
            ("Build", Build),
            ("Builds", Build),
            // github/gitignore Unity.
            ("ExportedObj", Build),
            ("UIElementsSchema", Build),
            (".consulo", Cache),
            (".utmp", Cache),
        ],
        glyph: "🎲",
        name_source: None,
    },
    Ecosystem {
        tag: "ue",
        name: "Unreal",
        markers: &["*.uproject"],
        cleans: &[
            ("Binaries", Build),
            ("Intermediate", Build),
            ("Saved", Cache),
            ("DerivedDataCache", Cache),
            ("Build", Build),
            // github/gitignore UnrealEngine.
            (".vs", Cache),
        ],
        glyph: "🎮",
        name_source: Some(("*.uproject", NameField::FileStem)),
    },
];

pub fn dir_names(root: &Path) -> Vec<String> {
    crate::fs_gate::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `*.ext` matches by extension, `prefix*` by prefix, anything else is
/// an exact file or directory name.
fn marker_present(names: &[String], marker: &str) -> bool {
    if let Some(ext) = marker.strip_prefix("*.") {
        names.iter().any(|n| n.ends_with(&format!(".{ext}")))
    } else if let Some(prefix) = marker.strip_suffix('*') {
        names.iter().any(|n| n.starts_with(prefix))
    } else {
        names.iter().any(|n| n == marker)
    }
}

fn detect_in(names: &[String]) -> Vec<&'static Ecosystem> {
    ECOSYSTEMS
        .iter()
        .filter(|e| e.markers.iter().any(|m| marker_present(names, m)))
        .collect()
}

/// Ecosystem tags present at `root`, in table order. Empty when none.
pub fn detect(root: &Path) -> Vec<String> {
    detect_in(&dir_names(root))
        .into_iter()
        .map(|e| e.tag.to_string())
        .collect()
}

/// `[rs][js]` for a project row; empty string when nothing was detected.
pub fn tags(ecosystems: &[String]) -> String {
    ecosystems.iter().map(|t| format!("[{t}]")).collect()
}

pub fn by_tag(tag: &str) -> Option<&'static Ecosystem> {
    ECOSYSTEMS.iter().find(|e| e.tag == tag)
}

/// Human name for a tag, for help text and JSON output.
pub fn name_for(tag: &str) -> Option<&'static str> {
    by_tag(tag).map(|e| e.name)
}

/// The badge glyph for a tag; `?` for a tag the table does not know.
pub fn glyph_for(tag: &str) -> &'static str {
    by_tag(tag).map(|e| e.glyph).unwrap_or("?")
}

/// `*suffix` matches by suffix, `prefix*` by prefix, anything else is an
/// exact name. Both wildcard forms require at least one character where
/// the `*` is, so `build-*` never matches a bare `build-`.
fn cleans_name(pattern: &str, name: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix) && name.len() > suffix.len()
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix) && name.len() > prefix.len()
    } else {
        pattern == name
    }
}

/// The ecosystem, among `tags` (a project's detected ecosystems), that
/// generates a directory called `name`. When none of the project's own
/// ecosystems claims it, the name alone decides only if exactly one
/// ecosystem in the table generates it (`target` is Rust, JVM and Scala,
/// so a bare `target` stays unattributed; `.stack-work` is Haskell's
/// alone). `None` for a name no ecosystem claims (`.cache`, `.git`) or an
/// ambiguous one nobody at the root vouches for.
pub fn artifact_ecosystem(tags: &[String], name: &str) -> Option<&'static str> {
    if let Some(e) = tags
        .iter()
        .filter_map(|t| by_tag(t))
        .find(|e| e.cleans.iter().any(|(p, _)| cleans_name(p, name)))
    {
        return Some(e.tag);
    }
    let mut claimers = ECOSYSTEMS
        .iter()
        .filter(|e| e.cleans.iter().any(|(p, _)| cleans_name(p, name)));
    match (claimers.next(), claimers.next()) {
        (Some(only), None) => Some(only.tag),
        _ => None,
    }
}

/// [`artifact_ecosystem`] with the artifact's parent directory available:
/// the markers sitting next to the artifact are the strongest evidence
/// (a nested CMake project's `build/` is C/C++ even when the checkout
/// root is a Node monorepo), then the project's own tags, then a name
/// only one ecosystem generates.
pub fn artifact_ecosystem_at(parent: &Path, tags: &[String], name: &str) -> Option<&'static str> {
    let names = dir_names(parent);
    artifact_ecosystem_among(parent, &names, tags, name)
}

/// [`artifact_ecosystem_at`] with the parent's entry names already
/// listed -- so a caller annotating many artifacts under one parent
/// lists it once (R19: the growth consumer annotated every artifact row,
/// each with its own `read_dir` of the parent, and a `target/` with
/// thousands of identified interiors listed `target/debug/deps` thousands
/// of times per pass -- 138 s of an unchanged `observe`).
pub fn artifact_ecosystem_among(
    parent: &Path,
    names: &[String],
    tags: &[String],
    name: &str,
) -> Option<&'static str> {
    if ruby_vendor_bundle(parent, name) {
        return Some("rb");
    }
    if let Some(e) = detect_in(names)
        .into_iter()
        .find(|e| e.cleans.iter().any(|(p, _)| cleans_name(p, name)))
    {
        return Some(e.tag);
    }
    artifact_ecosystem(tags, name)
}

/// Bumped whenever the classification tables above change what counts as
/// an artifact. The store records the version it was walked with; a
/// mismatch forces one full walk so rows that no longer classify leave
/// and rows that now do arrive, instead of lingering until something
/// happens to touch their directory.
pub const RULES_VERSION: u32 = 5;

/// Marker-gated classification: `name` inside `parent` is an artifact of
/// the kind an ecosystem declares, if that ecosystem's marker sits in
/// `parent`. This is what lets `build/`, `dist/`, `vendor/`, `bin/` and
/// `obj/` count only where the project type that produces them lives.
pub fn classify_gated(parent: &Path, name: &str) -> Option<ArtifactKind> {
    if ruby_vendor_bundle(parent, name) {
        return Some(Deps);
    }
    let names = dir_names(parent);
    detect_in(&names).into_iter().find_map(|e| {
        e.cleans
            .iter()
            .find(|(p, _)| cleans_name(p, name))
            .map(|(_, k)| k.clone())
    })
}

/// The project's own name from its manifest, for a checkout without a
/// remote to name it: first ecosystem in table order whose manifest is
/// present and yields a name.
pub fn manifest_name(root: &Path) -> Option<String> {
    let names = dir_names(root);
    for e in detect_in(&names) {
        for (file, field) in name_sources(e) {
            let Some(file_name) = resolve_manifest_file(&names, file) else {
                continue;
            };
            if let NameField::FileStem = field {
                if let Some(stem) = Path::new(&file_name).file_stem() {
                    return Some(stem.to_string_lossy().into_owned());
                }
                continue;
            }
            let Ok(text) = crate::fs_gate::read::bounded_string(
                root.join(&file_name),
                crate::fs_gate::read::BoundedCap::MANIFEST,
            ) else {
                continue;
            };
            if let Some(n) = extract_name(&text, field) {
                return Some(n);
            }
        }
    }
    None
}

fn resolve_manifest_file(names: &[String], file: &str) -> Option<String> {
    match file.strip_prefix("*.") {
        Some(ext) => names
            .iter()
            .find(|n| n.ends_with(&format!(".{ext}")))
            .cloned(),
        None => names.iter().find(|n| n.as_str() == file).cloned(),
    }
}

/// Existing manifest readers remain first in table order; these are only
/// declarative fallbacks when the ecosystem's primary manifest is absent or
/// does not yield a name.
fn name_sources(e: &Ecosystem) -> Vec<(&'static str, NameField)> {
    let mut sources = Vec::new();
    if let Some(source) = e.name_source {
        sources.push(source);
    }
    match e.tag {
        "java" => sources.extend([
            ("settings.gradle", NameField::GradleRootProject),
            ("settings.gradle.kts", NameField::GradleRootProject),
        ]),
        "hs" => sources.push(("*.cabal", NameField::CabalName)),
        "py" => sources.push(("setup.cfg", NameField::SetupCfgMetadataName)),
        _ => {}
    }
    sources
}

fn quoted(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(',').trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|r| r.split('"').next())
        .or_else(|| s.strip_prefix('\'').and_then(|r| r.split('\'').next()))?;
    (!inner.is_empty()).then(|| inner.to_string())
}

fn extract_name(text: &str, field: NameField) -> Option<String> {
    match field {
        NameField::TomlName => {
            // The first `name = "..."` after a [package]/[project]/[tool.poetry]
            // header, so a dependency table's `name` never wins.
            let mut in_table = false;
            for line in text.lines() {
                let l = line.trim();
                if l.starts_with('[') {
                    in_table = matches!(
                        l,
                        "[package]" | "[project]" | "[tool.poetry]" | "[metadata]"
                    );
                }
                if in_table && let Some(rest) = l.strip_prefix("name") {
                    let rest = rest.trim_start();
                    if let Some(v) = rest.strip_prefix('=') {
                        return quoted(v);
                    }
                }
            }
            None
        }
        NameField::JsonName => serde_json::from_str::<Value>(text)
            .ok()?
            .as_object()?
            .get("name")?
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        NameField::GoModule => text.lines().find_map(|l| {
            let m = l.trim().strip_prefix("module ")?.trim();
            m.rsplit('/').next().map(str::to_string)
        }),
        NameField::PomArtifactId => text.lines().find_map(|l| {
            let l = l.trim();
            let inner = l
                .strip_prefix("<artifactId>")?
                .strip_suffix("</artifactId>")?;
            (!inner.is_empty()).then(|| inner.to_string())
        }),
        NameField::YamlName => text.lines().find_map(|l| {
            let rest = l.strip_prefix("name:")?.trim();
            let v = quoted(rest).unwrap_or_else(|| rest.to_string());
            (!v.is_empty()).then_some(v)
        }),
        NameField::SbtName => text.lines().find_map(|l| {
            let rest = l
                .trim()
                .strip_prefix("name")?
                .trim_start()
                .strip_prefix(":=")?;
            quoted(rest)
        }),
        NameField::SwiftName => text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("name:")?;
            quoted(rest)
        }),
        NameField::MixApp => text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("app:")?.trim().strip_prefix(':')?;
            let v: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!v.is_empty()).then_some(v)
        }),
        NameField::CmakeProject => text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("project(")?;
            let v: String = rest
                .trim_start()
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ')')
                .collect();
            (!v.is_empty()).then_some(v)
        }),
        NameField::GradleRootProject => text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("rootProject")?.trim_start();
            let rest = rest.strip_prefix(".name")?.trim_start();
            let rest = rest.strip_prefix('=')?;
            quoted(rest)
        }),
        NameField::CabalName => text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("name:")?.trim();
            (!rest.is_empty()).then(|| rest.to_string())
        }),
        NameField::SetupCfgMetadataName => {
            let mut in_metadata = false;
            text.lines().find_map(|l| {
                let l = l.trim();
                if l.starts_with('[') && l.ends_with(']') {
                    in_metadata = l[1..l.len() - 1].trim().eq_ignore_ascii_case("metadata");
                    return None;
                }
                if !in_metadata {
                    return None;
                }
                let (key, value) = l.split_once('=')?;
                (key.trim().eq_ignore_ascii_case("name"))
                    .then(|| value.trim().to_string())
                    .filter(|name| !name.is_empty())
            })
        }
        NameField::FileStem => None,
    }
}

fn ruby_vendor_bundle(parent: &Path, name: &str) -> bool {
    name == "bundle"
        && parent.file_name().and_then(|n| n.to_str()) == Some("vendor")
        && parent
            .parent()
            .map(|root| marker_present(&dir_names(root), "Gemfile"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_several_ecosystems_at_one_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("App.csproj"), "").unwrap();
        std::fs::write(tmp.path().join("main.tf"), "").unwrap();
        assert_eq!(detect(tmp.path()), vec!["rs", "js", "net", "tf"]);
        assert_eq!(tags(&detect(tmp.path())), "[rs][js][net][tf]");
        assert_eq!(name_for("hs"), Some("Haskell"));
        assert_eq!(glyph_for("rs"), "🦀");
        let empty = tempfile::tempdir().unwrap();
        assert!(detect(empty.path()).is_empty());
    }

    #[test]
    fn ambiguous_names_classify_only_next_to_their_marker() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(classify_gated(tmp.path(), "build"), None);
        assert_eq!(classify_gated(tmp.path(), "vendor"), None);
        std::fs::write(tmp.path().join("CMakeLists.txt"), "project(x)").unwrap();
        assert_eq!(classify_gated(tmp.path(), "build"), Some(Build));
        // ESP-IDF's per-variant build directories and vendored components.
        assert_eq!(classify_gated(tmp.path(), "build-dial"), Some(Build));
        assert_eq!(classify_gated(tmp.path(), "cmake-build-debug"), Some(Build));
        assert_eq!(classify_gated(tmp.path(), "managed_components"), Some(Deps));
        // ESP-IDF identifies itself by sdkconfig, with no CMakeLists.txt.
        let idf = tempfile::tempdir().unwrap();
        std::fs::write(idf.path().join("sdkconfig"), "").unwrap();
        assert_eq!(classify_gated(idf.path(), "build"), Some(Build));
        assert_eq!(classify_gated(idf.path(), "managed_components"), Some(Deps));
        // A Python project whose only marker is requirements-dev.txt.
        let py = tempfile::tempdir().unwrap();
        std::fs::write(py.path().join("requirements-dev.txt"), "").unwrap();
        assert_eq!(classify_gated(py.path(), "build"), Some(Build));
        assert_eq!(
            classify_gated(tmp.path(), "builder"),
            None,
            "prefix needs the dash"
        );
        assert_eq!(
            classify_gated(tmp.path(), "build-"),
            None,
            "a bare prefix is not a variant"
        );
        assert_eq!(classify_gated(tmp.path(), "vendor"), None);
        std::fs::write(tmp.path().join("go.mod"), "module a/b").unwrap();
        assert_eq!(classify_gated(tmp.path(), "vendor"), Some(Deps));
        std::fs::write(tmp.path().join("setup.py"), "").unwrap();
        assert_eq!(classify_gated(tmp.path(), "foo.egg-info"), Some(Build));
        assert_eq!(classify_gated(tmp.path(), ".egg-info"), None);
    }

    #[test]
    fn artifact_ecosystem_prefers_the_projects_own_tags() {
        let both = vec!["py".to_string(), "js".to_string()];
        assert_eq!(artifact_ecosystem(&both, "dist"), Some("py"));
        assert_eq!(artifact_ecosystem(&[], ".stack-work"), Some("hs"));
        assert_eq!(
            artifact_ecosystem(&[], "target"),
            None,
            "rs/java/scala all claim it"
        );
        assert_eq!(
            artifact_ecosystem(&[], "node_modules"),
            None,
            "js and deno claim it"
        );
        assert_eq!(artifact_ecosystem(&["rs".into()], "target"), Some("rs"));
        assert_eq!(artifact_ecosystem(&[], ".cache"), None);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CMakeLists.txt"), "").unwrap();
        assert_eq!(
            artifact_ecosystem_at(tmp.path(), &["js".into()], "build"),
            Some("cpp"),
            "the marker next to the artifact outranks the root's tags"
        );
    }

    #[test]
    fn manifest_names_from_common_formats() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"crabby\"\n[dependencies]\nname = \"no\"\n",
        )
        .unwrap();
        assert_eq!(manifest_name(tmp.path()).as_deref(), Some("crabby"));
        let js = tempfile::tempdir().unwrap();
        std::fs::write(
            js.path().join("package.json"),
            "{\"name\":\"@org/pkg\",\"nested\":{\"name\":\"wrong\"}}",
        )
        .unwrap();
        assert_eq!(manifest_name(js.path()).as_deref(), Some("@org/pkg"));
        let malformed = tempfile::tempdir().unwrap();
        std::fs::write(
            malformed.path().join("package.json"),
            "{\"metadata\":{\"name\":\"nested-only\"}",
        )
        .unwrap();
        assert_eq!(manifest_name(malformed.path()), None);
        let go = tempfile::tempdir().unwrap();
        std::fs::write(go.path().join("go.mod"), "module github.com/a/gopher\n").unwrap();
        assert_eq!(manifest_name(go.path()).as_deref(), Some("gopher"));
        let net = tempfile::tempdir().unwrap();
        std::fs::write(net.path().join("Shop.csproj"), "").unwrap();
        assert_eq!(manifest_name(net.path()).as_deref(), Some("Shop"));
        let ex = tempfile::tempdir().unwrap();
        std::fs::write(ex.path().join("mix.exs"), "  app: :phoenix_app,\n").unwrap();
        assert_eq!(manifest_name(ex.path()).as_deref(), Some("phoenix_app"));

        let gradle = tempfile::tempdir().unwrap();
        std::fs::write(
            gradle.path().join("settings.gradle.kts"),
            "rootProject.name = \"declared-gradle\"\n",
        )
        .unwrap();
        assert_eq!(
            manifest_name(gradle.path()).as_deref(),
            Some("declared-gradle")
        );

        let cabal = tempfile::tempdir().unwrap();
        std::fs::write(
            cabal.path().join("sample.cabal"),
            "name: cabal-project\nversion: 0.1.0\n",
        )
        .unwrap();
        assert_eq!(
            manifest_name(cabal.path()).as_deref(),
            Some("cabal-project")
        );

        let setup_cfg = tempfile::tempdir().unwrap();
        std::fs::write(
            setup_cfg.path().join("setup.cfg"),
            "[options]\nname = wrong\n\n[metadata]\nname = cfg-project\n",
        )
        .unwrap();
        assert_eq!(
            manifest_name(setup_cfg.path()).as_deref(),
            Some("cfg-project")
        );

        let precedence = tempfile::tempdir().unwrap();
        std::fs::write(
            precedence.path().join("pyproject.toml"),
            "[project]\nname = \"pyproject-wins\"\n",
        )
        .unwrap();
        std::fs::write(
            precedence.path().join("setup.cfg"),
            "[metadata]\nname = cfg-loses\n",
        )
        .unwrap();
        assert_eq!(
            manifest_name(precedence.path()).as_deref(),
            Some("pyproject-wins")
        );
    }

    #[test]
    fn ruby_vendor_bundle_is_the_only_vendor_dependency_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Gemfile"),
            "source \"https://rubygems.org\"\n",
        )
        .unwrap();
        let vendor = tmp.path().join("vendor");
        std::fs::create_dir_all(vendor.join("bundle/gems")).unwrap();
        std::fs::create_dir_all(vendor.join("handwritten")).unwrap();
        assert_eq!(classify_gated(tmp.path(), "vendor"), None);
        assert_eq!(classify_gated(&vendor, "bundle"), Some(Deps));
        assert_eq!(classify_gated(&vendor, "handwritten"), None);
        assert_eq!(
            artifact_ecosystem_at(&vendor, &["rb".into()], "bundle"),
            Some("rb")
        );
        assert_eq!(
            artifact_ecosystem_at(tmp.path(), &["rb".into()], "vendor"),
            None
        );
    }
}
