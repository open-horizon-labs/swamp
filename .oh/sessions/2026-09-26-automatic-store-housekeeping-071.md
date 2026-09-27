# Automatic store housekeeping — Swamp 0.7.1

## Aim

After ordinary complete observations, users should find only the active Swamp
store formats and state. They should not need a reset command, manual cleanup,
or a migration workflow. Preserve `config.toml`, `protect.parquet`,
`notes.parquet`, current Parquet facts, compatible history, and active
`enrich.parquet`; discard recognized retired derived payloads and let observation
rebuild what it needs.

Branch: `fix/071-automatic-store-cleanup`, based on `origin/main` at
`17c4fe325a75de01a6a57e0d62fa277dc37ffd63`. Work was confined to
`/private/tmp/swamp-fix-full-check`.

## Decision and boundaries

Use one fixed `housekeeping.version` marker. After a complete successful
observation, a fixed-name allowlist removes retired root-level and numeric
volume-level payloads (`unowned.json`, `fsevents.json`, `topology.json`,
`docker_facts.json`, `last_run.json`, `grants.json`, `ledger.jsonl`, and valid
16-hex-key `last_report-*.json[.zst]`) plus UUID-named JSON payloads directly
inside the retired `plans/` directory. The marker is written atomically only
after cleanup succeeds. Current stores pay a marker metadata/read check and do
not enumerate the store again.

All observing writers share an advisory store lock from before persistence until
after housekeeping. The CLI's PID lock remains for its existing user-facing
single-flight behavior. Incomplete, inaccessible, missing, empty, partial-family,
or errored observations do not run housekeeping. Report remains a pure read.

Do not prune numeric volume directories or root-keyed current tables based on
the invocation's scope. There is no authoritative complete ownership record
covering every earlier scope, detector location, and explicit-root observation;
the current scope cannot prove that an omitted root is orphaned. Recognized
retired files and plans are genuine orphan formats; broader root-generation
pruning remains out of this patch until ownership can be proved without a second
index or scope-widening assumption.

## Dissent

**Decision under review:** automatically delete retired files during ordinary
observation. **Stakes:** user data could be deleted from an overridden
`SWAMP_DIR`, or concurrent writers could lose store state. **Confidence before:**
medium.

### Steel-man

The store has a finite set of retired Swamp filenames. A one-time marker avoids
repeated scans; exact locations and filename shapes bound deletion; a shared
writer lock prevents observation races. Complete successful observations are
the natural point at which derived state can be refreshed.

### Contrary evidence and pre-mortem

1. **Functional failure:** an explicit-root observation mistakes all omitted
   volume data for an orphan. The cleanup never prunes volume directories or
   current tables; a two-root history fixture verifies a narrow observation
   preserves other numeric-volume state and the prior two-root report.
2. **Data-loss failure:** a recognized filename is a symlink to user data or a
   lock path is redirected outside the store. Cleanup only unlinks the legacy
   symlink itself; it never descends through links. Lock opens use `O_NOFOLLOW`
   on Unix, and fixtures verify the outside sentinel survives.
3. **Upgrade failure:** an incompatible marker repeats cleanup every run, or a
   partial observation marks cleanup complete. Tests verify incompatible
   markers trigger one pass, current markers skip it, and a missing-root
   observation leaves both the old marker and candidate file untouched.
4. **Adoption failure:** housekeeping runs but leaves the old reports/plans that
   motivated the patch. Fixtures cover legacy reports, JSON names, and UUID
   plans; normal `observe` cleans them automatically.
5. **Opportunity cost:** broad orphan pruning adds a parallel ownership index or
   causes an unrelated rescan. No index or rescan was added; broad root pruning
   is explicitly deferred because current observations cannot prove global
   liveness.

### Hidden assumptions checked

| Assumption | Evidence | Risk if wrong | Check |
|---|---|---|---|
| These exact names are retired Swamp-owned payloads | Historical formats and allowlist names; names are fixed or UUID/key validated | A colliding user file in an overridden store is removed | Exact path/name allowlist; unrelated names and malformed report keys survive fixture cleanup |
| Current state is not retired by format cleanup | Current Parquet tables remain allowlisted; active enrichment is live by source audit | Compatible history or enrichment disappears | Unit and normal-observe fixtures preserve current data, notes, protection, config, history, and `enrich.parquet` |
| Observation writers serialize | Every normal observer uses `report::observe_scope` | TUI and CLI race cleanup against writes | Shared lock holder test; lock acquired at the common core entry point |
| Cleanup need not repeat on current stores | The version marker changes only after successful cleanup | Ongoing full-store traversal slows every observe | Marker fast path returns before `read_dir`; repeat observe is verified incremental |

### Decision

**Recommendation: PROCEED.** The remaining concern is the unavoidable naming
convention boundary for retired files in a user-overridden store. The exact
legacy paths are Swamp's former store schema; the implementation refuses broad
recursive removal, follows no symlinks, and preserves all unrecognized content.
The parent requested an independent destructive-boundary review before merge;
that review remains a release gate.

## Execute

**Success criteria:** normal successful observation performs the cleanup once;
report is unchanged; compatible scope/history survives; concurrent writers
serialize; no user sentinels are followed or removed; v0.7.1 release metadata and
minimal docs are updated.

### Risk retirement

| Risk | Status | Evidence |
|---|---|---|
| Narrow-root cleanup deletes all noncurrent roots | Retired | CLI fixture upgrades from an incompatible marker during a one-root observe and preserves numeric-volume history and the previous two-root report |
| Config, protection, or notes are removed | Retired | Config/protection bytes and notes table survive the end-to-end upgrade fixture |
| Symlink traversal reaches outside data | Retired | Store cleanup unlinks the link itself; unit sentinel survives. Unix lock symlink is refused with sentinel unchanged |
| Failed/incomplete observation cleans the store | Retired | Missing-root CLI fixture leaves legacy candidate and old marker intact; error paths return before housekeeping |
| Concurrent observation races deletion | Retired | Shared advisory lock serializes holders; every observing pipeline acquires it before writes |
| Incompatible marker repeats cleanup | Retired | Unit test verifies marker version 0 triggers one pass and version 1 skips later passes |
| Live `enrich.parquet` is deleted by directory naming | Retired | Unit and CLI fixtures verify enrichment is preserved in numeric volume directories |
| Old report/plan/JSON files remain | Retired | Unit and CLI upgrade fixtures seed and verify removal of each recognized legacy family |
| Unchanged current stores incur repeated enumeration | Retired | Current marker exits before store `read_dir`; end-to-end repeat observation remains incremental |
| Orphan root state is removed from incomplete scope | Accepted boundary | No root directory/current table pruning is attempted. Current scope is not authoritative across earlier explicit and detector scopes; broader pruning needs a separately reviewed ownership source |

### Verification

- `cargo test -p swamp-core housekeeping_tests --locked` — the three focused
  cases pass as part of the routine suite (payload boundaries/idempotence,
  concurrent writers, and symlinked lock/marker protection).
- Focused CLI upgrade test — passes, including previous-scope report and
  incremental repeat observation.
- Missing-root deferral test — passes.
- `cargo test -p swamp-core --test store_contents_are_allowlisted --locked` —
  passes.
- `cargo run -q --locked -p swamp-source-audit` — 12/12 audits pass.
- `bash scripts/check.sh` — passed at 20:33:42 local: format, strict Clippy,
  audits, release graph, workspace tests, named-target checks, and greps.
- Repository `sg review` was not available on PATH.

### Remaining review and release gates

- Parent's independent destructive-boundary review before merge.
- Push branch and open PR; CI on GitHub remains to run.
- No merge, tag, publish, or mutation of the live `~/.local/share/swamp` store.
- Native Linux behavior has not been run locally in this macOS checkout; Linux
  CI remains relevant for `O_NOFOLLOW` and lock behavior.
