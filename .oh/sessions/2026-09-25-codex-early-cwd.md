# Codex sessions link again: bounded early `cwd` extraction (2026-09-25)

Branch `claude/agent-project-attribution-hybrid`, after
`2026-09-25-agent-folder-inference.md`. Scope: the Codex adapter's
header parse, its tests, and the docs that had claimed Codex carries
no `cwd`.

## The defect
`agents::codex::read_header_cwd` read the bounded 8 KiB prefix of a
rollout and then required the *whole first line* to parse as JSON. On
the owner's machine that line never parsed, so no Codex session had a
`cwd`: 3,450 units, 1.49 GB, all `unresolved`. The earlier spike note
("no cwd in any record within the bound") described the parser's
result, not the file.

## Structural evidence (100 most recent local rollouts; key names and
## byte offsets only -- no values read into any output, nothing retained)
| fact | count |
| --- | --- |
| first record longer than 8 KiB | 100/100 (median 22,457 B, max 48,460 B) |
| first record `"type":"session_meta"` within 8 KiB | 100/100 |
| `payload.cwd` key within 8 KiB | 100/100, at 220-334 B |
| what makes the record long | `payload.base_instructions.text` (the project's instructions file) begins ~600 B in |
| `payload.git` key present in the first record | 92/100, at 18,546-48,291 B -- never within the bound |
| `payload.forked_from_id` within 8 KiB | 9/100 |
| a later `turn_context` record carrying `cwd` within the first 64 KiB | 9/100 |
| `payload.runtime_workspace_roots` | present (an array; one string the same length as `cwd` in the probed record) |

Observed first-record key order: `timestamp`, `ordinal`, `type`,
`payload.{session_id, id, forked_from_id?, forked_from_ordinal_exclusive?,
timestamp, cwd, runtime_workspace_roots, originator, cli_version,
source, thread_source, model_provider, base_instructions.text, ..., git?}`.

## The fix (second cut; the first was rejected)
The first cut (`26fcfe2`) read the 8 KiB prefix through `ctx.derived`
and ran a tokenizer over it that decoded only `type` and the `cwd`.
Rejected by the owner, rightly: the instructions text begins ~600 B in,
so ~7.5 KB of it was in memory before the tokenizer saw a byte. The
packet's rule is about what is *read*, not what is kept.

Second cut: a new gate primitive `fs_gate::read::bounded_scan_header`
reads one byte per `read(2)` and stops at the byte the scanner marks
done, or at the cap (still a `BoundedCap`, still `header_at_most`);
it retains nothing and counts exactly the bytes fetched. Surfaced as
`bounded_io::scan_header` and `IdentifyCtx::derived_scanned`, which
memoises only the extracted value. The Codex adapter's `SessionMetaCwd`
is a byte-fed state machine: it requires the observed root
`timestamp`/`ordinal`/`type=session_meta`/`payload` envelope, streams a
small allowlist of pre-cwd session-ID/timestamp scalars without
retaining them, and stops at the closing quote of `payload.cwd`.
Unknown keys, other record types, malformed value shapes, and unverified
nested envelopes stop before their values. The previous shape-tolerant
acceptance of bare `cwd`, `meta.cwd`, and `payload.meta.cwd` was removed:
this is internal wire format, so alternate variants need verification.

Tests: `the_read_ends_at_the_closing_quote_of_the_cwd_and_the_canary_
after_it_is_never_fetched` -- the canary begins one byte after the
`cwd`; `counters.header_bytes_read == head.len()` exactly, where `head`
ends with the closing quote; `the_scanner_stops_at_the_field_and_
refuses_at_the_record_kind` -- exact stop bytes for: cwd beyond the
ceiling (stops at the cap, `None`); cwd cut by the ceiling (`None`);
`turn_context` (refused at the type's closing quote); cwd in an array;
escaped path (stops at its closing quote, the `SECRET` after it unseen);
unverified nested envelopes; no `type` before the cwd; non-string cwd;
unknown fields before cwd; a non-ASCII path. The long-record fixture
includes observed pre-cwd IDs/timestamps, then proves the read stops
before `base_instructions`.
`bounded_io` has its own test of the primitive's stop and cap; the gate
compile-fail cases (`content_reads_need_a_cap`, `caps_are_named_
constants`, `no_unbounded_read_in_the_gate`) still pass with the new
primitive in the gate.

The source audit `adapters_do_not_reach_gates` rejected the first draft
of the scanner for an `.expect()` (a panic would print its payload);
replaced with a non-panicking path. That audit doing its job is worth
recording.

## Measurement (fresh mktemp store, same config as the folder-inference run)
| Codex | before (`139a1f6`) | after |
| --- | --- | --- |
| sessions linked/declared | 0 | **1,217 (590.6 MB)** |
| sessions missing (declared cwd no longer exists: deleted checkouts, temp worktrees) | 0 | 1,760 (874.2 MB) |
| sessions not-a-project (cwd exists, not a git checkout: e.g. a home dir) | 0 | 473 (42.5 MB) |
| sessions unresolved | 3,450 (1,487.5 MB) | **0** |
| archived-sessions | 23 unresolved | 23 missing |
| cold observe | 11.2-12.8 s | 11.5 s (prefix parse), 11.7 / 12.2 s (byte stream: ~1.3 M one-byte reads, within noise) |
| longest string field in any Codex unit's JSON | -- | 153 chars; no instruction text |

Claude Code sessions in the same run: 29 declared / 119 inferred /
3 unresolved (one more than the earlier run: a new session under a
path outside the known worktree set).

## Corroboration candidates
- `payload.git.{branch,commit_hash,repository_url}`: real corroboration
  of the `cwd` (and the only way to notice a moved checkout), but it
  sits after the instructions text; reaching it means raising the bound
  through 18-48 KB of a file that is, by the privacy contract, content.
  A future design could seek to a known offset only if Codex ever
  writes `git` before `base_instructions`; it does not today.
- `payload.forked_from_id`: names the parent *session*; useful to say
  "forked from <session>" (fork lineage), not to link a project. Parent
  ownership alone is unsafe because a fork can change project context.
- `turn_context.cwd` per turn: later lines; would need reading the
  transcript body. Out.
- `payload.runtime_workspace_roots`: within the bound and resembles
  Oh My Pi's `additionalDirectories`, but official source describes it
  as permission/scope roots, not a record of which projects the thread
  actually used. Do not treat it as ownership or `Shared` without a
  separate explicit usage relationship.

### Codex SQLite metadata spike (metadata/schema only)

Official Codex sources describe `state_5.sqlite` thread metadata with
`cwd`, optional `project_id`, Git origin/branch metadata, and an exact
`rollout_path`; JSONL history and SQLite metadata are separate stores.
The local read-only schema/aggregate check found 4,956 thread rows: all
had `cwd`, none had `project_id`, and 4,512 had a Git origin URL. No row
paths or conversation fields were selected or printed. This is a useful
exact thread-ID join/corroboration source, but adds no `cwd` coverage on
this machine; origin identifies a repo family, not necessarily one
clone/worktree. `runtime_workspace_roots` is not equivalent evidence of
use. Any future reader must use a consistent read-only snapshot and
avoid triggering migrations or touching WAL/SHM state while Codex runs.

Further mapping approaches, ranked:

1. Join rollout thread ID to the exact SQLite thread row via its
   `rollout_path`/thread identity, then use declared `cwd` when a rollout
   header is absent or unsupported. Locally this is redundant for
   existing rows because all sampled rows already have cwd.
2. Use Git origin URL only to corroborate repo-family identity; require
   declared cwd or another exact worktree signal to choose among clones.
3. Use `forked_from_id` for lineage/supporting context only, not inherited
   project ownership.
4. Use canonical Codex `project_id` only where populated and mapped to a
   known root; it is null in the sampled local database.
5. Do not claim project ownership from `runtime_workspace_roots`, a bare
   directory basename, or branch name alone: they are scope/weak clues,
   not proof the session used a worktree.

## Gaps
- Claude Code's `read_header_cwd` still reads up to 8 KiB of the first
  records looking for the first one with a `cwd`; when that record is a
  user message, prompt text is in that buffer. Pre-existing, outside
  this task's scope, and the same rule applies: it should stream and
  stop at the field. Recorded, not fixed here.
- Records where `type` precedes `payload` and both fit in 8 KiB are the
  only shape verified locally; older Codex versions' first lines are
  covered by the pre-existing whole-line tolerance tests, not by
  observation.
- Archived sessions share the parser and the fix, but were not
  separately probed (23 units locally).

## Execute — replace transcript scan with Codex state-index linkage

**Aim:** link Codex sessions to projects without reading rollout contents.
The chosen source is Codex's versioned state SQLite index, located by
`sqlite_home` in config, then `CODEX_SQLITE_HOME`, then the Codex home.
This replaces the entire rollout-header parser; it does not add a
fallback scan. The existing stat-only walk remains because it measures
session storage size.

**Implementation:** `agents::codex_state` selects the highest numeric
`state_<n>.sqlite`, opens it read-only, verifies the `threads` table and
required columns, and selects only exact `rollout_path` plus `cwd`.
Missing, stale, conflicting, or incompatible rows remain unresolved.
No title, preview, prompt, or transcript fields are read. The context
captures `CODEX_SQLITE_HOME` outside the adapter to preserve the
environment-free adapter boundary. The SQLite index is queried each
identification pass, but it does not invalidate the day-container size
cache: cached session rows retain their declared-path basis, and their
project links are refreshed from the current index. Thus unrelated
Codex index writes do not trigger a session-tree re-stat walk. Adapter
and container format versions were bumped so old header-derived rows
cannot replay. Newer numeric state DB filenames are also protected and
folded with their sidecars.

**Evidence and trade-offs:** the earlier local read-only data check
found 4,956 index rows, 3,262 exact rows for 3,473 current rollout
files, with 211 files absent from the index (6.26% of rollout bytes
then measured). DB-only linkage intentionally leaves those unmatched
sessions unresolved; it does not infer by basename or parse content.
The index's incompleteness is accepted in favor of a cheaper and
privacy-preserving source, not hidden as perfect coverage.

**Verification:** Codex adapter tests cover exact-path linking,
transcript-shaped cwd not linking without an index row, conflicting
rows, future state DB versions, private-column canaries, and zero
rollout-header bytes. A container regression changes the SQLite `cwd`
between passes and proves that project attribution refreshes while the
session day is reused with zero re-identification. The full core suite
and TUI suite pass; source audits confirm the adapter still does not
reach filesystem gates or environment directly. The full workspace
release gate remains separate.

**Remaining checks:** confirm actual current Codex config/schema
behavior on a live install without printing or retaining row values;
inspect SQLite's read-only WAL coordination behavior and ensure
protected accounting remains exact. Other agent adapters may still
perform their own bounded reads; this change removes only Codex
rollout-content scanning.
