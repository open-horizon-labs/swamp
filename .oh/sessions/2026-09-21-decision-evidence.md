# Decision-evidence layer: #53-#61 (chunk H)

## Aim

Implement the current-state decision-evidence contract (#53) and its
five domain populators -- activity (#54), live-use/occupancy (#55),
tool-version associations (#56), external build-output/dependency
associations (#57), recovery evidence (#58), reclaimability (#59) --
plus presentation (#60) and action-layer integration (#61), per
`.oh/handoffs/2026-09-21-claude-full-scope.md` section 3, as one
worker chunk (`chunk H`) in the `integration/full-scope` sequential
chain. Evidence must live in current-state storage/report caches, never
in the byte-history Parquet store, and refreshing it must never
manufacture a byte-history delta.

## What landed

- **`crates/core/src/evidence.rs`** (#53): the shared `Evidence`
  contract -- `FactKind` (Activity/Consumer/CurrentUse/Recovery/
  Reclaimability), `FactSubtype`, `FactStatus`
  (Known/Unknown/Unavailable/Conflicting, each with a stated reason),
  `EvidenceSource` (nine provenance variants down to a specific tool/
  path), `observed_at`/`event_at`, and `Freshness` (expiry and/or a
  coverage-limit note). Attached as `evidence: Vec<Evidence>` on
  `ArtifactRow`, `ExternalUnit`, `AgentUnit`, and
  `NestedArtifact::decision_evidence` (kept distinct from that
  struct's existing narrower `ArtifactEvidence` producer/consumer
  fields, which belong to the independent #64-#74 build-artifact
  epic).
- **`activity.rs`** (#54): folded modification age from the walk's
  existing `mtime_max` (labelled "newest recorded modification among
  measured children", never "last used"); access-time reliability
  detection (macOS `statfs` `MNT_NOATIME` flag, Linux `/proc/mounts`
  option parsing) before ever trusting an atime read; Docker's own
  `last_used` normalized as a distinct tool-reported fact, never
  flattened into filesystem mtime. A documented inventory (also in
  `docs/usage.md`) of which domains have real activity evidence versus
  must report unknown.
- **`occupancy.rs` extensions** (#55): structured current-use evidence
  alongside the pre-existing `occupied()` bool -- `lsof` open-file
  match with permission-denied distinguished from no-match, Docker
  running-container relationships from already-collected
  `ContainerRef`s, a non-blocking `flock` probe for manager lock files
  (acquired and immediately released, never held, never writes), and
  simulator "booted" state via the bounded, allow-listed `xcrun simctl
  list devices -j` query (new `locations::ALLOWED_COMMANDS` entry).
  Every current-use fact expires after 60s.
- **`toolchain_declarations.rs`** (#56): read-only parsers for
  `.tool-versions`/`mise.toml`, `.python-version`, `.ruby-version`,
  `.nvmrc`/`.node-version`, `rust-toolchain(.toml)`, and rustup's
  global `default_toolchain`. Matches a declared version against
  measured installations with manager semantics -- an alias/range
  (`lts/*`, a bare `3.12`) stays an explicit unresolved range unless
  exactly one installation uniquely matches; conflicting declarations
  within one project are named, not silently resolved.
- **`external_associations.rs`** (#57): Xcode DerivedData
  `info.plist`'s `WorkspacePath` (read via the bounded, read-only,
  output-only `plutil -convert xml1 -o -`, never a bespoke binary-plist
  parser) joined against known project roots -- never guessed from the
  DerivedData folder's own basename+hash. Dependency-lockfile identity
  parsers for Cargo.lock, package-lock.json, pnpm-lock.yaml (a narrow
  line scan of its documented `packages:` shape, mirroring the existing
  `.condarc` precedent, not a general YAML parser), go.sum and
  gradle.lockfile, joined by exact name+version. `docker_join_evidence`
  normalizes the existing Docker join decision into the same contract.
- **`recovery.rs`** (#58): `RecoveryAssessment` replaces the blanket
  per-kind label with sourced, per-unit evidence -- Rebuild (source
  present), NetworkFetch (lockfile present; network/credential
  reachability always stated as a material unknown, never asserted),
  LocalReinstall (known toolchain version), PotentiallyUniqueLocalState
  (mutable simulator/emulator/Docker-volume state), Unknown. Maven's
  local repository gets the `_remote.repositories`-marker treatment
  named in `docs/locations.md`'s known gap. Every assessment carries a
  concrete smallest-useful follow-up check; never a fabricated cost or
  an assumed backup.
- **`reclaimability.rs`** (#59): logical vs. allocated vs.
  estimated-reclaimable (`Known`/`Bounded`/`Unknown`, bounded rather
  than exact for APFS clones/snapshots and unresolved hardlink
  membership) vs. observed post-action free-space change (a real
  `statvfs`/`df` reading before/after, already partly wired via
  `actions::free_space_bytes`). `estimate_selection` reconciles a
  selection set's shared inodes so the same physical storage is never
  summed twice. `DockerByteAccounting` keeps the daemon's logical
  object accounting structurally separate from the host VM
  backing-file allocation.
- **Live wiring, not just library code** (#53's "populated end-to-end"
  requirement): `report::attach_decision_evidence`, called exactly
  once from `bus::run_report` (the single choke point every report
  caller -- CLI text/JSON, TUI, single- and multi-root -- goes
  through), attaches Activity/Reclaimability/Recovery to every
  `ArtifactRow`. The Docker-join site in `report.rs` and
  `external::discover_and_measure` attach Consumer facts from data
  already collected. `agents::discover_and_measure` attaches Activity
  from each unit's already-recorded `mtime_max`. CurrentUse is
  deliberately *not* attached during a passive report (a live check is
  short-lived and only meaningful right before an action):
  `actions::plan_unit_evidence` takes it fresh at proposal time, and
  `execute_with_trash_opts`'s generic path takes it fresh *again*
  immediately before acting -- an occupancy change between propose and
  execute is refused with a named cause, never silently missed.
- **Presentation** (#60): `render::render_evidence_lines` (one
  source-qualified line per fact, reusing `age_label`'s existing
  correct unknown/future-timestamp handling), wired into
  `render_view_external`'s CLI text output. CLI JSON: the default
  report view, `--view external`, and `--view agents` already pass
  whole structs through serde, so `evidence` appears there with zero
  extra shaping code -- confirmed by a real end-to-end test driving the
  built binary
  (`crates/cli/tests/agent_json_contract.rs::report_json_carries_decision_evidence_on_artifact_rows`).
  The bespoke-shaped views (`kinds`/`builds`/`deps`/`unowned`/
  `worktrees`) do not yet carry it -- named, not silent.
- **Actions carry evidence** (#61): `actions::PlanUnit.evidence`
  (report-row facts plus a fresh current-use reading at proposal time),
  wired through `unit_from_row`/`unit_from_external`/`unit_from_agent`
  (and, through `unit_from_row`, `unit_from_worktree`/`unit_from_dir`).
  Every `ExternalUnit`-derived `PlanUnit` carries `external_category`
  unconditionally regardless of which `StorageCategory` a future
  detector adds, so a new unit kind can never silently acquire generic
  deletion capability -- made an explicit table-driven test across the
  full `StorageCategory` enum.
- **Human protect extended beyond agent-storage units** (#60/#61):
  `actions::propose_checking_protection` refuses (naming the cause in
  `plan.refused`) a proposal that names a human-protected ordinary
  filesystem artifact row, wired into the CLI's `propose_unified`. Only
  `agents::protect_add`/`protect_remove` (reachable only from the CLI's
  own `protect` subcommand) can change the underlying list.

## Decisions that needed to be made explicitly

**CurrentUse is not attached during a passive report.** A live
process/lock/container check is meaningful only right before an
action, and eagerly `lsof`-checking every artifact row on every
`report` call would be real, avoidable cost for no decision benefit
most of the time. Instead it is taken fresh exactly at the two moments
it actually matters: proposal time (`plan_unit_evidence`) and
immediately before `execute` acts. This also satisfies #55's own
"short-lived facts expire and are rechecked at existing action
boundaries" language more literally than eager per-row attachment
would.

**Consumer evidence in the live pipeline reuses existing data rather
than performing new discovery on every report.** Full lockfile
discovery across every project, matched against every shared-cache
entry, on every `report` call would violate "index lockfile identities
once per worktree... never walk caches per project" (#57's own
bounded-cost requirement) without a real caching design this chunk did
not have time to build. `external_associations.rs`'s
lockfile/Xcode/Docker-join populators are complete, real, and unit-
tested; the *live* wiring into `report_scope`/`external::
discover_and_measure` reuses the Docker-join site's already-computed
decision and `ExternalUnit`'s existing manual consumer-association
sidecar. Wiring the lockfile/toolchain-declaration populators into the
live per-project/per-root pipeline (with real caching) is named as a
follow-up below, not claimed done.

**Recovery evidence in the live per-row pass only covers what a single
row can honestly source without extra context.** `BuildOutput` gets
`Rebuild` from worktree presence (the worktree is in this report, so
its source is present this pass); `DependencyTree` gets a real
lockfile-existence check at the worktree root (bounded: a handful of
`Path::exists()` calls, not a new per-file scan); `Cache` gets an
honest `Unknown` (no lockfile/source signal available at this generic
pass). Docker-specific recovery (volumes, images) is not attached in
the generic per-row pass; `recovery::docker_volume_recovery` exists and
is tested, but wiring it requires the Docker-specific context (volume
name, container references) that isn't available in the generic loop
-- named as a follow-up.

**Maven and `pom.xml` dependency parsing are out of scope, matching the
existing named gap in `docs/locations.md`.** No XML parser is a
workspace dependency; adding one for this alone was not justified
within this chunk's bounded scope. Maven association still relies on
the `_remote.repositories` marker (#58's own treatment), which is
real and tested.

## What was verified, not assumed

- Ran the full workspace test suite (`cargo test --workspace --locked`)
  after every commit in this chunk; it stayed green throughout (453
  `swamp-core` lib tests at the first evidence commit, growing to
  cover every new module, plus the existing cli/tui integration
  suites).
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all --check` clean after every commit.
- `cargo run -p swamp-source-audit` (including
  `agent_interface_facts_not_verdicts`, which parses `render.rs`,
  `agent_json.rs`, and every file under `crates/cli/src`/
  `crates/tui/src` for verdict vocabulary) passes with the new
  `render_evidence_lines` output.
- The occupancy-recheck adversarial test
  (`crates/core/tests/evidence_action_recheck.rs::occupancy_change_between_propose_and_execute_refuses_execution`)
  actually opens a real file handle on the exact unit path between
  `propose` and `execute` (the same real-`lsof` mechanism the existing
  agent-storage test uses, no mocking) and confirms `execute` refuses
  it -- this is a genuine adversarial check, not a happy-path test with
  the mechanism merely present.
- `crates/core/tests/evidence_contract.rs::refreshing_evidence_never_writes_byte_history_delta`
  runs three real observations against a disposable fixture with
  nothing changed on disk and asserts byte-identical
  `bytes`/`growth_bytes`/`regrowth_count` (past the first observation's
  one-time `None -> Some(0)` growth-baseline transition), plus a direct
  check that calling `attach_decision_evidence` again on an
  already-annotated report changes nothing.

## Measured cost

Ten repeated `report_full_mode` calls against the same disposable
git-fixture tree (`crates/core/tests/fixture::build`, ~9 artifact
rows across two projects, Docker facts included) with nothing changed
on disk between calls, before and after this chunk's
`attach_decision_evidence` wiring in `bus::run_report`:

- **Before** (evidence attachment commented out):
  avg 15.79ms, min 14.90ms, max 16.40ms over 10 runs.
- **After** (evidence attachment active): avg 16.17ms, min 15.05ms,
  max 18.58ms over 10 runs.

The difference (~0.4ms average, within the run-to-run noise band on
this small fixture) is consistent with the design intent: evidence
attachment is O(rows) pure arithmetic over numbers the walk/measurement
pass already produced, with no new filesystem walk and no new
per-file I/O. This is a small-fixture measurement, not a claim about a
large real `~/src` tree; #62's own cost-validation chunk should
re-measure against a realistic tree and the full detector catalog.

## Commits (integration/full-scope)

In order (see `git log` for exact hashes):

1. `Add the current-state evidence contract, activity and occupancy evidence (#53, #54, #55)`
2. `Add artifact-specific recovery evidence and reclaimability estimates (#58, #59)`
3. `Associate tool versions and external build output/dependencies with projects (#56, #57)`
4. `Wire decision evidence into the live report pipeline end to end (#53, #54, #57, #58, #59)`
5. `Carry decision evidence through action proposal and execution (#61)`
6. `Present evidence in CLI text/JSON and extend human protect to ordinary units (#60)`
7. `Add end-to-end evidence-contract acceptance tests (#53)`
8. This session note plus doc updates (usage.md, architecture.md, DESIGN.md, skills/swamp/references/evidence.md, CHANGELOG.md).

## Follow-ups for the next worker / #62-#63 validation

- Live wiring of `toolchain_declarations.rs`/`external_associations.rs`'s
  lockfile-join populators into the production report/external-unit
  pipeline (today they are complete, real, unit-tested modules reached
  from the live path only via the Docker-join site and the existing
  manual consumer sidecar). A real per-worktree lockfile cache (keyed
  by lockfile mtime, per #57's own bounded-cost requirement) is the
  right shape; this chunk did not have time to design and land it
  alongside everything else.
- TUI detail-area rendering of `PlanUnit.evidence`/row evidence is not
  wired (`crates/tui/src/ui.rs`); the CLI text/JSON contract is the
  currently-exercised presentation surface. Named in `DESIGN.md`.
- The bespoke-shaped JSON views (`kinds`/`builds`/`deps`/`unowned`/
  `worktrees` in `agent_json.rs`) do not carry `evidence`; only the
  default report view and `--view external`/`--view agents` do (they
  pass whole structs through serde). Extending the others is
  mechanical but untouched here.
- Docker-specific recovery (`recovery::docker_volume_recovery`) is
  implemented and tested but not wired into the generic per-row
  `attach_decision_evidence` pass, which lacks the Docker-specific
  context (volume name, container list) at that point in `report.rs`.
- Maven/`pom.xml` dependency parsing remains a named gap (no XML parser
  dependency in this workspace); Maven association still relies on the
  `_remote.repositories` marker.
- `docs/architecture.md`'s new "Decision evidence contract" section
  lists these same gaps; keep both in sync if either changes.
