# Scope resolution and the location-detector registry

## Aim

Implement #41 (built-in scan defaults, includes, exclusions, detector
overrides) and #44 (source-aware location-detector registry) as one
worker chunk in the `integration/full-scope` sequential chain, per the
handoff at `.oh/handoffs/2026-09-21-claude-full-scope.md` section 3.
Deliver a pure, testable effective-scope model and a small first
detector catalog, without changing per-root observation/history
semantics (that is #42's job) or the full catalog (#45-#49).

## What landed

- `crates/core/src/locations/` (#44): a `Detector` trait, an
  `Environment`/`CommandRunner` fixture-injection seam, and a
  `Registry` with static registration (`with_builtins`, mirroring
  `EventBus::with_builtins`). Four detectors: `builtin-defaults`
  (`~/src`, `~/Library/Developer`, `~/Library/Caches` on macOS;
  nothing on Linux -- #84's job), `cargo-home`, `rustup`, `homebrew`
  (with a bounded, allow-listed `brew --prefix` tool query that always
  falls back to proposing the conventional `/opt/homebrew`/`/usr/local`
  prefixes regardless of whether the query runs or succeeds).
- `crates/core/src/scope.rs` (#41): `ScanConfig` (the `[scan]` table),
  `resolve_effective_scope` (pure: environment/config/explicit-roots/
  registry in, `EffectiveScope` out, with a single non-recursive
  presence/readability stat per candidate root as the only I/O),
  `coverage_changes` (diffs two `EffectiveScope`s into add/remove
  notes), and `persist_effective_scope`/`load_last_effective_scope`
  (`scope.json` under `$SWAMP_DIR`).
- `crates/core/src/growth.rs`: replaced the hand-rolled line parser
  with real TOML (`toml::from_str`), moved `toml` to a workspace
  dependency. `GrowthConfig` gained a `scan: ScanConfig` field.
  `load_config_checked` is the new fallible entry point; the existing
  `load_config` stays infallible (falls back to defaults) for the deep
  report-pipeline call sites that only ever read the four scalar keys
  and were never in scope for this chunk's "fails visibly" requirement.
- CLI (`crates/cli/src/main.rs`): new `swamp scope [--json] [roots...]`
  command; `Report`/`Ui`'s `root` became `Option<PathBuf>`, `Observe`'s
  `roots` is no longer `required = true`. A shared `resolve_scope`/
  `resolve_single_root` pair is the one code path every scope-resolving
  command goes through. `Observe`/`Schedule` (already multi-root) walk
  every present root in the resolved scope; `Report`/`Ui` (still
  single-root end to end) use the scope's first present root and print
  a stderr note when more than one is in scope.
- Docs: `docs/usage.md` (new "Scope and coverage" section, updated
  Configuration), `docs/architecture.md` (new "Effective scope and
  location detectors" section, including the worked pattern for adding
  a detector), `README.md`, `skills/swamp/SKILL.md` and its
  `references/commands-and-json.md`/`coverage-and-history.md`,
  `CHANGELOG.md`.

## Decisions that needed to be made explicitly

**What `defaults = false` means.** The issue text gives two candidate
readings: (a) it disables *only* the three built-in default roots,
leaving every still-enabled detector running, or (b) it disables all
automatic discovery (built-in defaults *and* detectors), leaving only
`include`. I chose (a): `defaults` controls exactly the
`builtin-defaults` detector (folded into `disabled_detectors` for that
one invocation inside `resolve_effective_scope`); every other detector
is controlled independently and only by `disabled_detectors`. Reasons:

- The issue's own scope-semantics bullets list "built-in defaults" and
  "detector results" as two separate items, then separately list
  `disabled_detectors` as the detector-specific override. Reading
  `defaults = false` as also silencing every detector would make
  `disabled_detectors` partially redundant with it and would remove
  the only way to keep e.g. Cargo-home leftover discovery while
  dropping the ordinary `~/src` convention. That said, "Plus enabled
  detector results (from #44 registry)" reads at least as naturally as
  *also* gated by `defaults` as it does as independent of it -- the
  issue text genuinely admits both readings, which is exactly why this
  needed to be a written decision rather than left implicit. Treat
  "`defaults` controls only `builtin-defaults`" as this session's
  judgment call, not an unambiguous reading of #41; reconsider it if
  #45-#50's real usage or the epic owner disagrees.
- "explicit-only scope (includes + enabled detectors unless also
  disabled...)" in the issue text names *enabled detectors* as part of
  explicit-only scope, which only makes sense if `defaults = false`
  does not itself disable them.
- It keeps two independent, single-purpose knobs (`defaults` for the
  three convention paths, `disabled_detectors` for everything else)
  rather than one flag with two different blast radii depending on
  what else is set.

This is a judgment call, not the only defensible reading; it is
recorded here, in `scope.rs`'s `ScanConfig::defaults` doc comment, and
in `docs/usage.md` so a later reader (or #50's worker) can revisit it
if real usage disagrees.

**Nested-root folding.** "a root inside another enabled root is folded
into the parent for measurement, retained as an inclusion reason" --
implemented as: sort candidate (non-excluded) roots shallowest-first,
fold any root that is a path-prefix descendant of an already-kept root
into that parent, mark the folded root's own `ScopeRoot` entry
`SkippedAsNested { parent }` (still listed, for auditability), and push
a `RootReason::NestedFrom { path }` onto the *parent's* reasons list.
Only the parent appears in `scan_paths()`. This applies uniformly
regardless of source (builtin default, detector, include, or explicit
root) -- there is no special case for "this nesting came from a
detector vs. a human's `include`".

**Exclusion vs. "prune matching subtrees".** Exclusion patterns that
exactly match, or are an ancestor of, a candidate *root* remove that
root outright (`RootStatus::Excluded`), and this always wins, including
over an explicit command-line root -- implemented and adversarially
tested. Exclusion patterns that fall *inside* an already-kept root
(e.g. `exclude = ["~/src/scratch"]` with `~/src` in scope) do not
currently prune anything during the walk itself: `EffectiveScope`
records them as `pruned_subtrees` (root + pattern) so the contract
exists and is inspectable now, but nothing in this chunk wires that
list into `walk.rs`. Per the brief, "Do NOT change per-root
observation/history semantics in this chunk (that is #42, next
worker)" -- I read walk-time subtree pruning as squarely inside that
boundary (it changes what the walker actually reads for an unchanged
root), not inside #41's "effective scope" contract, which is about
which *roots* are in scope. **Follow-up for #42/#50: consume
`EffectiveScope.pruned_subtrees` in the walker so a subtree exclusion
inside an otherwise-included root is actually skipped, not just
recorded.**

**Report/Ui remain single-root.** Both take one root end to end
(`report_full_mode`, `swamp_tui::run`) and produce one JSON/TUI state.
Making them multi-root-coherent against per-volume history is #42's
job, explicitly deferred by the brief ("Do NOT change per-root
observation/history semantics in this chunk"), and then #50's job to
wire into the CLI on top of that. For this chunk, `resolve_single_root`
resolves the full configured scope, uses the first present root, and
prints a stderr note naming how many roots were left out and pointing
at `swamp scope`/`swamp observe`/`swamp schedule`. `Observe`/`Schedule`
(`roots: Vec<PathBuf>` already) got full multi-root scope resolution
now, since they already loop per root (`schedule::cmd_observe`) and
needed no walker/history changes to do so correctly.

**`LocationStatus::UnresolvedWithReason`.** Originally a tuple variant
(`UnresolvedWithReason(String)`) under `#[serde(tag = "state")]`
internal tagging. serde's internally tagged representation does not
support newtype variants wrapping a non-map type (`String`); this
silently produced an empty `scope.json` on persist (`to_string_pretty`
returning `Err`, swallowed by `.unwrap_or_default()`) and a swallowed
`None` on load. Caught by the `persisted_scope_round_trips` test, not
by inspection. Fixed by making it a struct variant
(`UnresolvedWithReason { reason: String }`), which internal tagging
supports. Lesson for the next detector author: struct variants, not
tuple/newtype variants, in any enum tagged this way.

## Adversarial tests written (not just happy path)

- `crates/core/src/scope.rs` (15 tests): defaults=false is
  explicit-only scope but leaves detectors enabled; a disabled detector
  does not hide a path reachable via another enabled parent root;
  exclusion wins over a builtin default and over an explicit
  command-line root; explicit roots replace defaults/detectors
  entirely; empty effective scope is its own explicit, distinguishable
  state (never falls back to cwd/home); `~/src` vs. an absolute path
  vs. a trailing slash all dedupe to one root retaining every reason;
  nested roots fold into their parent and the fold is retained as a
  reason; a nested *exclusion* is recorded as a pruned subtree; missing
  optional candidate paths remain explicit scope, not silently dropped;
  escaped/spaced paths round-trip; coverage-change notes report add/
  remove without asserting anything about bytes; a persisted scope
  round-trips through JSON.
- `crates/core/src/locations/` (12 tests): macOS proposes the three
  builtin defaults, Linux proposes none (no leaked macOS paths);
  Cargo/rustup env-var overrides win and are labelled, convention
  fallback still proposes leftovers with no executable present;
  Homebrew proposes both conventional prefixes even when no command
  runner is configured (and reports the failed tool query rather than
  swallowing it); an env override skips the tool query entirely (the
  fake runner records zero calls); the tool query only ever calls the
  allow-listed `brew --prefix`; a disallowed command is refused and
  recorded, never executed.
- `crates/core/src/growth.rs`: the `[scan]` table round-trips through
  `load_config_checked`; a wrongly-typed `[scan]` field
  (`defaults = "yes"`) is rejected by the checked loader while the
  infallible `load_config` still falls back for the pipeline's scalar
  reads; malformed TOML syntax is rejected.
- `crates/cli/tests/observe.rs`: `observe` with no roots now resolves
  the configured default scope (deterministic fixture: `HOME`
  overridden, non-builtin detectors disabled via config so the test
  never depends on real `/opt/homebrew`/`/usr/local`/`~/.cargo` on the
  machine running it) and persists `scope.json`; an empty effective
  scope fails visibly (`stderr` names it, nonzero exit) rather than
  silently falling back to cwd; invalid `[scan]` config fails visibly
  end to end through the real binary. The old
  `observe_needs_at_least_one_root` test asserted behavior this chunk
  intentionally changes (no roots is now valid -- it resolves configured
  scope) and was replaced by the three tests above.

## Measured cost

Scope resolution touches the filesystem exactly once per candidate root
(a `fs::metadata` presence/readability check) and runs zero external
processes unless a detector's tool query actually executes (only
Homebrew's `brew --prefix`, gated behind `HOMEBREW_PREFIX` being unset).
No walk, no Parquet I/O. Not separately benchmarked against a workload
baseline in this chunk; `scripts/check.sh`'s existing test suite ran in
single-digit seconds including the new tests.

## Follow-ups for later workers

- **#42**: consume `EffectiveScope.pruned_subtrees` in the walker so an
  `exclude` entry nested inside an in-scope root actually prunes that
  subtree during a walk, not just records it; make multi-root
  observation coherent per volume so #50 can make `report`/`ui`
  themselves resolve every present root instead of only the first.
- **#45-#49**: add the remaining detector families (mise/asdf/pyenv/uv/
  Conda; rbenv/RVM/ruby-install/chruby/nvm; npm/pnpm/Gradle/Maven/Go/
  Python caches; Xcode/Android; Hugging Face/Ollama/Docker) following
  the pattern in `docs/architecture.md`'s "Location detector registry"
  section and the one-test-per-detector style in
  `crates/core/src/locations/{cargo_home,rustup,homebrew}.rs`.
- **#50**: revisit whether `Report`/`Ui` should themselves become
  multi-root once #42 lands, replacing `resolve_single_root`'s
  first-present-root-plus-note behavior with real multi-root reporting;
  also revisit whether a *scheduled* run should re-resolve the
  configured scope on every fire instead of replaying the roots
  `schedule --every` resolved at install time (current behavior, same
  as an explicit root list always had).
- Revisit the `defaults = false` reading above if #45-#50's real usage,
  or the epic owner, disagrees with treating it as builtin-defaults-only.

## Correction, 2026-09-21 (later the same day)

The `defaults = false` decision recorded above -- that it turns off only
the `builtin-defaults` detector while every other enabled detector keeps
contributing roots -- **was rejected by the user.**

An independent review reproduced it as `defaults_false_must_mean_explicit_only`:
`defaults = false` with no `include` still resolved 111 candidate roots.
The review's objection is the one that matters, and it is not about
which reading is more useful:

> The implementation records a rationale and relabels the result
> explicit-only; a documented judgment call is not user approval to
> change the explicit scope contract.

That is right. Writing down a deviation makes it visible; it does not
make it agreed. The scope contract came from the user, and a session
note is not the place to amend it.

In force now (`crates/core/src/scope.rs`, and
`.oh/guardrails/explicit-only-scope-when-defaults-false.md`):

- `defaults = false` means swamp infers **nothing**. In scope are
  `include` entries, explicit command roots, and detectors the config
  names.
- `[scan] enabled_detectors = [...]` is the documented way to name
  them. A non-empty `disabled_detectors` counts as equally explicit
  curation ("run everything except these"), which is also what the
  reviewer's own fixtures rely on.
- With neither list set, the scope is empty and every command reports
  that, through the pre-existing empty-scope path.

Pinned by `scope.rs::tests::defaults_false_without_includes_or_enabled_detectors_is_empty`,
`..::defaults_false_with_an_enabled_detector_allow_list_runs_only_that_detector`,
and the reviewer's unchanged
`crates/core/tests/reviewer_counterexamples.rs::defaults_false_must_mean_explicit_only`.
