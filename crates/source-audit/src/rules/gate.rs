//! The capability gate, by exact path reference
//! (`crates/core/src/fs_gate/mod.rs`, `docs/architecture.md` "Capability
//! gates").
//!
//! What the compiler already enforces (proof tokens, private fields, the
//! `Program` enum, clippy's type-resolved `disallowed_methods`) is not
//! re-checked here. These rules check what *names* may appear where:
//!
//! * `gate_paths_only_inside_gates` -- the I/O crates and `std` modules
//!   only inside the gate; each gate group only in the modules allowed
//!   to hold that capability; no `unsafe`/`extern`/`#[path]` outside the
//!   gate; no orphan files; no production build enabling `testing`.
//! * `adapters_do_not_reach_gates` -- agent adapters reach I/O only
//!   through `IdentifyCtx`, never detectors, actions or the environment,
//!   and never print or panic.
//! * `sinks_have_no_path_predicates` -- the execution sinks hold no path
//!   containment logic of their own.
//! * `json_writes_allowlisted` -- no JSON writer outside the gate.
//! * `parquet_writers_are_the_named_fact_tables` -- the one Parquet
//!   writer is reached only from the named table writers, one per fact
//!   table: a derived view (a total, a series, a drill-down) is never
//!   given a table of its own (store-is-facts-report-is-views).
//! * `bus_static_registration` -- consumers never name each other.
//! * `tui_event_thread_has_no_gate_calls` -- the TUI's event/render code
//!   reaches no blocking gate capability except through `worker::spawn`.
//! * `no_unreferenced_public_items` -- a public workspace item nothing
//!   names is dead, and dead code is where a capability hides.

use super::{Rule, verdict};
use crate::model::{Krate, Module, RefKind, Workspace, under};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Modules that are the gate: everything under `fs_gate`, and the
/// FSEvents FFI child of `fs_events`.
pub fn is_gate(m: &Module) -> bool {
    m.is_within(Krate::Core, &["fs_gate"]) || m.is_within(Krate::Core, &["fs_events", "macos"])
}

/// Modules outside `fs_gate` that may lower a gate lint
/// (`#[allow(clippy::disallowed_methods)]` and friends): the FSEvents FFI
/// (a gate module) and the TUI's one thread spawner, which is the TUI's
/// own `std::thread` gate (tui-actions-off-event-thread).
const LINT_ALLOW_MODULES: &[(Krate, &[&str])] = &[
    (Krate::Core, &["fs_events", "macos"]),
    (Krate::Tui, &["worker"]),
];

/// `include!` targets allowed outside a crate's own `src/` (none).
const INCLUDE_ALLOW: &[&str] = &[];

/// External paths only the gate may name (prefix match, segment-wise).
/// `gix` and every `gix_*` crate are here too (see [`names_gated`]): the
/// gitoxide crates re-export file removal, subprocesses, temp files and
/// lock files (`fs_gate::git`).
const GATED: &[&str] = &[
    "gix",
    "std::fs",
    "core::fs",
    "std::os::unix::fs",
    "std::os::fd",
    "std::os::unix::io",
    "std::io::Read",
    "std::io::BufRead",
    "std::io::BufReader",
    "std::io::Seek",
    "std::io::copy",
    "std::io::read_to_string",
    "std::process",
    "libc",
    "tempfile",
    "trash",
    "walkdir",
    "jwalk",
    "tokio::fs",
    "tokio::process",
    "fsevent_sys",
    "core_foundation",
    "core_foundation_sys",
    "parquet",
    "zstd",
];

/// `std::process` items that start nothing.
const PROCESS_OK: &[&str] = &[
    "std::process::exit",
    "std::process::id",
    "std::process::ExitCode",
];

/// Arrow is the column-store modules' vocabulary only.
const ARROW: &[&str] = &["arrow_array", "arrow_schema"];
const ARROW_MODULES: &[&[&str]] = &[
    &["growth", "columns"],
    &["assoc_store"],
    &["store"],
    &["github"],
    &["fs_gate"],
];

/// A gate capability group and the modules that may name it.
struct Group {
    /// Absolute path prefix (`@core::fs_gate::read_dir`).
    path: &'static str,
    /// Modules (crate, path prefix) allowed to reference it.
    allowed: &'static [(Krate, &'static [&'static str])],
    why: &'static str,
}

const WALKERS: &[(Krate, &[&str])] = &[
    (Krate::Core, &["walk"]),
    (Krate::Core, &["attribution"]),
    (Krate::Core, &["folded_measurement"]),
    (Krate::Core, &["scan"]),
    (Krate::Core, &["git"]),
    (Krate::Core, &["signals"]),
    (Krate::Core, &["ignore"]),
    (Krate::Core, &["ecosystem"]),
    (Krate::Core, &["compose"]),
    (Krate::Core, &["cargo_artifacts"]),
    (Krate::Core, &["cargo_cleanup"]),
    (Krate::Core, &["preserve"]),
    (Krate::Core, &["growth"]),
    (Krate::Core, &["locations"]),
    (Krate::Core, &["live_watch"]),
];

const STORE_MODULES: &[(Krate, &[&str])] = &[
    (Krate::Core, &["actions"]),
    (Krate::Core, &["growth"]),
    (Krate::Core, &["protection"]),
    (Krate::Core, &["scope"]),
    (Krate::Core, &["schedule"]),
    (Krate::Core, &["docker"]),
    (Krate::Core, &["ledger"]),
    (Krate::Core, &["report"]),
    (Krate::Core, &["store"]),
    (Krate::Core, &["assoc_store"]),
    (Krate::Core, &["github"]),
    (Krate::Core, &["agents"]),
    (Krate::Tui, &["app"]),
    (Krate::Cli, &[]),
];

const GROUPS: &[Group] = &[
    Group {
        path: "@core::fs_gate::read_dir",
        allowed: WALKERS,
        why: "a directory listing is a traversal: only the walker, the reviewed-unit snapshot and \
              the capped shallow listing may take one (no-second-traversal-on-report-path, \
              agent-adapters-do-not-traverse)",
    },
    Group {
        path: "@core::fs_gate::store",
        allowed: STORE_MODULES,
        why: "writes to swamp's own files belong to the store modules",
    },
    Group {
        path: "@core::fs_gate::StoreDir::at",
        allowed: STORE_MODULES,
        why: "a store directory is built from a caller's path only by the store modules; \
              everything else uses the resolved swamp dir (`StoreDir::resolved`)",
    },
    Group {
        path: "@core::fs_gate::git",
        allowed: &[(Krate::Core, &["signals"]), (Krate::Core, &["ignore"])],
        why: "gitoxide queries belong to the git-signal and ignore-lens modules",
    },
    Group {
        path: "@core::fs_gate::read::read_owned_string",
        allowed: STORE_MODULES,
        why: "a whole-file read is for swamp's own state; anything else is a bounded read",
    },
    Group {
        path: "@core::fs_gate::columns",
        allowed: &[
            (Krate::Core, &["growth"]),
            (Krate::Core, &["assoc_store"]),
            (Krate::Core, &["store"]),
            (Krate::Core, &["github"]),
        ],
        why: "Parquet is the history store's format",
    },
    Group {
        path: "@core::fs_gate::destroy",
        allowed: &[
            (Krate::Core, &["actions"]),
            (Krate::Core, &["cargo_cleanup"]),
            (Krate::Core, &["preserve"]),
            (Krate::Core, &["docker"]),
            (Krate::Tui, &["actions"]),
        ],
        why: "destructive operations belong to the execution sinks",
    },
    Group {
        path: "@core::fs_gate::sys",
        allowed: &[
            (Krate::Core, &["activity"]),
            (Krate::Core, &["report"]),
            (Krate::Core, &["cargo_cleanup"]),
            (Krate::Core, &["occupancy"]),
            (Krate::Core, &["live_watch"]),
        ],
        why: "statfs/flock/O_NOFOLLOW are for the modules that need them",
    },
    Group {
        path: "@core::fs_gate::spawn::run",
        allowed: &[
            (Krate::Core, &["occupancy"]),
            (Krate::Core, &["external_associations"]),
            (Krate::Core, &["locations"]),
            (Krate::Core, &["attribution"]),
            (Krate::Core, &["docker"]),
            (Krate::Core, &["github"]),
            (Krate::Core, &["actions"]),
            (Krate::Core, &["schedule"]),
        ],
        why: "every module that starts a process is one of these",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::named",
        allowed: &[(Krate::Core, &["locations"])],
        why: "only the detector command runner maps a name to a program",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Lsof",
        allowed: &[(Krate::Core, &["occupancy"])],
        why: "lsof is the occupancy probe",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Plutil",
        allowed: &[(Krate::Core, &["external_associations"])],
        why: "plutil reads Xcode plists",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Du",
        allowed: &[(Krate::Core, &["attribution"])],
        why: "du is the --verify-du cross-check",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Docker",
        allowed: &[(Krate::Core, &["docker"])],
        why: "docker is the Docker module's",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Gh",
        allowed: &[(Krate::Core, &["github"])],
        why: "gh is GitHub enrichment's",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Git",
        allowed: &[],
        why: "mutating git runs only through fs_gate::destroy",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Df",
        allowed: &[(Krate::Core, &["actions"])],
        why: "df measures free space around an execution",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Id",
        allowed: &[(Krate::Core, &["schedule"])],
        why: "the launchd domain",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Launchctl",
        allowed: &[(Krate::Core, &["schedule"])],
        why: "the scheduled refresh is a LaunchAgent, and only `schedule` installs it \
              (scheduled-refresh-launchagent)",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Kill",
        allowed: &[(Krate::Core, &["schedule"])],
        why: "liveness of the observation lock holder",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Xcrun",
        allowed: &[],
        why: "xcrun runs only as an allow-listed detector command (`Program::named` in \
              `locations`)",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Brew",
        allowed: &[],
        why: "brew runs only as an allow-listed detector command (`Program::named` in \
              `locations`)",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Defaults",
        allowed: &[],
        why: "defaults runs only as an allow-listed detector command (`Program::named` in \
              `locations`)",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Systemctl",
        allowed: &[(Krate::Core, &["systemd_user"])],
        why: "systemd --user's own manager belongs to the systemd scheduling module",
    },
    Group {
        path: "@core::fs_gate::spawn::Program::Loginctl",
        allowed: &[(Krate::Core, &["systemd_user"])],
        why: "session state (Linger) belongs to the systemd scheduling module",
    },
    Group {
        path: "@core::occupancy::OccupancyState",
        allowed: &[(Krate::Core, &["occupancy"])],
        why: "an occupancy answer is shown as a fact (evidence, or a refusal string); nothing \
              gates a Trash move on it any more (2026-09-23)",
    },
    Group {
        path: "@core::growth::ObservationOwnership::new",
        allowed: &[
            (Krate::Core, &["external"]),
            (Krate::Core, &["agents"]),
            (Krate::Core, &["build_stores"]),
        ],
        why: "an ownership window is built by the discovery pass from the roots it covered \
              completely, never by hand (history-sweeps-are-owned); build_stores owns the \
              BuildStore key family the same way external and agents own theirs",
    },
    Group {
        path: "@core::evidence::Reason::__from_checked_format",
        allowed: &[(Krate::Core, &["evidence"])],
        why: "only the `reason!` macro, which checks the template is not blank, may build a \
              formatted reason this way",
    },
];

/// Token constructors whose module is large enough to hold a second,
/// unreviewed caller: each may be called only from the named functions.
type MintSite = (Krate, &'static [&'static str], &'static str);
const MINT_SITES: &[(&str, &[MintSite], &str)] = &[
    (
        "@core::report::pass::DiscoveryPass::begin",
        &[
            (Krate::Core, &["report"], "observe_scope"),
            (Krate::Core, &["report", "pass"], "for_tests"),
        ],
        "a discovery pass is minted only by `report::observe_scope` \
         (discovery-owned-by-report-pipeline)",
    ),
    (
        "@core::bus::registry::Stage::mint",
        &[
            (Krate::Core, &["bus", "registry"], "run"),
            (Krate::Core, &["bus", "registry"], "for_tests"),
        ],
        "a pipeline stage token is minted only by `EventBus::run` \
         (event-bus-pluggable-consumers)",
    ),
];

/// Token and record types whose struct literal may appear only in the
/// named functions: privacy makes a literal outside the defining module
/// a compile error, and this pins which functions *inside* it build one
/// (re-review 5, finding 1: construction only through the propose,
/// approve and loader paths).
const LITERAL_SITES: &[(&str, &[MintSite], &str)] = &[
    (
        "@core::actions::PlanUnit",
        &[
            (Krate::Core, &["actions"], "unit_from_row"),
            (Krate::Core, &["actions"], "unit_from_external"),
            (Krate::Core, &["actions"], "unit_from_agent"),
        ],
        "a plan unit is built from a report row, an external unit or an agent unit",
    ),
    (
        "@core::fs_gate::destroy::Trashed",
        &[(Krate::Core, &["fs_gate", "destroy"], "trash_move")],
        "a Trash receipt comes only from the move it records",
    ),
];

/// The report functions the TUI may name: the scope-aware observation
/// and the pure merge of per-root reports (`load_last_report` was one
/// of these until R18a-4 deleted it along with the JSON cache it read;
/// nothing in the TUI ever called it).
const TUI_REPORT_API: &[&str] = &[
    "@core::report::observe_scope",
    "@core::report::merge_reports",
    // R12: reads the stored scope-wide tables back by the same
    // `EffectiveScope` the caller already resolved -- no bare root, no
    // walk.
    "@core::report::report_scope_from_store",
];

fn allowed(m: &Module, list: &[(Krate, &[&str])]) -> bool {
    list.iter().any(|(k, p)| m.is_within(*k, p))
}

fn names_gated(abs: &str) -> Option<&'static str> {
    if PROCESS_OK.iter().any(|ok| under(abs, ok)) {
        return None;
    }
    if abs
        .split("::")
        .next()
        .is_some_and(|c| c.starts_with("gix_"))
    {
        return Some("gix_*");
    }
    GATED.iter().copied().find(|g| under(abs, g))
}

/// Method names that on `Path` touch the filesystem and exist nowhere
/// else in the workspace; called on any receiver outside the gate they
/// are a gate violation (clippy's `disallowed_methods` is the
/// type-resolved half, and also covers `is_dir`/`exists`/`metadata`).
const PATH_IO_METHODS: &[&str] = &[
    "read_dir",
    "symlink_metadata",
    "canonicalize",
    "read_link",
    "try_exists",
];

/// `Path` methods that stat (and, but for `symlink_metadata`, follow):
/// as *methods* they are clippy's (`disallowed_methods`, type-resolved);
/// as paths the audit sees them too.
const PATH_STAT_METHODS: &[&str] = &["metadata", "exists", "is_dir", "is_file", "is_symlink"];

pub fn gate_paths_only_inside_gates(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for e in &ws.parse_errors {
        problems.push(format!("does not parse or cannot be found: {e}"));
    }
    for o in &ws.orphans {
        problems.push(format!(
            "{o} is not part of any crate (no `mod` declaration reaches it): code the compiler \
             never builds cannot be audited as if it were, so it may not exist"
        ));
    }
    for script in &ws.unmodelled_build_scripts {
        problems.push(format!(
            "{script}: a build script in a workspace crate the audits do not model: it runs on \
             every `cargo build --workspace`, outside every rule"
        ));
    }
    for (mi, m) in ws.modules.iter().enumerate() {
        let gate = is_gate(m);
        for lit in &m.struct_literals {
            if lit.test {
                continue;
            }
            let abs = ws.resolve(mi, &lit.segments).join("::");
            for (ty, sites, why) in LITERAL_SITES {
                // By resolved path, and -- when resolution gave up (a glob
                // import such as a child module's `use super::*`) -- by the
                // type's name: the conservative reading, as for mint sites.
                let name = ty.rsplit("::").next().unwrap_or(ty);
                let by_name =
                    abs.starts_with('?') && lit.segments.last().is_some_and(|s| s == name);
                if abs != *ty && !by_name {
                    continue;
                }
                let f = lit.in_fn.map(|f| ws.fns[f].name.as_str()).unwrap_or("");
                let ok = sites
                    .iter()
                    .any(|(k, p, name)| m.krate == *k && m.path == *p && f == *name);
                if !ok {
                    problems.push(format!(
                        "{}: a `{}` literal in {}::{f}: {why}",
                        lit.site,
                        ty.trim_start_matches("@core::"),
                        m.display()
                    ));
                }
            }
        }
        for (what, site, test) in &m.lint_allows {
            if !*test && !gate && !allowed(m, LINT_ALLOW_MODULES) {
                problems.push(format!(
                    "{site}: {what} outside the capability gate: the crate root denies the gate's \
                     lints so a path the gate owns does not compile elsewhere; lowering one \
                     re-opens it (crates/core/src/fs_gate)"
                ));
            }
        }
        for inc in &m.includes {
            if inc.test {
                continue;
            }
            let src = format!("{}/src/", m.krate.dir());
            match &inc.target {
                None => problems.push(format!(
                    "{}: `{}!` of a computed path: its target cannot be followed, so what it \
                     splices in is outside every rule",
                    inc.site, inc.kind
                )),
                Some(t) if !t.starts_with(&src) && !INCLUDE_ALLOW.contains(&t.as_str()) => problems
                    .push(format!(
                        "{}: `{}!(\"{t}\")` reaches outside {src}: code and data a crate \
                         splices in live in its own source tree, where every rule reads them",
                        inc.site, inc.kind
                    )),
                Some(t) if !ws.root.join(t).is_file() => problems.push(format!(
                    "{}: `{}!` target {t} does not exist",
                    inc.site, inc.kind
                )),
                Some(_) => {}
            }
        }
        for h in &m.hazards {
            if h.test {
                continue;
            }
            if h.what.contains("#[path]") || !gate {
                problems.push(format!(
                    "{}: {} outside the capability gate",
                    h.site, h.what
                ));
            }
        }
        for r in &m.refs {
            if r.test {
                continue;
            }
            // A bare single identifier in code is a local, not a crate
            // (`trash` the Trash path, not the `trash` crate); `use trash;`
            // and `trash::x` are the crate.
            if r.segments.len() == 1 && r.kind == RefKind::Code {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments).join("::");
            // `Path::read_dir` / `<Path>::metadata` as a path (UFCS, or a
            // method passed as a value): the same filesystem access as the
            // method call.
            if !gate
                && (abs.starts_with("std::path::Path::") || abs.starts_with("std::path::PathBuf::"))
                && abs
                    .rsplit("::")
                    .next()
                    .is_some_and(|l| PATH_IO_METHODS.contains(&l) || PATH_STAT_METHODS.contains(&l))
            {
                problems.push(format!(
                    "{}: `{}` outside the capability gate: filesystem access goes through \
                     crates/core/src/fs_gate",
                    r.site,
                    r.segments.join("::")
                ));
            }
            if !gate && let Some(g) = names_gated(&abs) {
                problems.push(format!(
                    "{}: `{}` names `{g}`{} outside the capability gate (crates/core/src/fs_gate)",
                    r.site,
                    r.segments.join("::"),
                    if r.kind == RefKind::Glob {
                        " (glob import)"
                    } else {
                        ""
                    }
                ));
            }
            if ARROW.iter().any(|a| under(&abs, a))
                && !ARROW_MODULES.iter().any(|p| m.is_within(Krate::Core, p))
            {
                problems.push(format!(
                    "{}: `{}` names Arrow outside the column-store modules (growth::columns, \
                     assoc_store, store, github): table schemas are the column modules' \
                     (dir-mtime-int32-minutes, column-store-parquet-zstd)",
                    r.site,
                    r.segments.join("::")
                ));
            }
            if m.krate == Krate::Tui
                && under(&abs, "std::thread")
                && !m.is_within(Krate::Tui, &["worker"])
                && (abs != "std::thread::sleep")
            {
                problems.push(format!(
                    "{}: `{}` names `std::thread` in the TUI outside `worker.rs`: work leaves the \
                     event thread only through `worker::spawn`, which cannot be joined \
                     (tui-actions-off-event-thread)",
                    r.site,
                    r.segments.join("::")
                ));
            }
            if m.krate == Krate::Tui
                && under(&abs, "@core::report")
                && abs.split("::").count() == 3
                && abs
                    .rsplit("::")
                    .next()
                    .is_some_and(|l| l.starts_with(|c: char| c.is_ascii_lowercase()))
                && !TUI_REPORT_API.iter().any(|a| under(&abs, a))
            {
                problems.push(format!(
                    "{}: the TUI names `{}`: every TUI observation goes through the scope-aware \
                     `report::observe_scope` (or loads a stored report back); a report entry \
                     point that takes a bare root drops the scope's exclusions \
                     (tui-refresh-preserves-scope)",
                    r.site,
                    r.segments.join("::")
                ));
            }
            for (mint, sites, why) in MINT_SITES {
                // By resolved path, and by the last two segments whatever
                // they resolve to (a glob import, a re-export): the
                // conservative reading.
                let tail: Vec<&str> = mint.rsplit("::").take(2).collect();
                let n = r.segments.len();
                let by_name =
                    n >= 2 && r.segments[n - 1] == tail[0] && r.segments[n - 2] == tail[1];
                if under(&abs, mint) || by_name {
                    let f = r.in_fn.map(|f| ws.fns[f].name.as_str()).unwrap_or("");
                    let ok = sites
                        .iter()
                        .any(|(k, p, name)| m.krate == *k && m.path == *p && f == *name);
                    if !ok {
                        problems.push(format!(
                            "{}: `{}` is called in {}::{f}: {why}",
                            r.site,
                            r.segments.join("::"),
                            m.display()
                        ));
                    }
                }
            }
            if gate {
                continue;
            }
            for g in GROUPS {
                if under(&abs, g.path) && !allowed(m, g.allowed) {
                    problems.push(format!(
                        "{}: {} names `{}`, which it may not: {}",
                        r.site,
                        m.display(),
                        g.path.trim_start_matches("@core::"),
                        g.why
                    ));
                }
            }
        }
        if !gate {
            for c in &m.methods {
                if !c.test && PATH_IO_METHODS.contains(&c.name.as_str()) {
                    problems.push(format!(
                        "{}: `.{}()` outside the capability gate: filesystem access goes through \
                         crates/core/src/fs_gate",
                        c.site, c.name
                    ));
                }
            }
        }
    }
    // No production dependency graph may enable the test-fixture API.
    for k in Krate::ALL {
        let manifest = ws.root.join(k.dir()).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap_or_default();
        let mut section = String::new();
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.to_string();
                continue;
            }
            let production = section == "[dependencies]"
                || (section.starts_with("[target.") && section.ends_with(".dependencies]"));
            if production && t.starts_with("swamp-core") && t.contains("testing") {
                problems.push(format!(
                    "{}/Cargo.toml: a [dependencies] entry enables swamp-core's `testing` \
                     feature; the test-fixture API must never be in a production build",
                    k.dir()
                ));
            }
        }
    }
    problems
}

/// The adapter modules: every module under `agents` except the shared
/// machinery (the context, the registry, the support matrix, the unit
/// builder and the bounded reader).
pub fn is_adapter(m: &Module) -> bool {
    m.krate == Krate::Core
        && m.path.first().is_some_and(|s| s == "agents")
        && m.path.len() >= 2
        && !matches!(
            m.path[1].as_str(),
            "registry" | "matrix" | "unit" | "bounded_io"
        )
}

const ADAPTER_DENIED: &[(&str, &str)] = &[
    (
        "@core::fs_gate",
        "adapters reach the filesystem only through `IdentifyCtx` (list, stat, bounded header \
         reads), never the gate directly",
    ),
    (
        "@core::locations",
        "adapters do not reach detectors (agent-adapters-do-not-reach-detectors)",
    ),
    (
        "@core::actions",
        "adapters are inspection-only (agent-adapters-are-inspection-only)",
    ),
    (
        "@core::recheck",
        "adapters are inspection-only (agent-adapters-are-inspection-only)",
    ),
    (
        "@core::authority",
        "adapters are inspection-only (agent-adapters-are-inspection-only)",
    ),
    (
        "@core::protection",
        "adapters never read or change human keep intent",
    ),
    (
        "std::env",
        "adapters take every path from the context they are handed \
         (agent-adapters-are-environment-free)",
    ),
    (
        "std::thread",
        "adapters run on the thread they are called on",
    ),
    (
        "std::io::stdout",
        "adapters emit nothing (agent-adapters-do-not-emit-content)",
    ),
    (
        "std::io::stderr",
        "adapters emit nothing (agent-adapters-do-not-emit-content)",
    ),
];

/// Types from the gate an adapter may name: pure data about a `stat`
/// already taken through its context.
const ADAPTER_GATE_TYPES: &[&str] = &[
    "@core::fs_gate::Metadata",
    "@core::fs_gate::MetadataExt",
    "@core::fs_gate::FileType",
];

const EMITTING_MACROS: &[&str] = &[
    "panic",
    "print",
    "println",
    "eprint",
    "eprintln",
    "dbg",
    "todo",
    "unimplemented",
    "unreachable",
    "assert",
    "assert_eq",
    "assert_ne",
];

const EMITTING_METHODS: &[&str] = &["unwrap", "expect", "unwrap_err", "expect_err"];

/// Candidate-unit methods only the shared machinery may call.
const ADAPTER_DENIED_METHODS: &[&str] = &[
    "replayed",
    "push_replayed_member",
    "withdraw_for_unverified_layout",
    "set_resolved_link",
    "into_parts",
];

/// The tool adapters (a module under `agents` declaring a `*_TOOL_ID`),
/// by their module path, e.g. `@core::agents::cline`.
fn tool_adapters(ws: &Workspace) -> Vec<String> {
    ws.modules
        .iter()
        .filter(|m| is_adapter(m) && !m.test)
        .filter(|m| m.str_consts.iter().any(|(n, _, _)| n.ends_with("_TOOL_ID")))
        .map(|m| m.abs().join("::"))
        .collect()
}

pub fn adapters_do_not_reach_gates(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    let tools = tool_adapters(ws);
    for (mi, m) in ws.modules.iter().enumerate() {
        if !is_adapter(m) {
            continue;
        }
        let own = m.abs().join("::");
        for r in &m.refs {
            if r.test {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments).join("::");
            // One tool's adapter never names another's: shared mechanics
            // live in a family module that declares no tool id
            // (agent-adapters-are-pluggable).
            if let Some(other) = tools
                .iter()
                .find(|t| **t != own && !under(&own, t) && under(&abs, t))
            {
                problems.push(format!(
                    "{}: adapter {} names `{}`, inside another tool's adapter ({}): shared \
                     mechanics belong in a family module (agent-adapters-are-pluggable)",
                    r.site,
                    m.display(),
                    r.segments.join("::"),
                    other.trim_start_matches("@core::")
                ));
            }
            if ADAPTER_GATE_TYPES.iter().any(|t| under(&abs, t)) {
                continue;
            }
            for (p, why) in ADAPTER_DENIED {
                if under(&abs, p) {
                    problems.push(format!(
                        "{}: adapter {} names `{}`: {why}",
                        r.site,
                        m.display(),
                        r.segments.join("::")
                    ));
                }
            }
            if let Some(last) = r.segments.last()
                && (EMITTING_METHODS.contains(&last.as_str())
                    || ADAPTER_DENIED_METHODS.contains(&last.as_str()))
                && r.segments.len() > 1
            {
                problems.push(format!(
                    "{}: adapter {} names `{}`",
                    r.site,
                    m.display(),
                    r.segments.join("::")
                ));
            }
        }
        for l in &m.literals {
            let v = l.value.as_str();
            if !l.test
                && !v.contains(char::is_whitespace)
                && (v.starts_with("/Users/") || v.starts_with("/home/") || v.starts_with("~/"))
            {
                problems.push(format!(
                    "{}: adapter {} holds the home path \"{v}\" as a literal: adapters take every \
                     path from the context they are handed (agent-adapters-are-environment-free)",
                    l.site,
                    m.display()
                ));
            }
        }
        for (lit, site) in &m.path_literals {
            problems.push(format!(
                "{site}: adapter {} builds the path \"{lit}\" from a literal: adapters take every \
                 path from the context they are handed (agent-adapters-are-environment-free)",
                m.display()
            ));
        }
        for c in &m.methods {
            if c.test {
                continue;
            }
            if EMITTING_METHODS.contains(&c.name.as_str()) {
                problems.push(format!(
                    "{}: adapter {} calls `.{}()`: a panic prints the value it carries -- a \
                     path, a header -- to stderr (agent-adapters-do-not-emit-content)",
                    c.site,
                    m.display(),
                    c.name
                ));
            }
            if ADAPTER_DENIED_METHODS.contains(&c.name.as_str()) {
                problems.push(format!(
                    "{}: adapter {} calls `.{}()`, which only the shared agents machinery may",
                    c.site,
                    m.display(),
                    c.name
                ));
            }
        }
        for mc in &m.macros {
            if !mc.test && EMITTING_MACROS.contains(&mc.name.as_str()) {
                problems.push(format!(
                    "{}: adapter {} invokes `{}!` in production code \
                     (agent-adapters-do-not-emit-content)",
                    mc.site,
                    m.display(),
                    mc.name
                ));
            }
        }
    }
    problems
}

/// The execution sinks: the modules that move or remove data.
fn is_sink(m: &Module) -> bool {
    m.is_within(Krate::Core, &["actions"])
        || m.is_within(Krate::Core, &["cargo_cleanup"])
        || m.is_within(Krate::Core, &["preserve"])
        || m.is_within(Krate::Core, &["fs_gate", "destroy"])
        || m.is_within(Krate::Tui, &["actions"])
}

const CONTAINMENT: &[&str] = &["starts_with", "strip_prefix", "ancestors"];

pub fn sinks_have_no_path_predicates(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for m in &ws.modules {
        if !is_sink(m) {
            continue;
        }
        for c in &m.methods {
            if !c.test && CONTAINMENT.contains(&c.name.as_str()) {
                problems.push(format!(
                    "{}: `.{}()` in execution sink {}: containment belongs to \
                     `protection::ProtectList::conflict` and `scope::{{under, overlapping, \
                     relative_to}}`, so a sink cannot grow a second, one-directional protection \
                     predicate (protection-fails-closed)",
                    c.site,
                    c.name,
                    m.display()
                ));
            }
        }
        for r in &m.refs {
            if r.test {
                continue;
            }
            if let Some(last) = r.segments.last()
                && CONTAINMENT.contains(&last.as_str())
                && r.segments.len() > 1
            {
                problems.push(format!(
                    "{}: `{}` in execution sink {}",
                    r.site,
                    r.segments.join("::"),
                    m.display()
                ));
            }
        }
    }
    problems
}

pub fn json_writes_allowlisted(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for (mi, m) in ws.modules.iter().enumerate() {
        if is_gate(m) {
            continue;
        }
        for r in &m.refs {
            if r.test {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments).join("::");
            if under(&abs, "serde_json::to_writer")
                || under(&abs, "serde_json::to_writer_pretty")
                || under(&abs, "serde_json::ser")
            {
                problems.push(format!(
                    "{}: `{}` outside the gate: JSON reaches disk only through \
                     `fs_gate::store::write_json`, whose `JsonFile` variants are the allow-list \
                     (json-persistence-is-allowlisted)",
                    r.site,
                    r.segments.join("::")
                ));
            }
        }
    }
    problems
}

/// One function per Parquet table the store holds, and nothing else may
/// reach `fs_gate::columns::write_parquet_atomic`
/// (`.oh/guardrails/store-is-facts-report-is-views.md`, R20). Every name
/// here is a *fact* table: rows that come from an observation (a walk, a
/// detector, a daemon answer, a `git` query) or the reverse-delta history
/// of those rows. What a `Report` shows that is a function of these --
/// `summary`, `reconciliation`, `series_by_key`/`total_series`,
/// `dirs_by_worktree`/`files_by_worktree`, an artifact's growth -- is
/// computed at read time (`growth::derive_report_views`) and has no
/// entry here on purpose. Adding a table means adding it here, to
/// `crates/core/tests/store_contents_are_allowlisted.rs`'s exact list,
/// and to `docs/architecture.md`'s table, with the reason it is a fact.
const TABLE_WRITERS: &[(&str, &str)] = &[
    // growth::columns -- one row writer per table
    ("growth::columns", "write_rows"),
    ("growth::columns", "write_external_rows"),
    ("growth::columns", "write_folded_rows"),
    ("growth::columns", "write_dir_rows"),
    ("growth::columns", "write_file_rows"),
    ("growth::columns", "write_unowned_rows"),
    ("growth::columns", "write_unowned_list_rows"),
    ("growth::columns", "write_evidence_rows"),
    ("growth::columns", "write_topology_rows"),
    ("growth::columns", "write_protect_rows"),
    ("growth::columns", "write_project_rows"),
    ("growth::columns", "write_worktree_rows"),
    ("growth::columns", "write_worktree_fact_rows"),
    ("growth::columns", "write_artifact_shape_rows"),
    ("growth::columns", "write_artifact_shape_list_rows"),
    ("growth::columns", "write_unit_rows"),
    ("growth::columns", "write_unit_consumer_rows"),
    ("growth::columns", "write_agent_member_rows"),
    ("growth::columns", "write_nested_artifact_rows"),
    ("growth::columns", "write_nested_artifact_list_rows"),
    ("growth::columns", "write_nested_artifact_evidence_rows"),
    ("growth::columns", "write_coverage_rows"),
    ("growth::columns", "write_note_rows"),
    ("growth::columns", "write_run_rows"),
    ("growth::columns", "write_dir_track_rows"),
    ("growth::columns", "write_git_signal_rows"),
    ("growth::columns", "write_git_signal_value_rows"),
    ("growth::columns", "write_cargo_replay_meta_rows"),
    // the one write every `table!`-declared table goes through (R18b)
    ("growth::columns", "write_table"),
    // the other stores' own single writers
    ("github", "write_cache"),
    ("assoc_store", "write"),
    ("store", "write"),
];

pub fn parquet_writers_are_the_named_fact_tables(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    for (mi, m) in ws.modules.iter().enumerate() {
        for r in &m.refs {
            if r.test {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments).join("::");
            if abs != "@core::fs_gate::columns::write_parquet_atomic" {
                continue;
            }
            let (module, f) = match r.in_fn {
                Some(f) => (
                    ws.modules[ws.fns[f].module].path.join("::"),
                    ws.fns[f].name.as_str(),
                ),
                None => (m.path.join("::"), ""),
            };
            let named = m.krate == Krate::Core
                && TABLE_WRITERS
                    .iter()
                    .any(|(mod_path, name)| *mod_path == module && *name == f);
            if !named {
                problems.push(format!(
                    "{}: `write_parquet_atomic` reached from `{}`, which is not one of the named \
                     table writers: a table is a fact table, named in TABLE_WRITERS, \
                     store_contents_are_allowlisted and docs/architecture.md -- never a stored \
                     copy of what a read computes (store-is-facts-report-is-views)",
                    r.site,
                    if f.is_empty() {
                        format!("<no function> in `{module}`")
                    } else {
                        format!("{module}::{f}")
                    }
                ));
            }
        }
    }
    problems
}

pub fn bus_static_registration(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    let consumers: HashSet<String> = ws
        .modules
        .iter()
        .filter(|m| m.krate == Krate::Core && m.path.len() == 2 && m.path[0] == "consumers")
        .map(|m| m.path[1].clone())
        .collect();
    for (mi, m) in ws.modules.iter().enumerate() {
        let own = if m.krate == Krate::Core && m.path.len() >= 2 && m.path[0] == "consumers" {
            Some(m.path[1].clone())
        } else {
            None
        };
        for r in &m.refs {
            if r.test {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments);
            if let Some(own) = &own {
                // A consumer may name the consumers module itself (its
                // shared helpers) but no sibling consumer.
                if abs.len() >= 3 && abs[0] == "@core" && abs[1] == "consumers" {
                    let other = &abs[2];
                    if consumers.contains(other) && other != own {
                        problems.push(format!(
                            "{}: consumer `{own}` names consumer `{other}` (`{}`): the bus is the \
                             only coupling between stages (no-consumer-knows-other-consumers)",
                            r.site,
                            r.segments.join("::")
                        ));
                    }
                }
                if r.kind == RefKind::Glob && abs == ["@core", "consumers"] {
                    problems.push(format!(
                        "{}: consumer `{own}` glob-imports the consumers module, which brings \
                         every sibling consumer into scope",
                        r.site
                    ));
                }
                // A stage knows its events, not the bus that runs it.
                if abs.len() >= 3 && abs[0] == "@core" && abs[1] == "bus" && abs[2] == "EventBus" {
                    problems.push(format!(
                        "{}: consumer `{own}` names the `EventBus` (`{}`): a stage receives events \
                         and emits events; registration and dispatch are the bus's \
                         (event-bus-pluggable-consumers)",
                        r.site,
                        r.segments.join("::")
                    ));
                }
            } else if !m.is_within(Krate::Core, &["bus"])
                && !m.is_within(Krate::Core, &["consumers"])
                && abs.len() >= 3
                && abs[0] == "@core"
                && abs[1] == "consumers"
                && consumers.contains(&abs[2])
            {
                problems.push(format!(
                    "{}: {} names consumer `{}` (`{}`): outside the consumers and the bus's \
                     registrar nothing reaches into a stage; a new fact source is a registered \
                     consumer (extractors-are-pluggable)",
                    r.site,
                    m.display(),
                    abs[2],
                    r.segments.join("::")
                ));
            }
        }
    }
    problems
}

/// Gate capabilities that block: what the TUI event thread may not reach
/// except through `worker::spawn`.
const BLOCKING: &[&str] = &[
    "@core::fs_gate::destroy",
    "@core::fs_gate::spawn",
    "@core::fs_gate::read::bounded_read",
    "@core::fs_gate::read::bounded_read_header",
    "@core::fs_gate::read::bounded_string",
    "@core::fs_gate::read_dir",
];

pub fn tui_event_thread_has_no_gate_calls(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    // Functions by absolute path; methods by name (method-call syntax,
    // conservatively: every non-gate workspace method of that name -- a
    // gate object is obtained only through a gate path, which is an edge
    // of its own) and by `(owner or trait, name)` (paths `Type::m`,
    // `Trait::m`).
    let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
    let mut methods: HashMap<String, Vec<usize>> = HashMap::new();
    let mut owned: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (fi, f) in ws.fns.iter().enumerate() {
        if f.test {
            continue;
        }
        let m = &ws.modules[f.module];
        let mut abs = m.abs();
        if let Some(o) = &f.owner {
            abs.push(o.clone());
            if !is_gate(m) {
                methods.entry(f.name.clone()).or_default().push(fi);
            }
            owned
                .entry((o.clone(), f.name.clone()))
                .or_default()
                .push(fi);
            if let Some(t) = &f.impl_trait {
                owned
                    .entry((t.clone(), f.name.clone()))
                    .or_default()
                    .push(fi);
            }
        }
        abs.push(f.name.clone());
        by_path.entry(abs.join("::")).or_default().push(fi);
    }
    // A gate function is a leaf: what it is, not what it calls inside.
    let gate_fn = |fi: usize| is_gate(&ws.modules[ws.fns[fi].module]);
    let mut blocks: Vec<Option<String>> = vec![None; ws.fns.len()];
    for (fi, f) in ws.fns.iter().enumerate() {
        if !gate_fn(fi) {
            continue;
        }
        let m = &ws.modules[f.module];
        let mut abs = m.abs();
        if let Some(o) = &f.owner {
            abs.push(o.clone());
        }
        abs.push(f.name.clone());
        let abs = abs.join("::");
        if let Some(b) = BLOCKING.iter().find(|b| under(&abs, b)) {
            blocks[fi] = Some(b.trim_start_matches("@core::").to_string());
        }
    }
    let mut edges: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); ws.fns.len()];
    for (mi, m) in ws.modules.iter().enumerate() {
        if is_gate(m) {
            continue;
        }
        for r in &m.refs {
            let Some(f) = r.in_fn else { continue };
            if r.test || r.in_worker {
                continue;
            }
            let abs = ws.resolve(mi, &r.segments).join("::");
            if let Some(targets) = by_path.get(&abs) {
                edges[f].extend(targets.iter().copied());
            }
            // `Type::method` / `Trait::method` paths: that type's method, or
            // every implementor's (a UFCS call on a trait with no default
            // body reaches its implementors).
            if r.segments.len() >= 2 {
                let n = r.segments.len();
                let key = (r.segments[n - 2].clone(), r.segments[n - 1].clone());
                if let Some(ms) = owned.get(&key) {
                    edges[f].extend(ms.iter().copied());
                }
            }
        }
        for c in &m.methods {
            let Some(f) = c.in_fn else { continue };
            if c.test || c.in_worker {
                continue;
            }
            if let Some(ms) = methods.get(&c.name) {
                edges[f].extend(ms.iter().copied());
            }
        }
    }
    // What each function reaches: the first blocking capability found.
    let mut reaches: Vec<Option<(usize, String)>> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| b.clone().map(|w| (i, w)))
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for f in 0..ws.fns.len() {
            if reaches[f].is_some() || gate_fn(f) {
                continue;
            }
            if let Some(g) = edges[f].iter().find(|g| reaches[**g].is_some()) {
                reaches[f] = Some((*g, reaches[*g].as_ref().unwrap().1.clone()));
                changed = true;
            }
        }
    }
    // The event thread: everything reachable, over non-worker edges, from
    // the TUI's `event_loop` (which draws, polls and handles keys), plus
    // every workspace impl of a trait the compiler calls *implicitly*
    // (`Drop` at scope end, `Display` inside `format!`, `Deref`, an
    // iterator's `next` in a `for`, operators, comparisons, `Clone`):
    // those have no call site to follow, so they count as reachable from
    // anywhere. References inside closures handed to `worker::spawn` are
    // not edges (dropped above). Startup (`run` before the loop) may read
    // the store; once the loop runs, nothing may block it.
    const IMPLICIT: &[&str] = &[
        "Drop",
        "Display",
        "Debug",
        "Deref",
        "DerefMut",
        "Iterator",
        "Index",
        "IndexMut",
        "Add",
        "Sub",
        "Mul",
        "Div",
        "AddAssign",
        "SubAssign",
        "PartialEq",
        "Eq",
        "PartialOrd",
        "Ord",
        "Hash",
        "Clone",
    ];
    let mut on_thread: BTreeSet<usize> = BTreeSet::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut roots = 0;
    for (fi, f) in ws.fns.iter().enumerate() {
        if f.test || gate_fn(fi) {
            continue;
        }
        let m = &ws.modules[f.module];
        let loop_root = m.krate == Krate::Tui && f.owner.is_none() && f.name == "event_loop";
        let implicit = matches!(m.krate, Krate::Core | Krate::Tui)
            && f.impl_trait
                .as_deref()
                .is_some_and(|t| IMPLICIT.contains(&t));
        if loop_root {
            roots += 1;
        }
        if loop_root || implicit {
            stack.push(fi);
        }
    }
    if roots == 0 {
        problems.push(
            "crates/tui: no `event_loop` function found -- the rule has no event thread to \
             follow, so it cannot pass"
                .to_string(),
        );
    }
    while let Some(f) = stack.pop() {
        if !on_thread.insert(f) {
            continue;
        }
        for g in &edges[f] {
            if !gate_fn(*g) {
                stack.push(*g);
            }
        }
    }
    for &fi in &on_thread {
        let f = &ws.fns[fi];
        for site in &f.opaque_calls {
            problems.push(format!(
                "{site}: `{}` (TUI event-thread code) calls a value whose target is not a path -- \
                 a field, an index, a call's result -- which this rule cannot follow; call a \
                 named function, or do the work in `worker::spawn`",
                f.name
            ));
        }
        for g in &edges[fi] {
            if gate_fn(*g)
                && let Some((_, what)) = &reaches[*g]
            {
                let gf = &ws.fns[*g];
                problems.push(format!(
                    "{}: `{}` runs on the TUI event thread and calls `{}` ({}), blocking gate \
                     capability {what}; do the work in `worker::spawn`",
                    f.site, f.name, gf.name, gf.site
                ));
            }
        }
    }
    problems
}

/// Public items in the workspace crates that nothing names -- no path
/// reference, no method call, no serde attribute string -- anywhere in
/// the workspace, its tests included. Dead public code is where the old
/// audits' blind spots lived (a capability "kept live" by an unused
/// `Drop`, a delivered field nothing fills), and rustc's `dead_code`
/// lint cannot see it because `pub` items of a library are exported.
pub fn no_unreferenced_public_items(ws: &Workspace) -> Vec<String> {
    let mut problems = Vec::new();
    // Every name referenced anywhere: last segments of every path, every
    // method name, and every identifier in test files.
    let mut named: HashSet<String> = HashSet::new();
    for m in &ws.modules {
        for r in &m.refs {
            if r.own_impl {
                // Its own impl blocks name a type (and `Self::f` its
                // methods) without using it.
                continue;
            }
            // Every segment: `Type::f` names `Type` as well as `f`.
            named.extend(r.segments.iter().cloned());
        }
        for c in &m.methods {
            named.insert(c.name.clone());
        }
        for l in &m.literals {
            // `#[serde(serialize_with = "crate::x::f")]` names `f`.
            if let Some(last) = l.value.rsplit("::").next() {
                named.insert(last.to_string());
            }
            // Inline format captures: `format!("{ADAPTER_VERSION}")`.
            for cap in l.value.split('{').skip(1) {
                let id: String = cap
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !id.is_empty() {
                    named.insert(id);
                }
            }
        }
    }
    for id in crate::model::test_identifiers(&ws.root) {
        named.insert(id);
    }
    for m in &ws.modules {
        for site in &m.dead_code_allows {
            problems.push(format!(
                "{site}: `#[allow(dead_code)]` in production code: it hides dead code from rustc, \
                 and a caller it keeps \"alive\" makes dead public API look used \
                 (no-dead-public-evidence-api)"
            ));
        }
    }
    for (name, site, what) in public_definitions(ws) {
        if !named.contains(&name) {
            problems.push(format!(
                "{site}: public {what} `{name}` is named nowhere in the workspace (not by \
                 production code, not by a test): dead public code is where a capability hides \
                 from review; delete it, use it, or make it private so rustc's dead_code lint \
                 sees it"
            ));
        }
    }
    problems
}

fn public_definitions(ws: &Workspace) -> Vec<(String, crate::model::Site, &'static str)> {
    let mut out = Vec::new();
    for f in &ws.fns {
        let m = &ws.modules[f.module];
        if f.test || !f.public {
            continue;
        }
        if f.trait_impl {
            continue;
        }
        if matches!(f.name.as_str(), "main" | "new" | "default" | "fmt" | "drop") {
            continue;
        }
        let _ = m;
        out.push((f.name.clone(), f.site.clone(), "fn"));
    }
    for (name, site) in ws.public_types() {
        out.push((name, site, "type"));
    }
    out
}

pub const RULES: &[Rule] = &[
    ("gate_paths_only_inside_gates", |ws| {
        verdict(
            "gate paths only inside the capability gate",
            gate_paths_only_inside_gates(ws),
        )
    }),
    ("adapters_do_not_reach_gates", |ws| {
        verdict(
            "agent adapters reach I/O only through IdentifyCtx, never detectors, actions or the \
             environment, and never emit",
            adapters_do_not_reach_gates(ws),
        )
    }),
    ("sinks_have_no_path_predicates", |ws| {
        verdict(
            "execution sinks hold no path containment logic",
            sinks_have_no_path_predicates(ws),
        )
    }),
    ("json_writes_allowlisted", |ws| {
        verdict(
            "no JSON writer outside the gate",
            json_writes_allowlisted(ws),
        )
    }),
    ("parquet_writers_are_the_named_fact_tables", |ws| {
        verdict(
            "the Parquet writer is reached only from the named fact-table writers",
            parquet_writers_are_the_named_fact_tables(ws),
        )
    }),
    ("bus_static_registration", |ws| {
        verdict(
            "consumers never name each other",
            bus_static_registration(ws),
        )
    }),
    ("tui_event_thread_has_no_gate_calls", |ws| {
        verdict(
            "TUI event-thread code reaches no blocking gate capability except through \
             worker::spawn",
            tui_event_thread_has_no_gate_calls(ws),
        )
    }),
    ("no_unreferenced_public_items", |ws| {
        verdict(
            "every public item is named somewhere",
            no_unreferenced_public_items(ws),
        )
    }),
];
