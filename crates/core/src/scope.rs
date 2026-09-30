//! Effective scan scope (#41): which roots swamp actually looks at, and
//! why -- built-in defaults, detector results (`crate::locations`), and
//! config `include`/`exclude`/`disabled_detectors`, folded into one
//! reusable, serde-serializable [`EffectiveScope`] every command shares.
//!
//! This module is pure and fixture-injectable: [`resolve_effective_scope`]
//! takes an explicit [`crate::locations::Environment`], [`ScanConfig`],
//! optional explicit command roots, and a detector [`crate::locations::Registry`],
//! and returns a value -- no home-directory or filesystem access happens
//! anywhere else in this file except the read-only presence/readability
//! check in [`stat_root`]. Tests inject a temp-dir "home" so they never
//! touch the real one.
//!
//! Explicitly **not** this chunk's job (left for #42/#50, see
//! `.oh/handoffs/2026-09-21-claude-full-scope.md` section 3 and
//! `.oh/sessions/2026-09-21-scope-and-detector-registry.md`): making the
//! walker/observation pipeline actually consume more than one root
//! coherently against shared per-volume history. `resolve_effective_scope`
//! only decides *which* roots are in scope; #42 makes multi-root
//! *observation* of that scope coherent.

use crate::locations::{
    Environment, LocationStatus, Platform, ProposedLocation, Provenance, Registry, StorageCategory,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The `[scan]` table in `config.toml`. Every field optional; a missing
/// `[scan]` table is the all-defaults value (`defaults = true`, no
/// includes/excludes/disabled detectors).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanConfig {
    /// Whether swamp may infer scope at all.
    ///
    /// `true` (the default): the built-in default roots (`~/src`,
    /// `~/Library/Developer`, `~/Library/Caches` on macOS) plus every
    /// detector that is not named in `disabled_detectors`.
    ///
    /// `false` means **explicit-only scope**: swamp infers nothing. Only
    /// `include` entries, explicit command roots, and detectors the
    /// config names are in scope. With `defaults = false` and neither
    /// `enabled_detectors` nor `disabled_detectors` set, the scope is
    /// genuinely empty and every command says so.
    ///
    /// An earlier revision of this file read `defaults = false` as
    /// "drop the builtin-defaults detector only, keep inferring from
    /// every other detector", recorded as a judgment call. The user
    /// rejected that reading (see the dated correction in
    /// `.oh/sessions/2026-09-21-scope-and-detector-registry.md`); the
    /// explicit-only contract above is the one in force.
    pub defaults: bool,
    /// Additional roots, always in scope regardless of `defaults`.
    /// `~` and relative-to-home paths are resolved against the
    /// environment's home directory.
    pub include: Vec<String>,
    /// Roots (or root prefixes) pruned from scope, applied *after*
    /// defaults/detectors/includes are resolved. Always wins: an
    /// exclusion removes a root regardless of source, including an
    /// explicit command-line root.
    pub exclude: Vec<String>,
    /// Detector IDs to exclude from resolution entirely (the detector's
    /// `detect()` is still called by the registry only to be labelled
    /// `Disabled` for transparency -- see `crate::locations::Registry::resolve`).
    pub disabled_detectors: Vec<String>,
    /// Detector IDs explicitly turned **on**.
    ///
    /// Under `defaults = false` (explicit-only scope), this is the
    /// documented way to say "explicit-only scope, but do still look at
    /// my Hugging Face cache" -- the deny-list `disabled_detectors`
    /// controls the opposite direction there.
    ///
    /// Under `defaults = true` (the ordinary case), this is instead the
    /// way to turn on a detector that opts out of ordinary scope on its
    /// own (`Detector::default_enabled` returning `false` -- a
    /// system-wide install tree like Homebrew, off by default because it
    /// is shared by every account on the machine and can hold bytes
    /// nothing to do with any project). Naming an ordinary opt-out
    /// detector here does nothing extra: it was already going to run.
    #[serde(default)]
    pub enabled_detectors: Vec<String>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            defaults: true,
            include: Vec::new(),
            exclude: Vec::new(),
            disabled_detectors: Vec::new(),
            enabled_detectors: Vec::new(),
        }
    }
}

/// Whether detector-inferred roots may enter the scope at all
/// (`.oh/guardrails/explicit-only-scope-when-defaults-false.md`).
///
/// With `defaults = true` this is always `true` -- the deny-list
/// `disabled_detectors` decides which detectors run.
///
/// With `defaults = false` the scope is explicit-only, so swamp infers
/// nothing unless the config *speaks about detectors*: either an
/// `enabled_detectors` allow-list (the documented mechanism) or a
/// non-empty `disabled_detectors` deny-list, which is an equally
/// explicit curation of the detector set ("run everything except
/// these"). Neither present means no detector runs and the scope is
/// exactly `include` plus any explicit command roots -- empty when there
/// are none, which every command reports rather than falling back to cwd
/// or home.
pub fn detectors_permitted(config: &ScanConfig) -> bool {
    if config.defaults {
        return true;
    }
    !(config.enabled_detectors.is_empty() && config.disabled_detectors.is_empty())
}

impl ScanConfig {
    /// The `[scan]` table body `config init` writes, documenting every
    /// key with its meaning and default.
    pub fn to_toml_table(&self) -> String {
        let include = toml_string_array(&self.include);
        let exclude = toml_string_array(&self.exclude);
        let disabled = toml_string_array(&self.disabled_detectors);
        let enabled = toml_string_array(&self.enabled_detectors);
        format!(
            "\n[scan]\n\
# Built-in default roots (~/src, ~/Library/Developer, ~/Library/Caches on\n\
# macOS) plus every enabled detector's results. false = explicit-only\n\
# scope: only `include` plus detectors not named in `disabled_detectors`.\n\
defaults = {}\n\
# Extra roots always in scope, e.g. [\"~/code\", \"/Volumes/data/src\"].\n\
include = {}\n\
# Roots (or root prefixes) pruned from scope; always wins over defaults,\n\
# detectors, and include, e.g. [\"~/src/scratch\"].\n\
exclude = {}\n\
# Detector IDs to turn off without excluding a path another enabled\n\
# root already reaches, e.g. [\"homebrew\"]. `swamp scope --json` lists ids.\n\
disabled_detectors = {}\n\
# Detector IDs explicitly turned on: under defaults = false (explicit-only\n\
# scope), the only way any detector runs at all; under defaults = true,\n\
# the way to turn on a detector that is off by default on its own (a\n\
# system-wide install tree, e.g. \"homebrew\" -- `swamp scope` marks these\n\
# `disabled (default off)`). With defaults = false and neither list set,\n\
# no detector runs at all and the scope is `include` plus explicit roots.\n\
enabled_detectors = {}\n",
            self.defaults, include, exclude, disabled, enabled
        )
    }
}

fn toml_string_array(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let quoted: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Why a root ended up in scope. A root can have more than one reason
/// (e.g. it is both a built-in default and something the user also
/// listed under `include`); every reason is retained, never collapsed
/// to "the first one found".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum RootReason {
    BuiltinDefault,
    Detector {
        detector_id: String,
        category: StorageCategory,
        provenance: Provenance,
    },
    Included,
    ExplicitCommand,
    /// Propagated up from a root that was folded into this one because
    /// it is a subdirectory of it (see `RootStatus::SkippedAsNested`).
    NestedFrom {
        path: PathBuf,
    },
}

/// The filesystem-facing state of a root already selected as an
/// in-scope candidate (i.e. it survived detector/defaults/include
/// selection). This is a different vocabulary from
/// `crate::locations::LocationStatus`, which is about whether a
/// *detector* could resolve a candidate at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RootStatus {
    Present,
    Missing,
    Unreadable {
        reason: String,
    },
    /// Folded into `parent` because it is a subdirectory of another
    /// in-scope root; not scanned separately. `parent`'s `reasons` gains
    /// a `NestedFrom` entry for this path, per #41's "retained as an
    /// inclusion reason".
    SkippedAsNested {
        parent: PathBuf,
    },
    /// Pruned by a config `exclude` entry; always wins over every other
    /// source, including an explicit command-line root.
    Excluded {
        pattern: String,
    },
}

/// One root in the effective scope, with every reason it is there and
/// its current filesystem status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRoot {
    pub path: PathBuf,
    pub reasons: Vec<RootReason>,
    pub status: RootStatus,
}

impl ScopeRoot {
    pub fn is_scan_target(&self) -> bool {
        matches!(self.status, RootStatus::Present | RootStatus::Missing)
    }

    /// Whether this root gets ordinary project treatment -- Git/ecosystem
    /// discovery, project grouping, and the unowned-remainder walk --
    /// versus a *detector location*, which is measured only as an
    /// external unit (`crate::external::discover_and_measure`) and never
    /// walked for projects (#R13 item B, "project roots vs. detector
    /// locations"). A root is a project root when the user named it
    /// (`include`/an explicit command root) or it is the one built-in
    /// `~/src`-style habit; a root whose only reasons are detector
    /// results (Cargo home, rustup, Homebrew, `~/Library/Caches`,
    /// `~/Library/Developer`, ...) is a detector location, even when it
    /// is also present and readable.
    ///
    /// `NestedFrom` never makes a root a project root by itself -- it
    /// only says this root absorbed another one's walk; whether *this*
    /// root gets walked at all still turns on its own other reasons.
    pub fn is_project_root(&self) -> bool {
        self.reasons.iter().any(|r| {
            matches!(
                r,
                RootReason::BuiltinDefault | RootReason::Included | RootReason::ExplicitCommand
            )
        })
    }
}

/// One detector's full output for `swamp scope` transparency, including
/// entries that never became a root (not-present, disabled, unresolved).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorSummary {
    pub detector_id: String,
    pub name: String,
    /// Raw detector output, before exclusion, dedup and explicit-root
    /// replacement. Private: discovery consumes the authorized
    /// [`EffectiveScope`] roots, never these candidates
    /// (`.oh/guardrails/discovery-consumes-effective-scope.md`), and a
    /// destructuring `let DetectorSummary { locations, .. }` outside this
    /// module does not compile.
    locations: Vec<ProposedLocation>,
}

impl DetectorSummary {
    /// The raw candidates, for `swamp scope`'s explanation of what each
    /// detector proposed and why it was or was not authorized. The gate
    /// audit allows calling this only from the CLI's scope printer.
    pub fn locations_for_display(&self) -> &[ProposedLocation] {
        &self.locations
    }
}

/// A detector that exists in the catalog and does not apply to the
/// platform this scope was resolved for.
///
/// Reported, not omitted (#84). "Not applicable" and "found nothing"
/// are different answers and a user is entitled to tell them apart: a
/// Linux user who does not see Xcode in their scope should be able to
/// learn that Xcode is a macOS detector, rather than wonder whether
/// detection failed, whether a permission was missing, or whether they
/// configured something wrong. Silence reads as the third.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotApplicableDetector {
    pub detector_id: String,
    pub name: String,
    /// The platforms it does apply to, so the note is a fact rather
    /// than a shrug.
    pub applies_to: Vec<Platform>,
}

/// A reusable, serde-serializable description of exactly what swamp will
/// scan for one invocation, and why -- shared by every command
/// (`scope`, `report`, `observe`, `ui`, `schedule`) through
/// [`resolve_effective_scope`], never recomputed ad hoc per command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveScope {
    pub catalog_version: String,
    /// The platform this scope was resolved for. A scope is a set of
    /// platform conventions as much as a set of paths (`~/Library/...`
    /// on macOS, `$XDG_CACHE_HOME` on Linux), so the persisted and
    /// `--json` forms say which one produced it -- otherwise a scope
    /// read on the other machine looks like a pile of missing roots
    /// with no explanation.
    #[serde(default = "default_scope_platform")]
    pub platform: Platform,
    pub generated_at: u64,
    pub defaults_enabled: bool,
    pub disabled_detectors: Vec<String>,
    /// The subset of [`Self::disabled_detectors`] that is off because
    /// the detector itself defaults to off (`Detector::default_enabled`
    /// -- a system-wide install tree like Homebrew), not because the
    /// user's config named it. `swamp scope` reports these `disabled
    /// (default off)` rather than plain `disabled`.
    #[serde(default)]
    pub default_off_detectors: Vec<String>,
    pub configured_include: Vec<String>,
    pub configured_exclude: Vec<String>,
    /// True when this scope was resolved from explicit command-line
    /// roots (which replace inferred/default roots for the invocation,
    /// though configured exclusions still apply).
    pub explicit: bool,
    /// Every root considered, with reasons and status. Filter with
    /// [`ScopeRoot::is_scan_target`] and `status == Present` for the
    /// paths an observation should actually walk.
    pub roots: Vec<ScopeRoot>,
    /// Full per-detector transparency, independent of whether a
    /// detector's output became a root (see `DetectorSummary`).
    pub detectors: Vec<DetectorSummary>,
    /// Detectors that do not apply to [`EffectiveScope::platform`],
    /// reported rather than silently absent. Empty on the platform a
    /// detector was written for.
    #[serde(default)]
    pub not_applicable_detectors: Vec<NotApplicableDetector>,
    /// Subtrees an `exclude` entry names that fall *inside* an in-scope
    /// root, i.e. a subtree the walker should prune once #50 wires
    /// exclusion patterns into the walk itself. Exposed now so the
    /// contract exists before that integration lands.
    pub pruned_subtrees: Vec<PruneNote>,
    /// Detector-resolved (external-unit-eligible) subtrees folded into a
    /// kept root by nesting, pruned from that root's ordinary walk so
    /// `crate::external::discover_and_measure`'s independent measurement
    /// of the same path is the sole count for its bytes. See
    /// [`ExternalPruneNote`].
    #[serde(default)]
    pub external_pruned_subtrees: Vec<ExternalPruneNote>,
    /// Every `configured_exclude` entry after tilde expansion and
    /// normalization, in the one spelling every exclusion comparison
    /// uses.
    ///
    /// Carried on the scope because [`EffectiveScope::exclusion_for`]
    /// needs it and has no `Environment` to expand `~` with. The 2026-09-22
    /// re-review's CE1 was exactly a comparison that could not reach
    /// this list: `authorized_detector_paths_in_explicit_roots` tested
    /// exclusion only against roots whose *status* was `Excluded`, and
    /// under `--root <parent>` an excluded home is not a root at all.
    #[serde(default)]
    pub normalized_exclude: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneNote {
    pub root: PathBuf,
    pub pattern: String,
}

/// A detector-resolved location that is also a candidate external unit
/// (`crate::external::discover_and_measure` measures it independently)
/// and that folded into a kept, ordinarily-walked root as a nested
/// subdirectory (see the folding pass in [`resolve_effective_scope`]).
/// Recorded so `report_scope_with_source` can prune `path` out of
/// `root`'s ordinary walk and note why: without this, the same bytes
/// would be counted twice -- once as `root`'s walked/unowned total, once
/// as the external unit's own measurement (see the B2 gap fixed
/// alongside the developer-storage detector catalog, #45-#49, and
/// `.oh/sessions/2026-09-21-detector-catalog-and-multi-root-ui.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalPruneNote {
    /// The kept, ordinarily-walked root this subtree is pruned from.
    pub root: PathBuf,
    /// The nested subtree, pruned from `root`'s walk because it is
    /// separately measured as an external unit.
    pub path: PathBuf,
    pub detector_id: String,
}

/// One root the effective scope authorizes a discovery/observation pass
/// to look at, with everything that pass needs to decide *how*.
/// [`EffectiveScope::authorized_roots`] is the only supported way to get
/// these: nothing outside `scope.rs` may reinterpret raw detector
/// candidates (`.oh/guardrails/discovery-consumes-effective-scope.md`).
#[derive(Debug, Clone)]
pub struct AuthorizedRoot {
    pub path: PathBuf,
    /// The detector that proposed this root, when one did. `None` for a
    /// configured `include` or an explicit command root.
    pub detector_id: Option<String>,
    /// The detector's display name, and how it classified this
    /// location. Carried here so a discovery pass never has to read
    /// `scope.detectors` back for a label -- the whole point of the
    /// authorized seam is that scope decides and discovery is told.
    pub detector_name: Option<String>,
    pub category: StorageCategory,
    pub provenance: Provenance,
    /// `true` when this root is folded into a larger walked root and is
    /// measured separately as its own unit (see [`ExternalPruneNote`]).
    pub nested_in: Option<PathBuf>,
    /// Exclusion patterns that fall inside this root; a discovery pass
    /// must not descend into them.
    pub pruned_subtrees: Vec<PathBuf>,
}

/// Why a root the scope *considered* is not available to this
/// observation. Never silently dropped: a caller turns these into
/// coverage notes so "nothing found" and "not looked at" stay distinct
/// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
#[derive(Debug, Clone)]
pub struct UnauthorizedRoot {
    pub path: PathBuf,
    /// `true` when the root is deliberately out of scope (excluded, a
    /// disabled detector, outside the explicit command roots) and
    /// `false` when it is in scope but could not be observed (missing,
    /// unreadable). Only the latter is a coverage gap.
    pub out_of_scope: bool,
    pub reason: String,
}

impl EffectiveScope {
    /// Every root this scope authorizes a discovery or measurement pass
    /// to look at, and every root it considered but did not, with the
    /// reason.
    ///
    /// This is the single seam between "what the user authorized" and
    /// "what swamp looks at". `external::discover_and_measure` and
    /// `agents::discover_and_measure` consume *this*, never
    /// `self.detectors`' raw `Resolved` candidates: the review's
    /// `excluded_agent_home_must_not_be_scanned` counterexample was
    /// exactly a discovery pass that read detector output directly and
    /// so never saw the exclusion.
    ///
    /// Explicit command roots replace inferred ones: when `self.explicit`
    /// is set, a detector-proposed path is authorized only if it lies
    /// inside an explicit, present root.
    pub fn authorized_roots(&self) -> (Vec<AuthorizedRoot>, Vec<UnauthorizedRoot>) {
        let mut authorized = Vec::new();
        let mut unauthorized = Vec::new();

        for root in &self.roots {
            let detector_id = root.reasons.iter().find_map(|r| match r {
                RootReason::Detector { detector_id, .. } => Some(detector_id.clone()),
                _ => None,
            });
            let pruned: Vec<PathBuf> = self
                .pruned_subtrees
                .iter()
                .filter(|p| p.root == root.path)
                .map(|p| PathBuf::from(&p.pattern))
                .collect();
            match &root.status {
                RootStatus::Excluded { pattern } => unauthorized.push(UnauthorizedRoot {
                    path: root.path.clone(),
                    out_of_scope: true,
                    reason: format!("excluded by {pattern}"),
                }),
                RootStatus::Missing => unauthorized.push(UnauthorizedRoot {
                    path: root.path.clone(),
                    out_of_scope: false,
                    reason: "not present on disk".to_string(),
                }),
                RootStatus::Unreadable { reason } => unauthorized.push(UnauthorizedRoot {
                    path: root.path.clone(),
                    out_of_scope: false,
                    reason: format!("could not be read ({reason})"),
                }),
                RootStatus::Present | RootStatus::SkippedAsNested { .. } => {
                    // Defence in depth: a root whose ancestor is
                    // excluded is out of scope even if resolution
                    // recorded it as present. Through the one predicate,
                    // so this path and the explicit-root path below
                    // cannot disagree.
                    if let Some(pattern) = self.exclusion_for(&root.path) {
                        unauthorized.push(UnauthorizedRoot {
                            path: root.path.clone(),
                            out_of_scope: true,
                            reason: format!("excluded by {pattern}"),
                        });
                        continue;
                    }
                    if self.explicit && detector_id.is_some() && !self.inside_explicit(&root.path) {
                        unauthorized.push(UnauthorizedRoot {
                            path: root.path.clone(),
                            out_of_scope: true,
                            reason:
                                "outside the explicit command roots, which replace inferred roots"
                                    .to_string(),
                        });
                        continue;
                    }
                    let nested_in = match &root.status {
                        RootStatus::SkippedAsNested { parent } => Some(parent.clone()),
                        _ => None,
                    };
                    let (detector_name, category, provenance) = detector_id
                        .as_deref()
                        .and_then(|id| self.detector_label(id, &root.path))
                        .unwrap_or((
                            None,
                            StorageCategory::Unclassified,
                            Provenance::BuiltinConvention,
                        ));
                    authorized.push(AuthorizedRoot {
                        path: root.path.clone(),
                        detector_id,
                        detector_name,
                        category,
                        provenance,
                        nested_in,
                        pruned_subtrees: pruned,
                    });
                }
            }
        }
        (authorized, unauthorized)
    }

    /// How the detector that proposed `path` labels it. Only `scope.rs`
    /// reads the detector summaries; callers receive the label on their
    /// [`AuthorizedRoot`].
    fn detector_label(
        &self,
        detector_id: &str,
        path: &Path,
    ) -> Option<(Option<String>, StorageCategory, Provenance)> {
        let summary = self
            .detectors
            .iter()
            .find(|d| d.detector_id == detector_id)?;
        let loc = summary
            .locations
            .iter()
            .find(|l| l.path.as_deref() == Some(path))
            .or_else(|| summary.locations.first())?;
        Some((
            Some(summary.name.clone()),
            loc.category,
            loc.provenance.clone(),
        ))
    }

    /// Why `path` is excluded from this scope, or `None`.
    ///
    /// **The only** exclusion predicate. Every caller -- the default
    /// path, the explicit-root path, and the ownership window the growth
    /// store sweeps with -- routes through here, so explicit and
    /// configured scope cannot diverge. It consults all three places an
    /// exclusion can be recorded:
    ///
    /// * a root whose status is [`RootStatus::Excluded`] (the ordinary
    ///   case: the excluded path was itself a candidate root);
    /// * a [`PruneNote`], which is how an `exclude` entry *inside* a
    ///   kept root is recorded -- the case `--root <parent>` produces
    ///   and the case the re-review's CE1 walked through; and
    /// * `normalized_exclude` directly, so an entry that matched neither
    ///   of the above (a path that does not exist yet, an entry outside
    ///   every root) still excludes.
    ///
    /// Both sides are compared through [`comparable`], so an entry
    /// written as `/var/folders/...` covers a unit the external pass
    /// reports as `/private/var/folders/...`. The two discovery families
    /// disagreed about spelling before (external canonicalizes its
    /// candidates, agents does not), which meant one `exclude` entry
    /// could cover one family and not the other -- the re-review's P2.
    pub fn exclusion_for(&self, path: &Path) -> Option<String> {
        let candidate = comparable(path);
        for root in &self.roots {
            if let RootStatus::Excluded { pattern } = &root.status
                && under(&candidate, &comparable(&root.path))
            {
                return Some(pattern.clone());
            }
        }
        for note in &self.pruned_subtrees {
            let pattern = PathBuf::from(&note.pattern);
            if under(&candidate, &comparable(&pattern)) {
                return Some(note.pattern.clone());
            }
        }
        for ex in &self.normalized_exclude {
            if under(&candidate, &comparable(ex)) {
                return Some(ex.display().to_string());
            }
        }
        None
    }

    /// Whether this invocation's authorized scope includes Docker.
    ///
    /// The detector must be enabled *and* its resolved home must not be
    /// excluded, and under `--root` it must lie inside an explicit root
    /// -- the same three tests every other location passes. Consulted by
    /// the report pipeline so `consumers::docker` never asks the daemon
    /// about a tool the user did not authorize
    /// (the 2026-09-22 re-review's CE6).
    pub fn docker_in_scope(&self) -> bool {
        let id = crate::locations::docker_desktop::DOCKER_DESKTOP_DETECTOR_ID;
        if !self.detector_enabled(id) {
            return false;
        }
        let authorized = if self.explicit {
            self.authorized_detector_paths_in_explicit_roots()
        } else {
            self.authorized_roots().0
        };
        // A detector that is enabled but resolved no present home has
        // nothing in scope -- and nothing to ask the daemon about.
        authorized
            .iter()
            .any(|r| r.detector_id.as_deref() == Some(id))
    }

    /// Every authorized detector-resolved root the external and agent
    /// unit families measure under, de-duplicated and in scope order.
    ///
    /// This is the set that gets its own FSEvents cursor
    /// (`crate::growth::replay_unit_roots`). Without one, a default
    /// install's event window comes only from the walked project roots,
    /// which never contain `~/.claude` or `~/.cargo`, so no stored
    /// measurement of either is ever reusable and the event gate does
    /// nothing outside a scope that happens to contain a tool home.
    ///
    /// Derived from the same authorized seam the two families derive
    /// their own candidate lists from, so a disabled detector, an
    /// exclusion or an explicit-root replacement removes a root's cursor
    /// exactly as it removes its units
    /// (`.oh/guardrails/discovery-consumes-effective-scope.md`). The
    /// builtin-defaults pseudo-detector is dropped: it proposes the
    /// configured scan roots themselves, which the walk already anchors.
    pub fn authorized_unit_roots(&self) -> Vec<PathBuf> {
        let roots = if self.explicit {
            self.authorized_detector_paths_in_explicit_roots()
        } else {
            self.authorized_roots().0
        };
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for root in roots {
            let Some(detector_id) = root.detector_id.as_deref() else {
                continue;
            };
            if detector_id == crate::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID {
                continue;
            }
            if seen.insert(root.path.clone()) {
                out.push(root.path);
            }
        }
        out
    }

    /// Whether this scope lets a detector contribute at all.
    ///
    /// Distinct from "did it resolve a present home": a detector can be
    /// enabled and simply find nothing. Project-local storage that a
    /// detector *governs* without proposing a home for it (Aider's
    /// per-repository files) is in scope exactly when the detector is,
    /// so disabling the detector still turns off everything it governs.
    pub fn detector_enabled(&self, detector_id: &str) -> bool {
        !self.disabled_detectors.iter().any(|d| d == detector_id)
    }

    /// Whether a detector-proposed path lies inside one of the explicit
    /// command roots this invocation was given.
    fn inside_explicit(&self, path: &Path) -> bool {
        self.roots.iter().any(|r| {
            matches!(r.status, RootStatus::Present)
                && r.reasons
                    .iter()
                    .any(|x| matches!(x, RootReason::ExplicitCommand))
                && path.starts_with(&r.path)
        })
    }

    /// Detector-proposed paths inside the explicit command roots, for a
    /// discovery pass running under `--root`: the detector catalog still
    /// applies, but only within what the user named. Returns the
    /// authorized subset plus the detector locations that were dropped
    /// because they sit outside every explicit root.
    pub fn authorized_detector_paths_in_explicit_roots(&self) -> Vec<AuthorizedRoot> {
        if !self.explicit {
            return Vec::new();
        }
        let mut out = Vec::new();
        for summary in &self.detectors {
            for loc in &summary.locations {
                if loc.status != LocationStatus::Resolved {
                    continue;
                }
                let Some(path) = &loc.path else { continue };
                let path = lexically_normalize(path);
                // The one exclusion predicate, not a second
                // reimplementation of it. This function used to test
                // `self.roots` entries whose status was `Excluded` and
                // nothing else, so under `--root <parent>` an excluded
                // home -- recorded as a `PruneNote` inside the explicit
                // root, never as a root of its own -- matched nothing
                // and was discovered, measured and made actionable
                // (the 2026-09-22 re-review's CE1).
                if self.inside_explicit(&path) && self.exclusion_for(&path).is_none() {
                    out.push(AuthorizedRoot {
                        detector_id: Some(summary.detector_id.clone()),
                        detector_name: Some(summary.name.clone()),
                        category: loc.category,
                        provenance: loc.provenance.clone(),
                        nested_in: None,
                        pruned_subtrees: Vec::new(),
                        path,
                    });
                }
            }
        }
        out
    }

    /// The detector-proposed paths inside the explicit command roots
    /// that are *out of scope* this invocation, with the reason.
    ///
    /// The explicit-root path needs this for the same reason
    /// `authorized_roots` returns `UnauthorizedRoot`s: a nested location
    /// that is excluded must be subtracted from its parent's absorbed
    /// bytes and from the ownership window, or a config-only change
    /// reads as growth and then as regrowth
    /// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    pub fn unauthorized_detector_paths_in_explicit_roots(&self) -> Vec<UnauthorizedRoot> {
        if !self.explicit {
            return Vec::new();
        }
        let mut out = Vec::new();
        for summary in &self.detectors {
            for loc in &summary.locations {
                if loc.status != LocationStatus::Resolved {
                    continue;
                }
                let Some(path) = &loc.path else { continue };
                let path = lexically_normalize(path);
                if let Some(pattern) = self.exclusion_for(&path) {
                    out.push(UnauthorizedRoot {
                        path,
                        out_of_scope: true,
                        reason: format!("excluded by {pattern}"),
                    });
                } else if !self.inside_explicit(&path) {
                    out.push(UnauthorizedRoot {
                        path,
                        out_of_scope: true,
                        reason: "outside the explicit command roots, which replace inferred roots"
                            .to_string(),
                    });
                }
            }
        }
        out
    }

    /// Paths an observation should actually walk: present, in-scope,
    /// not folded into a parent, not excluded.
    pub fn scan_paths(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter(|r| matches!(r.status, RootStatus::Present))
            .map(|r| r.path.clone())
            .collect()
    }

    /// True when nothing at all is in scope: no default, detector,
    /// include, or explicit root survived resolution. Distinct from
    /// "every candidate root happens to be missing on disk" -- that is
    /// still an explicit, non-empty scope. Callers must treat this as an
    /// error/notice, never fall back to cwd or home (#41).
    pub fn is_empty_scope(&self) -> bool {
        !self.roots.iter().any(|r| {
            matches!(
                r.status,
                RootStatus::Present | RootStatus::Missing | RootStatus::Unreadable { .. }
            )
        })
    }
}

fn expand_tilde(home: &Path, raw: &str) -> PathBuf {
    let raw = raw.trim();
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

/// Lexically normalizes `.`/`..` components and trailing slashes without
/// touching the filesystem -- deliberately not `fs::canonicalize`, which
/// would follow symlinks. Root *resolution* here only ever dereferences
/// a root path itself once, when checking presence in [`stat_root`];
/// it never recursively follows symlinks found while normalizing or
/// walking (see #41's "never recursively follow symlinks").
fn lexically_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn normalize(home: &Path, raw: &str) -> PathBuf {
    lexically_normalize(&expand_tilde(home, raw))
}

fn normalize_path(home: &Path, p: &Path) -> PathBuf {
    match p.to_str() {
        Some(s) => normalize(home, s),
        None => lexically_normalize(p),
    }
}

/// Read-only presence/readability check. A single, non-recursive stat:
/// this dereferences the root path itself (ordinary `stat` behavior for
/// a symlinked root, matching how the walker will open it) but never
/// follows anything found *inside* the directory.
fn stat_root(path: &Path) -> RootStatus {
    match crate::fs_gate::metadata_following(path) {
        Ok(_) => RootStatus::Present,
        Err(e) if e.kind() == ErrorKind::NotFound => RootStatus::Missing,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => RootStatus::Unreadable {
            reason: "permission denied".to_string(),
        },
        Err(e) => RootStatus::Unreadable {
            reason: e.to_string(),
        },
    }
}

/// The one spelling every scope comparison uses.
///
/// `fs::canonicalize` when the path exists, lexical normalization
/// otherwise. This is *not* the walker following symlinks -- the walker
/// still never dereferences anything it finds inside a directory
/// (`.oh/guardrails/symlinks-never-followed.md`). It is the comparison
/// namespace: `external::discover_and_measure` canonicalizes every
/// candidate and `agents::discover_and_measure` does not, so the same
/// fixture home comes back as `/var/folders/.../claude` from one pass
/// and `/private/var/folders/.../claude` from the other. With both sides
/// of every `exclude`/`protect` comparison passed through here, an entry
/// written in either spelling covers both families (the 2026-09-22
/// re-review's P2).
pub fn comparable(path: &Path) -> PathBuf {
    let normalized = lexically_normalize(path);
    if let Ok(c) = crate::fs_gate::canonicalize(&normalized) {
        return c;
    }
    // A path that does not exist yet still has to land in the same
    // namespace as one that does, or a `protect` entry on an existing
    // directory would not cover a member below it that has not been
    // created -- and protecting in advance is explicitly supported.
    // Canonicalize the longest existing ancestor and re-append the rest.
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = normalized.as_path();
    while let Some(parent) = cursor.parent() {
        let Some(name) = cursor.file_name() else {
            break;
        };
        tail.push(name.to_os_string());
        if let Ok(c) = crate::fs_gate::canonicalize(parent) {
            let mut out = c;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return out;
        }
        cursor = parent;
    }
    normalized
}

/// Component-wise containment: `/a/bc` is not under `/a/b`.
pub fn under(candidate: &Path, ancestor: &Path) -> bool {
    candidate == ancestor || candidate.starts_with(ancestor)
}

/// `path` relative to `base`, or `None` when it does not lie beneath it.
/// The one spelling of "relative to" the execution sinks use, so a sink
/// holds no path containment logic of its own.
pub fn relative_to<'a>(path: &'a Path, base: &Path) -> Option<&'a Path> {
    path.strip_prefix(base).ok()
}

/// Whether two selected paths overlap (one is, or lies beneath, the
/// other): the "parent and child selected together" refusal the sinks
/// apply before anything moves. Containment logic lives here and in
/// `protection`, never in a sink module.
pub fn overlapping(a: &Path, b: &Path) -> bool {
    under(a, b) || under(b, a)
}

fn is_excluded(path: &Path, excludes: &[PathBuf]) -> Option<PathBuf> {
    if excludes.is_empty() {
        return None;
    }
    // Compare explicit roots in the same namespace as agent/external
    // exclusions: /var and /private/var must not bypass an exclusion.
    let path = comparable(path);
    excludes
        .iter()
        .find(|ex| under(&path, &comparable(ex)))
        .cloned()
}

/// Resolves the effective scan scope for one invocation. Pure: every
/// filesystem/environment input arrives through `env`; the only I/O is
/// the read-only presence/readability stat in [`stat_root`].
///
/// `explicit_roots`: when non-empty, these entirely replace inferred
/// defaults/detectors/includes for this invocation (#41's explicit-root
/// precedence); configured `exclude` entries still apply.
pub fn resolve_effective_scope(
    env: &Environment,
    config: &ScanConfig,
    explicit_roots: &[PathBuf],
    registry: &Registry,
    generated_at: u64,
) -> EffectiveScope {
    let permitted = crate::locations::permitted::PermittedDetectors::from_config(config, registry);
    let per_detector = registry.resolve(env, &permitted);

    let detectors: Vec<DetectorSummary> = registry
        .detectors()
        .iter()
        .filter(|d| d.platforms().contains(&env.platform))
        .filter_map(|d| {
            per_detector
                .iter()
                .find(|(id, _)| id == d.id())
                .map(|(id, locs)| DetectorSummary {
                    detector_id: id.clone(),
                    name: d.name().to_string(),
                    locations: locs.clone(),
                })
        })
        .collect();

    // The other half of per-detector transparency: what this platform
    // does not have, named. Deliberately independent of
    // `disabled_detectors` -- a detector can be both, and "you turned
    // this off" is a different sentence from "this OS has no such
    // thing".
    let mut not_applicable_detectors: Vec<NotApplicableDetector> = registry
        .detectors()
        .iter()
        .filter(|d| !d.platforms().contains(&env.platform))
        .map(|d| NotApplicableDetector {
            detector_id: d.id().to_string(),
            name: d.name().to_string(),
            applies_to: d.platforms().to_vec(),
        })
        .collect();
    not_applicable_detectors.sort_by(|a, b| a.detector_id.cmp(&b.detector_id));

    // candidate: (normalized path, reason)
    let mut candidates: Vec<(PathBuf, RootReason)> = Vec::new();

    if !explicit_roots.is_empty() {
        for r in explicit_roots {
            candidates.push((normalize_path(&env.home, r), RootReason::ExplicitCommand));
        }
    } else {
        for summary in &detectors {
            for loc in &summary.locations {
                if loc.status != LocationStatus::Resolved {
                    continue;
                }
                let Some(path) = &loc.path else { continue };
                let normalized = normalize_path(&env.home, path);
                // Only the `~/src`-style habit gets ordinary project
                // treatment (Git/ecosystem discovery, unowned remainder).
                // Every other builtin-defaults candidate
                // (`~/Library/Developer`, `~/Library/Caches`, the XDG
                // cache root) is a detector location like any other tool
                // home: measured only as an external unit, never walked
                // as a project root (#R13 item B). Matched by identity
                // against the one path `locations::builtin` proposes for
                // this purpose, not by index, so a future reordering of
                // that detector's candidate list cannot silently flip
                // which one is treated as a project root.
                let reason = if summary.detector_id
                    == crate::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID
                    && normalized == env.home.join("src")
                {
                    RootReason::BuiltinDefault
                } else {
                    RootReason::Detector {
                        detector_id: summary.detector_id.clone(),
                        category: loc.category,
                        provenance: loc.provenance.clone(),
                    }
                };
                candidates.push((normalized, reason));
            }
        }
        for inc in &config.include {
            candidates.push((normalize(&env.home, inc), RootReason::Included));
        }
    }

    // `BuiltinDefault` reasons are folded onto the same detector-sourced
    // candidates below (the built-in-defaults detector's own
    // `RootReason::Detector` entries already carry that information);
    // `RootReason::BuiltinDefault` is kept as a type for forward
    // compatibility / doc clarity but every current builtin-defaults
    // candidate arrives tagged `Detector { detector_id: "builtin-defaults", .. }`,
    // which is more informative (it also carries provenance/category).

    let excludes: Vec<PathBuf> = config
        .exclude
        .iter()
        .map(|e| normalize(&env.home, e))
        .collect();

    // Merge duplicate normalized paths, retaining every reason.
    let mut order: Vec<PathBuf> = Vec::new();
    let mut reasons_by_path: HashMap<PathBuf, Vec<RootReason>> = HashMap::new();
    for (path, reason) in candidates {
        if !reasons_by_path.contains_key(&path) {
            order.push(path.clone());
        }
        reasons_by_path.entry(path).or_default().push(reason);
    }

    // Exclusion first: wins over everything, including explicit roots.
    let mut roots: Vec<ScopeRoot> = Vec::new();
    let mut kept: Vec<PathBuf> = Vec::new(); // survives exclusion, candidate for nesting
    let mut pruned_subtrees: Vec<PruneNote> = Vec::new();

    for path in &order {
        let reasons = reasons_by_path.remove(path).unwrap_or_default();
        if let Some(pattern) = is_excluded(path, &excludes) {
            roots.push(ScopeRoot {
                path: path.clone(),
                reasons,
                status: RootStatus::Excluded {
                    pattern: pattern.display().to_string(),
                },
            });
        } else {
            kept.push(path.clone());
            roots.push(ScopeRoot {
                path: path.clone(),
                reasons,
                status: stat_root(path), // provisional; nesting pass below may override
            });
        }
    }

    // Exclusion patterns that fall *inside* a kept root: exposed as
    // pruned subtrees for the future walker integration, per #41's
    // "exclusions prune matching subtrees".
    if !excludes.is_empty() {
        let comparable_roots: Vec<(&PathBuf, PathBuf)> =
            kept.iter().map(|root| (root, comparable(root))).collect();
        for ex in &excludes {
            let comparable_ex = comparable(ex);
            for (root, comparable_root) in &comparable_roots {
                let Ok(relative) = comparable_ex.strip_prefix(comparable_root) else {
                    continue;
                };
                if relative.as_os_str().is_empty() {
                    continue;
                }
                // The walker receives paths in the spelling of its root. The
                // exclusion may use another spelling of the same location (for
                // example /private/var/... while the explicit root is /var/...);
                // compare canonically, then translate the suffix back under the
                // traversal root so the prune actually matches emitted paths.
                pruned_subtrees.push(PruneNote {
                    root: (*root).clone(),
                    pattern: root.join(relative).display().to_string(),
                });
            }
        }
    }

    // Nested folding: shallowest-first, a kept root that is a
    // subdirectory of another kept root is folded into the parent.
    // Parent's reasons gain a `NestedFrom` entry.
    let mut by_depth: Vec<&PathBuf> = kept.iter().collect();
    by_depth.sort_by_key(|p| p.components().count());

    let mut top_level: Vec<PathBuf> = Vec::new();
    let mut nested_of: HashMap<PathBuf, PathBuf> = HashMap::new();
    for p in by_depth {
        if let Some(parent) = top_level.iter().find(|tl| *p != **tl && p.starts_with(tl)) {
            nested_of.insert(p.clone(), parent.clone());
        } else {
            top_level.push(p.clone());
        }
    }

    for root in &mut roots {
        if let Some(parent) = nested_of.get(&root.path) {
            root.status = RootStatus::SkippedAsNested {
                parent: parent.clone(),
            };
        }
    }

    // A folded (nested) candidate that is itself detector-resolved (and
    // therefore also an external-unit candidate, per `crate::external`)
    // must be pruned from its parent's ordinary walk -- otherwise its
    // bytes are counted both there and in the external unit's own
    // measurement (the B2 gap fixed alongside #45-#49; see
    // `ExternalPruneNote`'s doc comment). Read before consuming
    // `nested_of` below.
    //
    // No `builtin-defaults` exemption here (unlike before #R13 item B):
    // that detector's own project-root candidate (`~/src`) never carries
    // a `RootReason::Detector` at all any more (it is
    // `RootReason::BuiltinDefault`), so it can never match this arm --
    // and its *other* candidates (`~/Library/Caches`,
    // `~/Library/Developer`, the XDG cache root) are detector locations
    // like any other, measured only by `crate::external`, so nesting one
    // of them under a kept project root needs exactly the same prune as
    // Cargo home or Homebrew would.
    let mut external_pruned_subtrees: Vec<ExternalPruneNote> = Vec::new();
    for (child, parent) in &nested_of {
        let Some(child_root) = roots.iter().find(|r| &r.path == child) else {
            continue;
        };
        for reason in &child_root.reasons {
            if let RootReason::Detector { detector_id, .. } = reason {
                external_pruned_subtrees.push(ExternalPruneNote {
                    root: parent.clone(),
                    path: child.clone(),
                    detector_id: detector_id.clone(),
                });
            }
        }
    }
    // De-duplicate: two detector reasons for the same folded path (e.g.
    // a duplicate registration) must produce one prune note, not one per
    // reason.
    external_pruned_subtrees.sort_by(|a, b| (&a.root, &a.path).cmp(&(&b.root, &b.path)));
    external_pruned_subtrees.dedup_by(|a, b| a.root == b.root && a.path == b.path);

    // Propagate nested reasons onto their parent roots.
    let nested_reasons: Vec<(PathBuf, PathBuf)> = nested_of.into_iter().collect();
    for (child, parent) in nested_reasons {
        if let Some(parent_root) = roots.iter_mut().find(|r| r.path == parent) {
            parent_root
                .reasons
                .push(RootReason::NestedFrom { path: child });
        }
    }

    EffectiveScope {
        catalog_version: crate::locations::CATALOG_VERSION.to_string(),
        platform: env.platform,
        generated_at,
        defaults_enabled: config.defaults,
        disabled_detectors: permitted.disabled(),
        default_off_detectors: permitted.default_off(),
        configured_include: config.include.clone(),
        configured_exclude: config.exclude.clone(),
        explicit: !explicit_roots.is_empty(),
        roots,
        detectors,
        not_applicable_detectors,
        pruned_subtrees,
        external_pruned_subtrees,
        normalized_exclude: excludes,
    }
}

/// A scope persisted before this field existed was necessarily written
/// by the build that wrote it, on the machine it ran on. Defaulting to
/// the running platform is the only answer that is not a guess about
/// someone else's machine.
fn default_scope_platform() -> Platform {
    Platform::current()
}

/// A change to what's in scope between two resolutions -- never a claim
/// about bytes on disk (see
/// `.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageChange {
    pub path: PathBuf,
    pub kind: CoverageChangeKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageChangeKind {
    Added,
    Removed,
}

/// Compares two effective scopes' *in-scope* root sets (present/missing,
/// not nested/excluded) and reports what was added or removed, with a
/// one-line reason drawn from the root's reasons/status. Callers use
/// this for a report note like "coverage changed since last observation:
/// +root X (detector Y), -root Z (excluded)" -- this function never
/// touches byte history, and its output must never be mistaken for a
/// storage change.
pub fn coverage_changes(
    previous: &EffectiveScope,
    current: &EffectiveScope,
) -> Vec<CoverageChange> {
    let prev_paths: std::collections::HashSet<&PathBuf> = previous
        .roots
        .iter()
        .filter(|r| r.is_scan_target())
        .map(|r| &r.path)
        .collect();
    let cur_paths: std::collections::HashSet<&PathBuf> = current
        .roots
        .iter()
        .filter(|r| r.is_scan_target())
        .map(|r| &r.path)
        .collect();

    let mut changes = Vec::new();
    for root in &current.roots {
        if root.is_scan_target() && !prev_paths.contains(&root.path) {
            changes.push(CoverageChange {
                path: root.path.clone(),
                kind: CoverageChangeKind::Added,
                reason: reason_label(&root.reasons),
            });
        }
    }
    for root in &previous.roots {
        if root.is_scan_target() && !cur_paths.contains(&root.path) {
            // Explain *why* it's gone using the current scope's record of
            // it, if any (e.g. now excluded); otherwise "no longer in scope".
            let reason = current
                .roots
                .iter()
                .find(|r| r.path == root.path)
                .map(|r| status_label(&r.status))
                .unwrap_or_else(|| "no longer in scope".to_string());
            changes.push(CoverageChange {
                path: root.path.clone(),
                kind: CoverageChangeKind::Removed,
                reason,
            });
        }
    }
    changes
}

fn reason_label(reasons: &[RootReason]) -> String {
    reasons
        .iter()
        .map(|r| match r {
            RootReason::BuiltinDefault => "built-in default".to_string(),
            RootReason::Detector { detector_id, .. } => format!("detector {detector_id}"),
            RootReason::Included => "configured include".to_string(),
            RootReason::ExplicitCommand => "explicit command root".to_string(),
            RootReason::NestedFrom { path } => format!("covers nested {}", path.display()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn status_label(status: &RootStatus) -> String {
    match status {
        RootStatus::Present => "present".to_string(),
        RootStatus::Missing => "missing".to_string(),
        RootStatus::Unreadable { reason } => format!("unreadable ({reason})"),
        RootStatus::SkippedAsNested { parent } => format!("folded into {}", parent.display()),
        RootStatus::Excluded { pattern } => format!("excluded ({pattern})"),
    }
}

/// Persists the resolved scope's roots (and its scalars/lists) as
/// `<store_dir>/scope.parquet` + `scope_values.parquet` +
/// `scope_roots.parquet` + `scope_root_reasons.parquet`, so the next
/// invocation can compute [`coverage_changes`] against it. Coverage
/// bookkeeping only -- never byte history, never consulted by the
/// growth store. The detector summaries and prune notes are not
/// persisted: nothing reads them back, and the next resolution
/// recomputes them.
pub fn persist_effective_scope(store_dir: &Path, scope: &EffectiveScope) -> std::io::Result<()> {
    use crate::growth::columns as c;
    let io = |e: anyhow::Error| std::io::Error::other(e.to_string());
    crate::fs_gate::store::StoreDir::at(store_dir)?.create()?;
    c::write_scope_rows(
        &store_dir.join("scope.parquet"),
        &[c::StoredScopeRow {
            catalog_version: scope.catalog_version.clone(),
            platform: platform_label(scope.platform).to_string(),
            generated_at: scope.generated_at,
            defaults_enabled: scope.defaults_enabled,
            explicit: scope.explicit,
        }],
    )
    .map_err(io)?;
    let mut values = Vec::new();
    for (kind, list) in [
        ("disabled_detector", &scope.disabled_detectors),
        ("default_off_detector", &scope.default_off_detectors),
        ("include", &scope.configured_include),
        ("exclude", &scope.configured_exclude),
    ] {
        for (seq, v) in list.iter().enumerate() {
            values.push(c::StoredScopeValueRow {
                list_kind: kind.to_string(),
                seq: seq as u32,
                value: v.clone(),
            });
        }
    }
    c::write_scope_value_rows(&store_dir.join("scope_values.parquet"), &values).map_err(io)?;
    let mut roots = Vec::new();
    let mut reasons = Vec::new();
    for (seq, r) in scope.roots.iter().enumerate() {
        let (status, status_detail) = match &r.status {
            RootStatus::Present => ("present", None),
            RootStatus::Missing => ("missing", None),
            RootStatus::Unreadable { reason } => ("unreadable", Some(reason.clone())),
            RootStatus::SkippedAsNested { parent } => {
                ("skipped-as-nested", Some(parent.display().to_string()))
            }
            RootStatus::Excluded { pattern } => ("excluded", Some(pattern.clone())),
        };
        roots.push(c::StoredScopeRootRow {
            seq: seq as u32,
            path: r.path.display().to_string(),
            status: status.to_string(),
            status_detail,
        });
        for (rseq, reason) in r.reasons.iter().enumerate() {
            let mut row = c::StoredScopeRootReasonRow {
                root_seq: seq as u32,
                seq: rseq as u32,
                source: String::new(),
                detector_id: None,
                category: None,
                provenance_kind: None,
                provenance_value: None,
                path: None,
            };
            match reason {
                RootReason::BuiltinDefault => row.source = "builtin-default".into(),
                RootReason::Included => row.source = "included".into(),
                RootReason::ExplicitCommand => row.source = "explicit-command".into(),
                RootReason::NestedFrom { path } => {
                    row.source = "nested-from".into();
                    row.path = Some(path.display().to_string());
                }
                RootReason::Detector {
                    detector_id,
                    category,
                    provenance,
                } => {
                    row.source = "detector".into();
                    row.detector_id = Some(detector_id.clone());
                    row.category = Some(crate::external::category_str(*category).to_string());
                    let (kind, value) = provenance_columns(provenance);
                    row.provenance_kind = Some(kind.to_string());
                    row.provenance_value = value;
                }
            }
            reasons.push(row);
        }
    }
    c::write_scope_root_rows(&store_dir.join("scope_roots.parquet"), &roots).map_err(io)?;
    c::write_scope_root_reason_rows(&store_dir.join("scope_root_reasons.parquet"), &reasons)
        .map_err(io)?;
    Ok(())
}

pub fn load_last_effective_scope(store_dir: &Path) -> Option<EffectiveScope> {
    use crate::growth::columns as c;
    let head = c::read_scope_rows(&store_dir.join("scope.parquet"))
        .ok()?
        .into_iter()
        .next()?;
    let values = c::read_scope_value_rows(&store_dir.join("scope_values.parquet")).ok()?;
    let list = |kind: &str| -> Vec<String> {
        let mut v: Vec<&c::StoredScopeValueRow> =
            values.iter().filter(|r| r.list_kind == kind).collect();
        v.sort_by_key(|r| r.seq);
        v.into_iter().map(|r| r.value.clone()).collect()
    };
    let mut root_rows = c::read_scope_root_rows(&store_dir.join("scope_roots.parquet")).ok()?;
    root_rows.sort_by_key(|r| r.seq);
    let mut reason_rows =
        c::read_scope_root_reason_rows(&store_dir.join("scope_root_reasons.parquet")).ok()?;
    reason_rows.sort_by_key(|r| (r.root_seq, r.seq));
    let roots = root_rows
        .into_iter()
        .map(|r| ScopeRoot {
            path: PathBuf::from(&r.path),
            status: match (r.status.as_str(), r.status_detail) {
                ("missing", _) => RootStatus::Missing,
                ("unreadable", d) => RootStatus::Unreadable {
                    reason: d.unwrap_or_default(),
                },
                ("skipped-as-nested", d) => RootStatus::SkippedAsNested {
                    parent: PathBuf::from(d.unwrap_or_default()),
                },
                ("excluded", d) => RootStatus::Excluded {
                    pattern: d.unwrap_or_default(),
                },
                _ => RootStatus::Present,
            },
            reasons: reason_rows
                .iter()
                .filter(|x| x.root_seq == r.seq)
                .map(|x| match x.source.as_str() {
                    "included" => RootReason::Included,
                    "explicit-command" => RootReason::ExplicitCommand,
                    "nested-from" => RootReason::NestedFrom {
                        path: PathBuf::from(x.path.clone().unwrap_or_default()),
                    },
                    "detector" => RootReason::Detector {
                        detector_id: x.detector_id.clone().unwrap_or_default(),
                        category: x
                            .category
                            .as_deref()
                            .and_then(crate::external::category_from_str)
                            .unwrap_or(StorageCategory::Unclassified),
                        provenance: provenance_from_columns(
                            x.provenance_kind.as_deref().unwrap_or(""),
                            x.provenance_value.as_deref(),
                        ),
                    },
                    _ => RootReason::BuiltinDefault,
                })
                .collect(),
        })
        .collect();
    Some(EffectiveScope {
        catalog_version: head.catalog_version,
        platform: platform_from_label(&head.platform),
        generated_at: head.generated_at,
        defaults_enabled: head.defaults_enabled,
        disabled_detectors: list("disabled_detector"),
        default_off_detectors: list("default_off_detector"),
        configured_include: list("include"),
        configured_exclude: list("exclude"),
        explicit: head.explicit,
        roots,
        detectors: Vec::new(),
        not_applicable_detectors: Vec::new(),
        pruned_subtrees: Vec::new(),
        external_pruned_subtrees: Vec::new(),
        normalized_exclude: list("exclude").into_iter().map(PathBuf::from).collect(),
    })
}

fn platform_label(p: Platform) -> &'static str {
    match p {
        Platform::MacOS => "macos",
        Platform::Linux => "linux",
    }
}

fn platform_from_label(s: &str) -> Platform {
    match s {
        "linux" => Platform::Linux,
        _ => Platform::MacOS,
    }
}

fn provenance_columns(p: &Provenance) -> (&'static str, Option<String>) {
    match p {
        Provenance::BuiltinConvention => ("builtin-convention", None),
        Provenance::EnvVar(v) => ("env-var", Some(v.clone())),
        Provenance::ConfigField(v) => ("config-field", Some(v.clone())),
        Provenance::ToolQuery(v) => ("tool-query", Some(v.clone())),
    }
}

fn provenance_from_columns(kind: &str, value: Option<&str>) -> Provenance {
    let v = value.unwrap_or("").to_string();
    match kind {
        "env-var" => Provenance::EnvVar(v),
        "config-field" => Provenance::ConfigField(v),
        "tool-query" => Provenance::ToolQuery(v),
        _ => Provenance::BuiltinConvention,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::{Environment, Platform, Registry};
    use std::collections::HashMap;

    fn env_at(home: &Path) -> Environment {
        Environment::fixture(home.to_path_buf(), HashMap::new(), Platform::MacOS)
    }

    fn mk(dir: &Path, rel: &str) {
        std::fs::create_dir_all(dir.join(rel)).unwrap();
    }

    #[test]
    fn no_config_resolves_builtin_defaults_plus_detectors() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(&env, &ScanConfig::default(), &[], &registry, 1000);
        let paths: Vec<_> = scope.scan_paths();
        assert!(paths.contains(&home.join("src")));
        // Missing candidates (e.g. ~/.cargo, never created in this
        // fixture) remain explicit scope, not silently dropped.
        assert!(
            scope
                .roots
                .iter()
                .any(|r| r.path == home.join(".cargo") && r.status == RootStatus::Missing)
        );
    }

    #[test]
    fn defaults_false_is_explicit_only_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        mk(home, "code");
        mk(home, ".cargo");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let mut cfg = ScanConfig {
            defaults: false,
            ..ScanConfig::default()
        };
        cfg.include.push("~/code".to_string());
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        // ~/src is a builtin default, not an included/detected root:
        // defaults=false drops it.
        assert!(!scope.roots.iter().any(|r| r.path == home.join("src")));
        // ~/code came from `include`, so it survives defaults=false.
        assert!(scope.scan_paths().contains(&home.join("code")));
        // The rejected earlier reading kept inferring from every
        // still-enabled detector here. Explicit-only means explicit:
        // ~/.cargo exists on disk and is *not* in scope, because nothing
        // in the config asked for it.
        assert!(
            !scope.roots.iter().any(|r| r.path == home.join(".cargo")),
            "defaults=false must not infer detector roots"
        );
    }

    /// The other half of the explicit-only contract, named because
    /// `.oh/guardrails/explicit-only-scope-when-defaults-false.md`'s
    /// audit checks for this exact function by name: nothing at all is
    /// in scope, and that is reported rather than replaced by a cwd or
    /// home fallback.
    #[test]
    fn defaults_false_without_includes_or_enabled_detectors_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        mk(home, ".cargo");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(
            &env,
            &ScanConfig {
                defaults: false,
                ..ScanConfig::default()
            },
            &[],
            &registry,
            1000,
        );
        assert!(
            scope.roots.is_empty(),
            "defaults=false with no include and no enabled detectors still inferred {} roots",
            scope.roots.len()
        );
        assert!(scope.is_empty_scope());
        assert!(!detectors_permitted(&ScanConfig {
            defaults: false,
            ..ScanConfig::default()
        }));
    }

    /// Names the reading of "explicit" that [`detectors_permitted`]
    /// actually implements, which was true but undocumented:
    ///
    /// - `disabled_detectors` under `defaults = false` is read as **the
    ///   catalog minus these**. Naming detectors to turn off is itself an
    ///   explicit curation of the detector set ("run everything except
    ///   these"), so the rest still run -- it is not treated as silence.
    /// - `enabled_detectors`, when present, is read as an **allow-list**:
    ///   only what it names runs, and everything else is disabled (see
    ///   `defaults_false_with_an_enabled_detector_allow_list_runs_only_that_detector`).
    ///
    /// Neither list present is the only silence, and that is the empty
    /// scope (`defaults_false_without_includes_or_enabled_detectors_is_empty`,
    /// and `tests/reviewer_counterexamples.rs::defaults_false_must_mean_explicit_only`).
    #[test]
    fn defaults_false_with_only_disabled_detectors_still_runs_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, ".cargo");
        mk(home, ".rustup");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            defaults: false,
            disabled_detectors: vec!["cargo-home".to_string()],
            ..ScanConfig::default()
        };
        assert!(
            detectors_permitted(&cfg),
            "a non-empty disabled_detectors list is an explicit statement about detectors, so \
             detectors are permitted even under defaults = false"
        );
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(
            scope.scan_paths().contains(&home.join(".rustup")),
            "a detector the deny-list does not name still runs: disabled_detectors under \
             defaults = false means the catalog minus these, not an allow-list"
        );
        assert!(
            !scope.roots.iter().any(|r| r.path == home.join(".cargo")),
            "the detector named in disabled_detectors must contribute no root"
        );
        // Still explicit-only in the other direction: the builtin
        // defaults detector never runs under `defaults = false`.
        assert!(!scope.roots.iter().any(|r| {
            r.reasons
                .iter()
                .any(|reason| matches!(reason, RootReason::BuiltinDefault))
        }));
    }

    #[test]
    fn defaults_false_with_an_enabled_detector_allow_list_runs_only_that_detector() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, ".cargo");
        mk(home, ".rustup");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            defaults: false,
            enabled_detectors: vec!["cargo-home".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(scope.scan_paths().contains(&home.join(".cargo")));
        assert!(
            !scope.roots.iter().any(|r| r.path == home.join(".rustup")),
            "a detector outside the allow-list must not run"
        );
    }

    #[test]
    fn authorized_roots_drops_excluded_and_explains_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            exclude: vec!["~/src".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        let (authorized, unauthorized) = scope.authorized_roots();
        assert!(!authorized.iter().any(|r| r.path == home.join("src")));
        let note = unauthorized
            .iter()
            .find(|r| r.path == home.join("src"))
            .expect("an excluded root is reported, never silently dropped");
        assert!(note.out_of_scope);
        assert!(note.reason.contains("excluded"));
        // A candidate that simply is not on disk is a coverage note, not
        // an out-of-scope decision.
        let missing = unauthorized
            .iter()
            .find(|r| r.path == home.join(".cargo"))
            .expect("missing candidates stay visible");
        assert!(!missing.out_of_scope);
    }

    #[test]
    fn explicit_roots_do_not_authorize_detector_paths_outside_them() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "proj");
        mk(home, ".cargo");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(
            &env,
            &ScanConfig::default(),
            &[home.join("proj")],
            &registry,
            1000,
        );
        let (authorized, _) = scope.authorized_roots();
        assert_eq!(authorized.len(), 1);
        assert_eq!(authorized[0].path, home.join("proj"));
        assert!(
            scope
                .authorized_detector_paths_in_explicit_roots()
                .is_empty(),
            "~/.cargo is outside the explicit root and must not be discovered"
        );
    }

    #[test]
    fn disabled_detector_does_not_hide_path_reachable_via_another_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // ~/.cargo lives *inside* ~/src for this fixture, so even with
        // the cargo-home detector disabled, ~/src (a builtin default)
        // still reaches it -- disabling discovery must not hide a path
        // covered by another enabled parent root.
        std::fs::create_dir_all(home.join("src/.cargo")).unwrap();
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            disabled_detectors: vec!["cargo-home".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(scope.scan_paths().contains(&home.join("src")));
        assert!(
            !scope
                .detectors
                .iter()
                .find(|d| d.detector_id == "cargo-home")
                .unwrap()
                .locations
                .iter()
                .any(|l| l.status == LocationStatus::Resolved),
            "disabled detector must report Disabled, not Resolved"
        );
    }

    #[test]
    fn exclusion_wins_over_builtin_default() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            exclude: vec!["~/src".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(!scope.scan_paths().contains(&home.join("src")));
        let root = scope
            .roots
            .iter()
            .find(|r| r.path == home.join("src"))
            .unwrap();
        assert!(matches!(root.status, RootStatus::Excluded { .. }));
    }

    #[test]
    fn exclusion_wins_over_explicit_command_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "proj");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            exclude: vec![home.join("proj").display().to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[home.join("proj")], &registry, 1000);
        assert!(scope.scan_paths().is_empty());
        assert!(matches!(scope.roots[0].status, RootStatus::Excluded { .. }));
    }

    #[test]
    fn explicit_roots_replace_defaults_and_detectors() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        mk(home, "elsewhere");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(
            &env,
            &ScanConfig::default(),
            &[home.join("elsewhere")],
            &registry,
            1000,
        );
        assert_eq!(scope.scan_paths(), vec![home.join("elsewhere")]);
        assert!(scope.explicit);
    }

    #[test]
    fn empty_scope_is_explicit_never_a_cwd_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let env = env_at(home);
        let registry = Registry::with_builtins();
        // Every non-builtin-defaults detector, disabled by id -- derived
        // from the registry itself (not a hand-maintained list) so this
        // test does not silently stop covering "every detector disabled"
        // every time a new detector is added to the catalog (#45-#49).
        let disabled_detectors: Vec<String> = registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != crate::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID)
            .collect();
        let cfg = ScanConfig {
            defaults: false,
            disabled_detectors,
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(scope.is_empty_scope());
        assert!(scope.scan_paths().is_empty());
    }

    #[test]
    fn aliases_tilde_absolute_and_trailing_slash_all_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            include: vec![
                "~/src".to_string(),
                format!("{}/", home.join("src").display()),
            ],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        let matches: Vec<_> = scope
            .roots
            .iter()
            .filter(|r| r.path == home.join("src"))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "aliases of the same path must merge into one root"
        );
        // Two `Included` reasons (once per config entry) plus the
        // builtin-defaults reason, since ~/src is also a default.
        assert!(matches[0].reasons.len() >= 2);
    }

    #[test]
    fn nested_root_folds_into_parent_and_retains_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src/nested");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        // Isolate this test from every detector's own convention paths
        // (some, like Homebrew's /opt/homebrew or CoreSimulator's
        // system-wide runtime volumes, are absolute paths outside the
        // fixture home and may or may not exist on the machine running
        // this test) -- this test is about nested folding under ~/src,
        // not the detector catalog. Disabling every non-builtin-defaults
        // detector (derived from the registry, not hand-enumerated) is
        // what keeps this assertion valid as the catalog grows (#45-#49).
        let non_builtin_detectors: Vec<String> = registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != crate::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID)
            .collect();
        let cfg = ScanConfig {
            include: vec!["~/src/nested".to_string()],
            disabled_detectors: non_builtin_detectors,
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        let nested = scope
            .roots
            .iter()
            .find(|r| r.path == home.join("src/nested"))
            .unwrap();
        assert!(matches!(nested.status, RootStatus::SkippedAsNested { .. }));
        let parent = scope
            .roots
            .iter()
            .find(|r| r.path == home.join("src"))
            .unwrap();
        assert!(parent.reasons.iter().any(
            |r| matches!(r, RootReason::NestedFrom { path } if path == &home.join("src/nested"))
        ));
        // The nested root's own bytes are only reachable via the parent
        // now; only the parent is a scan target.
        assert_eq!(scope.scan_paths(), vec![home.join("src")]);
    }

    #[test]
    fn nested_exclusion_is_recorded_as_a_pruned_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            exclude: vec!["~/src/scratch".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(scope.scan_paths().contains(&home.join("src")));
        assert!(
            scope
                .pruned_subtrees
                .iter()
                .any(|p| p.root == home.join("src") && p.pattern.ends_with("src/scratch"))
        );
    }

    #[test]
    fn missing_optional_paths_remain_explicit_scope_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path(); // nothing created at all
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(&env, &ScanConfig::default(), &[], &registry, 1000);
        assert!(
            !scope.is_empty_scope(),
            "candidates that are simply absent are still explicit scope"
        );
        assert!(
            scope
                .roots
                .iter()
                .any(|r| r.path == home.join("src") && r.status == RootStatus::Missing)
        );
    }

    #[test]
    fn escaped_and_spaced_paths_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join("My Projects/a b")).unwrap();
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let cfg = ScanConfig {
            include: vec!["~/My Projects/a b".to_string()],
            ..ScanConfig::default()
        };
        let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
        assert!(scope.scan_paths().contains(&home.join("My Projects/a b")));
    }

    #[test]
    fn coverage_changes_report_add_and_remove_without_touching_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        mk(home, "src");
        mk(home, "extra");
        let env = env_at(home);
        let registry = Registry::with_builtins();

        let before = resolve_effective_scope(&env, &ScanConfig::default(), &[], &registry, 1000);
        let cfg_after = ScanConfig {
            include: vec!["~/extra".to_string()],
            exclude: vec!["~/Library/Caches".to_string()],
            ..ScanConfig::default()
        };
        let after = resolve_effective_scope(&env, &cfg_after, &[], &registry, 2000);

        let changes = coverage_changes(&before, &after);
        assert!(
            changes
                .iter()
                .any(|c| c.path == home.join("extra") && c.kind == CoverageChangeKind::Added)
        );
        assert!(changes.iter().any(
            |c| c.path == home.join("Library/Caches") && c.kind == CoverageChangeKind::Removed
        ));
        // This function's contract: it never reports bytes, only path/kind/reason.
    }

    #[test]
    fn persisted_scope_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let store = tmp.path().join("store");
        let env = env_at(home);
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(&env, &ScanConfig::default(), &[], &registry, 1000);
        persist_effective_scope(&store, &scope).unwrap();
        let loaded = load_last_effective_scope(&store).expect("round trip");
        assert_eq!(loaded.catalog_version, scope.catalog_version);
        assert_eq!(loaded.roots.len(), scope.roots.len());
    }

    /// #84: "not applicable" is a third answer, beside "found nothing"
    /// and "could not tell". A macOS-only detector is absent from a
    /// Linux scope's `detectors` list entirely, which on its own is
    /// indistinguishable from a detector that ran and resolved nothing
    /// -- and a user chasing "why is Xcode not in my scope" would have
    /// no way to learn that the answer is "because this is Linux".
    #[test]
    fn a_detector_that_does_not_apply_to_this_platform_is_named_not_omitted() {
        use crate::locations::{Environment, Platform, Registry};
        let registry = Registry::with_builtins();

        let linux = resolve_effective_scope(
            &Environment::fixture(
                std::path::PathBuf::from("/home/dev"),
                std::collections::HashMap::new(),
                Platform::Linux,
            ),
            &ScanConfig::default(),
            &[],
            &registry,
            1,
        );
        let named: Vec<&str> = linux
            .not_applicable_detectors
            .iter()
            .map(|d| d.detector_id.as_str())
            .collect();
        for expected in ["xcode", "core-simulator", "docker-desktop"] {
            assert!(
                named.contains(&expected),
                "a Linux scope must say `{expected}` does not apply here, got {named:?}"
            );
        }
        // And it must not appear in both lists: a detector is either
        // something this platform has or something it does not.
        for d in &linux.not_applicable_detectors {
            assert!(
                !linux
                    .detectors
                    .iter()
                    .any(|s| s.detector_id == d.detector_id),
                "`{}` is reported both as applicable and as not applicable",
                d.detector_id
            );
            assert!(
                d.applies_to.contains(&Platform::MacOS) && !d.applies_to.contains(&Platform::Linux),
                "`{}` is listed as not applicable to Linux but claims to apply to it",
                d.detector_id
            );
        }

        // Symmetrically, a macOS scope must not report a macOS detector
        // as inapplicable -- otherwise this list is noise rather than an
        // answer.
        let macos = resolve_effective_scope(
            &Environment::fixture(
                std::path::PathBuf::from("/Users/dev"),
                std::collections::HashMap::new(),
                Platform::MacOS,
            ),
            &ScanConfig::default(),
            &[],
            &registry,
            1,
        );
        let macos_named: Vec<&str> = macos
            .not_applicable_detectors
            .iter()
            .map(|d| d.detector_id.as_str())
            .collect();
        for unexpected in ["xcode", "core-simulator", "docker-desktop"] {
            assert!(
                !macos_named.contains(&unexpected),
                "`{unexpected}` applies on macOS and must not be listed as not applicable: \
                 {macos_named:?}"
            );
        }
    }

    /// The other half of the same fact: a scope records which
    /// platform's conventions produced it, so a scope read on the other
    /// machine is not just a list of missing paths.
    #[test]
    fn a_scope_records_the_platform_it_was_resolved_for() {
        use crate::locations::{Environment, Platform, Registry};
        let registry = Registry::with_builtins();
        for platform in [Platform::MacOS, Platform::Linux] {
            let home = match platform {
                Platform::MacOS => "/Users/dev",
                Platform::Linux => "/home/dev",
            };
            let scope = resolve_effective_scope(
                &Environment::fixture(
                    std::path::PathBuf::from(home),
                    std::collections::HashMap::new(),
                    platform,
                ),
                &ScanConfig::default(),
                &[],
                &registry,
                1,
            );
            assert_eq!(scope.platform, platform);
        }
    }
}
