# 2026-09-24 — The bounded fold's queued undercount bug (stack/25)

Follow-up named directly by `.oh/sessions/2026-09-23-build-adapters-on-gates.md`
§4: `folded_measurement::folded_bytes_bounded_stamped` (the agent
family's bounded per-session/per-category fold, reached through
`agents::IdentifyCtx::folded_bytes`) had the identical `let Ok(rd) =
crate::fs_gate::read_dir(&dir) else { continue };` shape that session's
own bug lived in for `measure`/`resize_artifact_stamped` -- an
unreadable subdirectory a few levels inside a unit (not the unit's own
root, which the caller already probes) made the fold silently sum only
what it could read, with **no completeness signal at all**: only
`truncated` (hitting `max_entries`) was tracked, and even that was
never protected from growth history. Worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`, branch
`stack/25-bounded-fold-completeness` off `stack/27-report-only`.

## 1. Product-change note

The brief this chunk started from assumed a CLI action path
(`crates/core/src/actions.rs`) still existed. It does not any more:
`actions.rs` is TUI-only (`PlanUnit`/`trash_agent_cache`/
`trash_agent_session`, called from `crates/tui`'s worker path). Its two
`crate::agents::folded_bytes` calls (sizing a cache directory or a
session's members immediately before moving them to Trash) are a
right-now, in-memory measurement for the move that is about to happen,
never a stored baseline -- not the bug this chunk investigates.

## 2. Investigation: is this bounded fold's total persisted as history?

Yes. `agents::discover_and_measure_in` (the production entry point,
reached from `crate::report`'s pipeline) builds one `AgentUnit` per
adapter-identified `CandidateAgentUnit`, and for *every* home-level and
project-local candidate it pushes a `crate::growth::ObservedExternal {
bytes: cand.bytes(), .. }` into `observed`, then calls
`crate::growth::observe_and_annotate_external` -- the exact same
growth/regrowth store `external.rs` uses (same `ExternalHistory`
table, same `external_row_key` format; `agents::unit_key` calls it
directly). Before this chunk, the call passed a hardcoded
`&HashSet::new()` for `protected_keys`, so nothing ever protected an
agent unit's history the way `external.rs`'s own `protected_keys` set
does. `cand.bytes()` for the great majority of adapters comes straight
from one or more `ctx.folded_bytes(...)` calls (`IdentifyCtx::
folded_bytes`, which delegates to the buggy function). So: a `chmod
000` subdirectory inside, say, Claude Code's `shell-snapshots/` would
have reported a smaller "complete" total, and restoring access on a
later pass would have read as regrowth -- the same coverage-change-as-
storage-change bug `.oh/guardrails/coverage-changes-are-not-storage-
changes.md` forbids, one call away from the CARGO_HOME case the prior
session fixed.

Container-reuse rows (`ContainerCache`/`container_with_facts`) were
**already** safe from this specific bug, incidentally: `IdentifyCtx::
folded_bytes` already calls `abandon_container_recording()` whenever
`truncated` is true, which marks the whole container `unstorable` and
skips writing its cache row -- that machinery just never got the
unreadable-subdirectory case as an input to `truncated` in the first
place (the fix below closes exactly that gap, so the existing
container-cache protection now covers it too, for free).

## 3. The fix

**Root cause** (`crates/core/src/folded_measurement.rs`,
`folded_bytes_bounded_stamped`): the `let Ok(rd) = ... else { continue
}` arm now sets `truncated = true` before `continue`ing, exactly
mirroring how hitting `max_entries` is already reported. `truncated`
was already documented as this function's one incompleteness signal
("reported truncated rather than silently under-measured") -- the
unreadable-subdirectory branch had simply never routed into it.

**Completeness threaded through the candidate/unit types**
(`crates/core/src/agents/unit.rs`): `CandidateAgentUnit` gained a
`complete: bool` field (default `true`), an accessor, and
`AgentUnitBuilder::incomplete(reason)` -- sets `complete = false` and
appends `reason` to the unit's note, mirroring
`withdraw_for_unverified_layout`'s append pattern. A unit replayed from
the container store is always `complete: true` by construction (only a
complete container is ever stored, per §2). `AgentUnit` (`agents/
mod.rs`) gained the same public `complete` field
(`#[serde(default = "default_true")]` for a shape written before this
field existed).

**Growth-history protection** (`agents::discover_and_measure_in`): a
candidate with `complete() == false` is no longer pushed into
`observed` (so it is never diffed against history, mirroring
`external.rs`'s `!row.complete` guard) and its key is added to a new
`incomplete_keys` set, now passed as `observe_and_annotate_external`'s
`protected_keys` argument instead of a hardcoded empty set -- so it is
also never tombstoned by the owned sweep. Building the final
`AgentUnit`: an incomplete candidate's `growth_bytes` stays `None`
(never reset to a false zero) and `regrowth_count` is read back from
the store's last complete observation via
`crate::growth::peek_external_current` rather than defaulting to 0 --
the unit itself is still reported this pass (this pass *did* identify
it, just not completely), with its partial total and a note, exactly
satisfying "a displayed incomplete total must say it is incomplete."

**Every adapter call site that already treated `truncated` as
meaningful** (a `.note("... directory entry count bound reached")` or
equivalent, found across 10 of the 13 adapter files --
`claude_code.rs`, `aider.rs` (x2), `codex.rs`, `codex_desktop.rs`,
`pi.rs`, `vscode_family.rs` (x3), `opencode.rs` (x3), `continue_dev.rs`
(x3), `oh_my_pi.rs` (x1, the plain per-unit fold; the separate
per-container reference-count aggregate at `oh_my_pi.rs:177` was left
alone, see §4), `copilot_cli.rs` (x4), `gemini_cli.rs` (x3)) was
converted from a cosmetic note to `.incomplete(...)`, which both keeps
the note and now actually protects the unit's growth history. This
turned out to be nearly every adapter, not the two or three the prior
session's note guessed at when it named the follow-up.

## 4. What was deliberately left alone

- `oh_my_pi.rs`'s shared-blob reference-count aggregate
  (`encode_refs`/`fold_refs`, `TRUNCATED_FACT`) is a different kind of
  partial total -- a count of references across many sessions, already
  carried through `ContainerFacts`/`Unrecorded` with its own "a
  quietly-short count is worse than no count" discipline (see that
  file's own comments). It does not go through
  `folded_bytes_bounded_stamped` and is out of this chunk's scope.
- A handful of `ctx.folded_bytes` call sites still discard `truncated`
  as `_t`/`_truncated` with no note at all (e.g. Claude Code's
  `sessions`/`todos`/`file-history` container internals, several
  `identify_static_categories`-style residual folds). These predate
  this chunk, are not the bug it was asked to investigate (which named
  one function, not every call site of it), and are not touched here.
  Given the growth-history protection added in §3 is keyed on
  `CandidateAgentUnit::complete`, wiring any of these in later is a
  pure mechanical repeat of this chunk's `.note(...)` ->
  `.incomplete(...)` conversion, not a new mechanism.

## 5. New test

`crates/core/tests/coverage_changes_are_not_storage_changes.rs::an_unreadable_subdirectory_inside_an_agent_unit_is_not_growth_or_regrowth`.
Claude Code's `shell-snapshots` static category (an ordinary,
unprotected `CacheOrLogTrash` unit, well clear of the `sessions/`
container machinery this file's other tests do not exercise): `chmod
000` a plain subdirectory nested inside it across three passes --
baseline, the pass that sees the smaller (incomplete) total, a pass
with access restored. Where the platform enforces the permission bit
(skipped under root), asserts pass 2's unit has `complete: false`,
`growth_bytes: None`, `regrowth_count: 0`, and pass 3's unit is
`complete: true` with `bytes` back to the pass-1 baseline,
`growth_bytes: Some(0)` and `regrowth_count: 0` throughout.

Ran red against the pre-fix code first (reverted just the
`folded_measurement.rs` hunk, left the rest of the fix in place): fails
at `assert!(!u.complete, ...)`, confirming the test exercises the bug
rather than something else. Green after restoring the fix.

## 6. Verification

- `cargo check -p swamp-core --locked` then `cargo check --workspace
  --locked`: clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`:
  clean (caught and fixed five `AgentUnit { .. }` struct literals
  missing the new `complete` field: three in `crates/tui/tests/
  frames.rs`, one in `crates/core/tests/agent_refusal_matrix.rs`, one
  in `agents::testing::no_content_leak`'s debug-JSON round-trip).
- `cargo fmt --all -- --check`: clean.
- `cargo run -q -p swamp-source-audit`: 11/11 ok.
- `cargo test --workspace --locked` (captured with an explicit `echo
  EXIT:$?` rather than trusting a `| tail` pipeline's exit code, which
  is `tail`'s, not `cargo test`'s): `EXIT:0`, 67 `test result: ok`
  lines, 0 `FAILED`.
- `scripts/check.sh` with `SWAMP_TARGET_DIR` unset (the brief's
  specified verification): `EXIT:0` -- fmt, clippy, the 11 audits, the
  release-graph check, the full workspace test run, named-targets and
  the greps, one pass, zero failures. (First run caught a `cargo fmt`
  diff in the new test file, introduced after the earlier `cargo fmt
  --all`; reformatted and reran clean.)

`ps` checked before and after every cargo invocation; no concurrent
cargo runs and no stray background processes from this session.

## 7. Commit

One commit on `stack/25-bounded-fold-completeness`
(`GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign
GIT_CONFIG_VALUE_0=false`, unsigned per `WORKER_BRIEF.md`). Branch
pushed; CI to be confirmed green on both OS jobs before this chunk is
reported done.
