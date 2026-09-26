# Decision-evidence follow-ups: #56-#58, #60 (chunk H2)

## Aim

Close the named gaps chunk H's own session note
(`2026-09-21-decision-evidence.md`) and `FOLLOWUPS.md`'s "Chunk H note"
left open: live wiring of `toolchain_declarations.rs` (#56) and
`external_associations.rs` (#57) into the production report/external-
unit pipeline with a real per-worktree cache, Docker-specific recovery
in the generic per-row pass (#58), and the TUI/JSON presentation gaps
(#60) -- per `.oh/guardrails/computed-but-not-delivered.md`: "a
populated struct field nobody renders is a defect."

## What landed

- **`crates/core/src/consumer_wiring.rs`** (new, #56/#57 live wiring):
  the caller `toolchain_declarations.rs`/`external_associations.rs`
  were always missing. `attach_associations(report, external_units,
  swamp_dir)` reads each worktree's declaration/lockfile files (a
  small, fixed candidate-filename list per worktree), cached in two
  JSON sidecars under `${SWAMP_DIR}` keyed by each file's own mtime
  (`toolchain_declarations_cache.json`, `dependency_identities_cache.json`
  -- same shape as `external.rs`'s existing `external_consumers.json`
  precedent, never a new SQLite store or per-file inventory). A
  resolved match attaches a `consumer` fact both ways: the project's
  own `Source` row gets "this project declares X, resolved to
  installation/dependency Y" (reusing `toolchain_declarations::resolve`/
  `resolve_explicit`'s evidence shape); the installation/shared-store
  `ExternalUnit`
  gets "declared by project(s) [...]" (deduplicated, listed once per
  unit regardless of how many worktrees resolved to it).
- **Shared-store joins are hash lookups, not enumerations** (#57's own
  bounded-cost requirement, "index once per worktree per lockfile
  change; joins are hash lookups"): `external_associations.rs` gained
  `cargo_registry_entry_exists`/`go_module_cache_entry_exists`/
  `gradle_cache_entry_exists`/`maven_repo_entry_exists`, each a single
  `Path::exists()` check built from the parsed lockfile identity
  (never listing the shared store's contents, which can hold entries
  from projects entirely outside scanned scope). `go_module_escape`
  implements Go's own documented module-path escaping so a
  case-sensitive import path matches its on-disk, case-insensitive-safe
  directory name. npm's cacache (content-addressed, sha-keyed) cannot
  be resolved to a name+version at all -- stated as `unknown`, once per
  unit, not guessed; pnpm's store (also content-addressed) is
  attributed collectively per declaring worktree, the coarser
  granularity the store's own shape actually supports.
- **Maven `pom.xml` dependency parsing** (#57's other named gap):
  added the `roxmltree` dependency (0.21, MIT/Apache-2.0, actively
  maintained -- used by the resvg/usvg SVG toolchain) to
  `crates/core/Cargo.toml`. `external_associations::parse_pom_xml_with_gaps`
  reads direct `<dependencies>` (never `<dependencyManagement>`, which
  is a BOM/version declaration, not necessarily actually depended on)
  and reports any entry whose version is an unresolved Maven property
  (`${...}`) or inherited from an absent parent POM as a named gap,
  rather than fabricating a value this module cannot actually resolve.
- **`toolchain_declarations::resolve_rustup_channel_to_dir`**: rustup's
  installed-toolchain directories are named `<channel>-<host-triple>`
  (`stable-x86_64-apple-darwin`), but a `rust-toolchain(.toml)`
  declares only the channel. `match_version`'s own alias/range logic
  assumes a dotted-version convention (`3.12` -> `3.12.4`) and does not
  model this hyphenated one, so rather than bend that already-tested
  function, this widens the *installed* side: it returns the one
  installed directory whose channel prefix (split on the first hyphen
  only -- rustup channels never contain one) matches, when unambiguous,
  and stays unresolved (never guessed) when zero or more than one
  match. A new `toolchain_declarations::resolve_explicit` (bypassing
  `match_version`, `pub(crate)`-adjacent but exported for
  `consumer_wiring.rs`) builds the `ToolVersionAssociation` from that
  already-decided match. `match_version`/`resolve` themselves are
  untouched; every one of their existing tests still passes unmodified.
- **rustup's global default gets its own role** (#56's "global defaults
  as their own role" acceptance criterion): read from the already-
  discovered rustup `LocalState` unit's own `settings.toml` (no new
  `Environment`/env-var lookup -- the unit is already resolved by the
  same call), matched the same way, and attached to the `Installation`
  unit with a note distinguishing it from a per-project declaration.
  pyenv/rbenv/nvm/asdf/mise global defaults are not read: unlike
  rustup, `toolchain_declarations.rs` never had a global-default parser
  for them (only project-scope declarations), and inventing one for
  each convention was out of this chunk's bounded scope -- named below,
  not silently skipped.
- **Xcode DerivedData live wiring** (#57's other listed shared-store
  join): `external_associations::list_derived_data_subfolders` (one
  bounded `read_dir`) plus a real `plutil` read per subfolder's own
  `info.plist`, joined via the already-tested
  `xcode_derived_data_association` against every known project root.
- **Docker recovery wiring** (#58): `recovery::docker_image_recovery`
  (new) never asserts pull-vs-rebuild from a tag's shape alone -- it
  names both as candidate prerequisites and states which is more
  plausible from whether the row is joined to a known project, without
  picking one; `recovery::docker_build_cache_recovery` (new) requires
  the joined project's worktree to be present in this same report
  pass, mirroring `build_output_recovery`'s own source-presence logic;
  the pre-existing `docker_volume_recovery` is now actually wired in.
  `report::attach_decision_evidence`'s per-row match gained
  `DockerImage`/`DockerBuildCache`/`DockerVolume` arms; the *unjoined*
  branch (`join_docker_facts`'s `JoinOutcome::Unowned`) gets the same
  treatment via a new `UnownedRow::evidence` field -- "no project
  claims it" is a consumer fact, not a reason to skip an object's own
  recovery assessment.
- **A `RecoveryAssessment`'s own follow-up check now survives past
  attachment**: `attach_decision_evidence`/the Docker-unowned site
  previously took only `r.evidence` and threw away `r.follow_up_check`
  (the "smallest useful check" #58 requires). Both sites now carry it
  into the attached fact's `note` (`render_evidence_lines`/the TUI
  already render `note`), so it was not a rendering gap at all once
  traced -- it was silently dropped one step earlier, before rendering
  ever had a chance to show it.
- **TUI evidence rendering** (#60): `model::Row` gained an `evidence`
  field, populated at every row-builder that maps 1:1 to one
  `ArtifactRow`/`ExternalUnit`/`AgentUnit` (`kind_filtered_rows`
  (builds/deps), `docker_rows`, `external_rows`, `agent_rows`, the
  worktree-level row in `tree_rows_with_agents` (its own `Source` row's
  evidence -- the same row `consumer_wiring` attaches to), and the
  unfolded-artifact leaf row inside a worktree's tree). `ui::draw_body`
  renders the selected row's evidence below the existing signals line
  (`ui::ordered_evidence_lines`, reusing `render::render_evidence_lines`),
  ordered activity/consumer/current-use/recovery/reclaimability so a
  short terminal clips the least decision-relevant lines first; the
  detail area's height is sized from *estimated wrapped rows at the
  terminal's actual width* (not raw fact count -- a first attempt sized
  by fact count and silently clipped real content on an 80-column
  frame, since one logical fact line can wrap to several physical
  rows), capped at half the body height so a unit with many facts can
  never push the row table off screen. `app::mark_row` now also folds
  `render::evidence_warnings(&row.evidence)` into the same `warnings`
  vec the confirm banner already renders -- a new, deliberately
  *selective* function (a declared consumer, current use, or an
  uncertain recovery/reclaimability fact; never every fact restated as
  a warning, and never nagging on a routine "known rebuildable, no
  consumers" unit).
- **JSON views carry evidence** (#60): `agent_json::view_payload`'s
  `kinds`/`builds`/`deps`/`unowned` arms and `list_worktrees_payload`
  now include `evidence` per row (a `kinds` bucket aggregates many
  artifact rows into one, so its `evidence` is the concatenation of all
  of theirs; a `worktrees` row carries its own `Source` row's evidence).
  `docker_objects_payload` (`--view docker`, not in the original named
  list but the same kind of bespoke-shaped gap, and exactly where a
  Docker recovery fact from this same chunk would otherwise be
  invisible in JSON) got the same treatment.

## Correction (2026-09-21, chunk H3)

Three function names this note used no longer exist. The behaviour it
describes is still delivered; the names were wrong, and a claim naming a
function nobody calls is worse than no claim
(`.oh/guardrails/no-dead-public-evidence-api.md`):

- `external_associations::join_cache_entry` is **deleted**. It joined one
  cache entry's identity to the projects declaring it, but
  `consumer_wiring::attach_associations` aggregates consumers per *unit*
  rather than per entry and never went through it, so it had no
  production caller. The consumer evidence described above is real; it is
  built in `attach_associations` itself. A per-entry join, if wanted
  later, belongs in `consumer_wiring.rs` beside `match_identity`.
- `external_associations::parse_pom_xml` is **deleted** in favour of
  `parse_pom_xml_with_gaps`, the same parse plus the named gaps. The thin
  wrapper dropped exactly the gaps the PR #123 review required be stated,
  and nothing called it.
- `toolchain_declarations::conflicting_tools` is **deleted** for the same
  reason: it named tools declared at two different versions within one
  project, and nothing surfaced its result. #56's "conflicting
  declarations remain explicit" is delivered by
  `VersionMatch::Conflicting` -> `FactStatus::Conflicting` through
  `declaration_evidence`, which is wired. Re-adding the
  two-files-disagree check belongs in `consumer_wiring.rs`, the only
  caller that holds a worktree's whole declaration list at once.

## Decisions that needed to be made explicitly

**The general TUI mark-row path does not call `actions::propose` at
all for an ordinary artifact row.** Discovered while wiring the
confirm-row warnings: `app::mark_row` only calls
`actions::propose`/`propose_agents` for a Cargo-nested-group member or
an agent-storage unit; a plain top-level `BuildOutput`/`DependencyTree`/
`Cache`/Docker row builds its `MarkedUnit` directly from `Row` fields,
bypassing `PlanUnit`/`PlanUnit.evidence` entirely (that only happens
later, in the CLI's `propose` command, or the TUI's cargo/agent-only
branches). Rather than force every row through a fresh `propose()` call
at mark-time (a real behavior change to a well-tested interactive
path, for uncertain benefit -- `plan_unit_evidence` only adds one fresh
occupancy check on top of the same `a.evidence` already on the row),
this reads `row.evidence` directly (already wired for every row kind
above) once, uniformly, right where the existing git-status warnings
are built. This covers every markable row the same way and avoids
double-counting the Cargo/agent cases (which would otherwise also
receive the same facts a second time via their own `propose()` call's
`u.warnings`, producing a duplicate line).

**Global default reading is scoped to rustup, not "every manager".**
`toolchain_declarations.rs` itself only ever had a global-default
*parser* for rustup (`parse_rustup_default_toolchain`); pyenv's
`$PYENV_ROOT/version`, rbenv's `$RBENV_ROOT/version`, asdf's legacy
`~/.tool-versions`, mise's `~/.config/mise/config.toml`, and nvm's
`$NVM_DIR/alias/default` are all real, documented conventions this
chunk could have added parsers for, but doing so for five more
managers inside an already-large chunk was not a bounded addition --
named below as a real, not-silently-dropped gap, not implemented as a
guess.

**`uv`/Conda project-level declarations remain unimplemented**, exactly
as chunk H's own session note already scoped: #56's acceptance
criteria say "include uv/Conda environment references *where source
metadata establishes them*" -- conditional, and no parser for either
exists in `toolchain_declarations.rs`. Wiring cannot supply a source
the library does not have; adding those parsers was out of this
chunk's scope (it is squarely #56/#57 follow-up work, not #60
presentation work).

## What was verified, not assumed

- `cargo build --workspace --all-targets` and
  `cargo test --workspace --locked` (both against the shared warm
  target dir) stayed green throughout -- the full run at the end of
  this chunk: 41 test binaries, 0 failures.
- `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets --locked -- -D warnings` both
  clean.
- `cargo run -p swamp-source-audit` passes, including
  `agent_interface_facts_not_verdicts` (the new `evidence_warnings`
  strings were checked against the verdict-vocabulary scanner) and
  `all_report_paths_through_bus`/`static_registration_only`/
  `extractors_are_pluggable` (confirms `consumer_wiring.rs`, which is
  deliberately *not* a registered `Consumer`, does not trip the bus
  audit -- it runs as a post-pass at CLI/TUI call sites that already
  have both a `Report` and a `Vec<ExternalUnit>` in hand, the same
  shape `report::attach_decision_evidence` already established for
  `bus::run_report`).
- `crates/core/tests/consumer_wiring.rs`-equivalent coverage lives as
  unit tests inside `consumer_wiring.rs` itself (5 tests): a pinned
  `rust-toolchain` resolving to a real rustup toolchain directory and
  attaching evidence both ways; the per-worktree cache genuinely
  reusing a parsed result when a file is unchanged and genuinely
  re-parsing after a real mtime change (a real `tempfile` write, a real
  1.1s sleep, a real second write -- not a mocked clock); two projects
  sharing one Cargo dependency producing a `Known(List)` fact naming
  both; npm's cacache producing `unknown`, never a guessed match.
- `crates/core/tests/evidence_contract.rs::joined_docker_rows_carry_their_own_recovery_evidence`
  is a genuine adversarial test: it asserts a *specific* Docker row
  (from the fixture's real Docker join) carries a `Recovery` fact, not
  just "some row somewhere in the report has one" (which the
  pre-existing `fixture_report_carries_at_least_one_real_fact_of_each_kind`
  could already pass from an unrelated `BuildOutput` row without this
  chunk's Docker wiring existing at all).
- `crates/tui/tests/frames.rs::evidence_detail_area_frames` and
  `::confirm_row_shows_evidence_warnings` are real ratatui `TestBackend`
  captures at both 80x24 and 200x60 with a fixture row carrying a
  multi-consumer `Known(List)` fact and an `Unknown` recovery fact
  (missing evidence / no resolved consumer), asserting both project
  names and the word "unknown" are actually visible on screen, not just
  present in the underlying data -- and committed golden snapshots
  (`evidence_detail_{80x24,200x60}.txt`, `evidence_confirm_{80x24,200x60}.txt`)
  for regression review.
- `crates/cli/tests/agent_json_contract.rs::bespoke_json_views_carry_evidence_on_their_rows`
  drives the real built binary against a disposable git fixture with
  both a `Cargo.toml`/`target/` (build output) and `node_modules`
  (dependency tree), and asserts every row of `--view builds`/`deps`/
  `kinds`/`worktrees` carries a non-empty `evidence` array -- not just
  that the JSON key exists.

## Measured cost

Not independently re-measured this chunk (the brief's own #62 cost-
validation chunk is the dedicated place for a large-tree measurement).
`consumer_wiring`'s own cost shape is bounded by construction, not
measured against a real `~/src` tree here: a fixed, small candidate-
filename list's `fs::metadata` per worktree per report (a cheap stat,
not a walk), and one `Path::exists()` per declared dependency identity
against its ecosystem's shared store (never an enumeration of the
store). Chunk H's own measurement (~15.8ms -> ~16.2ms avg over 10 runs
on the disposable fixture, before/after `attach_decision_evidence`
alone) is unaffected by this chunk's additions, since `consumer_wiring`
is a separate call at the CLI/TUI call sites, not inside
`bus::run_report`.

## Commits (integration/full-scope)

See `git log` for exact hashes and full messages; in order:

1. Add live wiring of tool-version/dependency associations
   (`consumer_wiring.rs`, `roxmltree` dependency, `pom.xml` parsing,
   shared-store existence-check joins, rustup channel/global-default
   resolution) (#56, #57)
2. Wire Docker image/build-cache/volume recovery into the generic
   per-row evidence pass, joined and unjoined alike; carry a recovery
   assessment's follow-up check into its attached fact's `note` (#58)
3. Render decision evidence in the TUI detail area and confirm row;
   carry `evidence` through `model::Row` (#60)
4. Carry `evidence` through the bespoke-shaped JSON views (#60)
5. Docs, session note, CHANGELOG (this commit)

## Follow-ups for #62/#63 validation or a dedicated later chunk

- `uv`/Conda project-level toolchain declarations: no parser exists in
  `toolchain_declarations.rs`; #56's own acceptance criteria make this
  conditional ("where source metadata establishes them"), not a
  guaranteed feature.
- pyenv/rbenv/nvm/asdf/mise global-default files are real, documented,
  and not read -- only rustup's `settings.toml` is, matching what the
  library already had a parser for. Adding the other five is
  mechanical (the same single-value/`.tool-versions`/mise.toml parsers
  already used for project scope) but untouched here.
- A custom (non-default) `GOMODCACHE` is identified via its documented
  `cache/download` sibling's grandparent, not guessed from a path
  shape -- but an unconventional layout that does not follow Go's own
  convention could still miss. Same caveat for pnpm's per-volume store
  (already a named `docs/locations.md` gap from an earlier chunk,
  unrelated to this one).
- npm's cacache and pnpm's content-addressed store cannot be resolved
  to a specific declared name+version; this chunk states that limit
  explicitly (per #57's own acceptance criteria) rather than treating
  it as solved.
- `consumer_wiring`'s per-worktree cache is a new JSON sidecar file
  pair; it is never read/written by anything except this module, and
  is not yet covered by `scripts/check.sh`'s own file-shape audits
  (none currently apply to sidecar caches generally -- same as
  `external_consumers.json` before it).
- Large-tree cost re-measurement (#62's own job): this chunk's cost
  claims are architectural (bounded by construction), not measured
  against a realistic `~/src`-sized tree with many worktrees and a
  populated Cargo registry/Go module cache.
