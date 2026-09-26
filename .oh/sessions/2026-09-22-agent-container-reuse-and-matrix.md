# 2026-09-22 — agent container reuse, and a support matrix with real citations (stack/12)

Branch `stack/12-agent-container-reuse-and-matrix`, based on
`stack/11-review-2-repairs` (`10e9aeb`). Inputs: `CHUNK_R4.md`,
`review/REVIEW-STACK-2.md` sections "Cost measurement" and "Support
matrix vs upstream sources", and the previous session note's two
"measured, not satisfied" assertions.

Two halves. The first is the obvious next chunk the previous note named:
give the agent family the folded-row reuse the external family got,
keyed per container. The second is the re-review's mandate item 5 —
every `Supported` row re-verified against fetched, pinned upstream
source, with the adapters corrected where they disagreed.

---

# Part A — container-level reuse

## What it costs now

`incremental_external_and_agent_measurement.rs`, 5,000 synthetic Claude
Code sessions across **5** `projects/<encoded-cwd>/` containers:

| | first pass | unchanged second pass |
|---|---|---|
| wall time | 606 ms | 348 ms |
| header bytes | 740,000 | **0** |
| `dirs_listed` | 11 | **5** |
| `files_statted` | 5,031 | **31** |
| containers identified / reused | 5 / 0 | 0 / **5** |

One appended session: **1** container identified, 4 replayed, 1 header
read, exactly 1 derivation cache miss.

The 5 listings on the unchanged pass are all at the *home* level
(`projects/`, the home itself, and the absent `file-history/`,
`image-cache/`, `uploads/`), not per container. The 31 stats are the
watched directories' stamps plus those listings' entries. Neither grows
with `SESSIONS`, which is the claim.

## The mechanism

- `assoc_store::ContainerTable` → `associations/agent_containers.parquet`:
  one `dir` row per directory the container's identification listed, one
  `unit` row per unit, one `member` row per member. Columns, not a JSON
  blob in a Parquet cell. Never one row per file.
- `agents::ContainerCache` + `IdentifyCtx::container(adapter_id, dir, f)`.
  Every `ctx.list` and `ctx.folded_bytes` inside `f` records the
  directories it touched; `ctx.watch(dir)` declares one the closure
  depends on but does not list (Claude Code's `file-history/`,
  `image-cache/`, `uploads/`, `todos/` — all of which gain a
  `<session-id>` entry when a session acquires one).
- The fingerprint is those directories' own `(mtime_ns, ctime_ns)`, from
  the `stat` the reuse pays anyway; absent and not-a-directory are
  distinct values, so an appearance is a change.
- Nesting is refused by construction: a `container` call inside another
  container's closure runs inline and folds into the outer fingerprint,
  because a nested container that missed while its parent hit could not
  be re-identified without re-running the parent.
- A **truncated** fold makes its container unstorable: a measurement that
  stopped at the entry bound is not described by directory stamps.

## Linkage is not replayed

This was the design decision worth the most. A container's bytes and
members live inside the directories being stamped; its *project link*
does not — it is resolved against a declared path elsewhere on the disk,
so replaying it would keep reporting `Linked` for a worktree deleted
between two passes.

So a unit records the declared path (`LinkBasis::Declared`) rather than
the resolved state, and a replayed container re-resolves it live,
memoised per distinct declared path. That is one resolution per
*project*, not one per session: still container-scaled. A unit whose link
can neither be recomputed nor go stale (`Linked`/`Missing`/`Moved`/… with
no declared path) makes its whole container unpersistable — refusing to
store is the conservative outcome.

## The limit, and the concern I am raising rather than hiding

The stamp is a directory's `mtime`/`ctime`. A file rewritten **in place**
— same name, same directory — does not move it.

For the external family that is a corner case. For the agent family it is
not: **a tool appends to an open session transcript in place.** An append
moves the file's own size and mtime and not its parent's, so a container
whose shape has not changed is replayed and the growing session's stored
byte total stands until the next create/delete/rename inside it (the next
new session, companion directory or `todos/` entry).

Three things follow, and I want the integration owner and re-review 3 to
decide on the third:

1. It is a **reporting lag, not an authorization hole.** Every execution
   sink re-derives from the live filesystem with both caches disabled
   (`reidentify_for_tool`), which
   `relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged`
   now asserts directly.
2. It is **measured, not assumed**:
   `a_session_rewritten_in_place_is_not_seen_until_its_container_moves`
   asserts both halves — the stale total, and the real size appearing
   once the container is re-identified.
3. **The chunk brief asked for exactly this trade** ("a file rewritten in
   place does not change its parent's mtime — state this limit exactly as
   the external family does"), and I implemented the requirement rather
   than narrowing it. But the brief's wording is about *rewrites*, and I
   do not think it anticipated that session transcripts are **appended**
   in place as the normal case. The cost of removing the lag is exactly
   one `stat` per session file per pass — which is the "scales with all
   files" shape the handoff forbids — so the two requirements are in
   genuine tension and someone other than me should pick. A middle path
   exists and is not implemented here: gate reuse on a container's stored
   `mtime_max`, so recently-active containers are re-identified every
   pass and the dormant long tail (where the 5,000 sessions are) is
   replayed. That keeps the cost container-scaled *and* keeps an active
   project's numbers fresh.

## I changed an existing adversarial test, and here is exactly how

`relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged`
rewrites a session's declared `cwd` to a same-length path and asserted
that the next ordinary observation reported the new project. Container
reuse breaks that assertion — it is precisely the in-place rewrite above.

I did not delete it and did not weaken it to "fewer"/"eventually". It now
asserts three things where it asserted one:

- a **fresh re-identification** (the execution-sink path) sees the moved
  attribution immediately;
- the replayed pass reports the *old* project, named as the recorded
  limit rather than as desired behaviour;
- once anything moves the container's stamp, the ordinary pass reports
  the new project — and no pass, at any point, fabricates a growth delta
  for a session whose bytes did not change.

That is more information than the original test carried. It is still a
behaviour change to something a previous review cared about, which is why
it is written up here rather than buried in a diff.

## The reviewer cost test: what still fails, and the exact residual

`reviewer_cost_measurement_stack2::two_unchanged_full_observations_cost_report`
asserts four things about the unchanged pass. **Two hold**
(`header_bytes_read == 0`, zero subprocess spawns). **Two do not**, and I
did not edit the file:

    assert_eq!(second.dirs_listed, 0, ...)    // actual: 40  (was 46)
    assert_eq!(second.files_statted, 0, ...)  // actual: 5,605 (was 10,580)

Attributed rather than assumed — the same fixture observed with each part
separately (`ObservationParts::WALK_ONLY` / `AGENTS` / `EXTERNAL`):

| part of the unchanged pass | `dirs_listed` | `files_statted` |
|---|---|---|
| the walk alone | 34 | 5,553 |
| the agent family's share | 6 | 33 |
| the external family's share | 0 | 19 |
| **total (`ALL`)** | **40** | **5,605** |

The agent family's share fell from 12 listings / 5,027 stats to 6 / 33.
**The walk is now the whole of the residual.** It is byte-identical on
both passes, and it scales with the fixture because the walk's roots
include the synthetic agent home itself.

Why the walk cannot be made free here, checked rather than assumed:

- Pass 1 takes the `full_rules_changed` branch of `growth::observe`,
  which deliberately anchors no FSEvents id (no replay ran), so pass 2
  replays from nothing and refuses with `no_stored_event_id`.
- Anchoring an id at the *start* of a full walk would make pass 2 depend
  on `fseventsd`'s own log lag — the reason `RefreshRefusal::TooSoon`
  exists — so it is timing-dependent, not deterministic.
- The reviewer test constructs its own source
  (`fs_events::platform_source()`) inline, so no fixture source can be
  injected without editing a file this chunk may not edit. I checked
  `report_scope_with_source`: the seam exists, the test does not use it.

And even a zero-cost walk would leave `files_statted` at **52** — the
container and folded-row stamp checks, which are the reuse's own cost and
are counted as the real `stat`s they are. `== 0` is therefore not
reachable without reclassifying counted work, which is the
vacuous-instrument sin the re-review named. Left failing, with the
numbers, for re-review 3.

---

# Part B — the support matrix, re-verified against pinned source

Three parallel research passes fetched every cited file at a pinned
commit. Excerpts are vendored under
`crates/core/tests/fixtures/upstream/<tool>/<commit-prefix>/` with a
provenance header, listed in `citations.toml` with their blake3 digest
and the symbols each claim depends on, and checked offline by
`upstream_citations_are_checked.rs` (45 citations). `github/copilot-cli`
is under a proprietary licence that permits unmodified redistribution
only, so it is recorded as line numbers plus symbol strings in a
`NO-VENDOR-` file and backs no claim.

## Pinned commits

| upstream | commit | date |
|---|---|---|
| openai/codex | `ac7634b9f73ec1bf96466be7a5869f0949d20b30` | 2026-09-22 |
| sst/opencode | `fe3f3a41f79ad292cc3c7c629567385a20ec5130` | 2026-09-21 |
| google-gemini/gemini-cli | `d5b3e3accb26000d273abf16e0f1dd83aa5428a9` | 2026-09-21 |
| cline/cline | `254f40c4b592d1e662b84f2ba06fe45dca77cab3` | 2026-09-22 |
| RooCodeInc/Roo-Code | `b867ec9145750d0ae1ff7f02d35406e9bf2a0b16` | 2026-05-15 |
| continuedev/continue | `5522c6f44ca0ac3528b37244818fbfa39b5af470` | 2026-07-21 |
| github/docs | `72e940d15a9aff06b6e84216f3c97dac25c47d9b` | 2026-09-22 |
| earendil-works/pi | `d201760ffee16564aa8d9a759e0c85b70db33674` | 2026-09-22 |
| can1357/oh-my-pi | `fd3f8e3c569b511611081e16b181b740a4c98599` | 2026-09-22 |
| Aider-AI/aider | `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` | 2026-05-22 |
| code.claude.com, docs.devin.ai, cursor.com | docs pages | retrieved 2026-09-22 |

## Per row: what the source said, and what changed in the adapter

- **Gemini CLI** — `getGlobalBinDir() = join(getGlobalTempDir(), 'bin')`
  and `getGlobalTempDir() = join(getGlobalRuntimeDir(), 'tmp')`
  (`packages/core/src/config/storage.ts:24-25,195-201`; the reviewer's
  path said `packages/cli/`, the file is under `packages/core/`). The
  adapter looked at `~/.gemini/bin` — **dead code, no version writes it**
  — and `identify_tmp` would additionally have reported `tmp/bin` as a
  project id. Both fixed, both asserted. `SANDBOX=sandbox-exec` moves the
  runtime dir to `~/.cache/.gemini` (`:86-107`), now a second detector
  location proposed only when this process is itself under that sandbox.
- **Codex** — `const RUNTIME_DBS: [RuntimeDbSpec; 7]`
  (`codex-rs/state/src/sqlite.rs:105-113`). The seventh,
  `memories_v2_1.sqlite`, is easy to miss because its filename is an
  inline literal in a struct-update over `MEMORIES_DB` and there are
  exactly six `*_DB_FILENAME` consts. It was unfolded, unprotected and
  uncounted. Found while fixing it: the `-wal`/`-shm` sidecars of *all*
  stores were missing from `seen_top_level`, so their bytes were counted
  a second time in the unclassified residual. `state/src/lib.rs` holds
  only `SQLITE_HOME_ENV`, not the filenames — the row cited it for both.
  `skills/` carries upstream's own "Deprecated user skills location"
  comment (current root `~/.agents/skills`); `log_dir` defaults to
  `$CODEX_HOME/log`.
- **Codex desktop** — `desktop_log_root` is in `doctor/desktop.rs:135`,
  and `doctor/desktop/platform.rs` (which the row cited) contains no
  `log_root` at all. The function matches exactly `"macos"` and
  `"windows"` and returns `None` otherwise, so **Windows is confirmed**
  (`%LOCALAPPDATA%/Codex/Logs`) and the row's "no Windows desktop build
  is confirmed" was false; **Linux** is the only unconfirmed platform.
- **OpenCode** — every storage key becomes
  `path.join(dir, ...key) + ".json"` (`storage.ts:62-64`), reached from
  `storage.write(["session_diff", input.sessionID], diffs)`
  (`session/revert.ts:77`). So `storage/session_diff/<id>` is a **file**;
  the adapter gated on `is_dir()` and swept with `dir_names`, so every
  diff — claimed or orphaned — was in no unit at all. Both paths fixed
  and asserted on the *byte total*. `packages/core/src/global.ts` defines
  nine named entries, not three: `state` (`$XDG_STATE_HOME/opencode`) and
  `tmp` are real and are now listed as **unmodelled** rather than left
  silent.
- **Cline** — `vscode-to-file-migration.ts:25-28` says taskHistory "is
  **NOT** migrated here. It uses its own file-based storage at
  `{globalStorageFsPath}/state/taskHistory.json`", and
  `legacy-state-reader.ts:42-44` is the executable form. The store is
  *inside the directory this adapter already walks*, so the previous
  "it's in `state.vscdb`, which we refuse to open" reason was wrong about
  where the answer lives. Linkage now works: one bounded read per host,
  indexed by task id, field `cwdOnTaskInitialization` (optional
  upstream — not `cwd`/`workspace`/`workspaceFolder`, none of which exist
  in the type). Upstream spells the path **three** ways at the same
  commit (`state/` in the migration comment, `tasks/` in the same file's
  skip list, `~/.cline/data/tasks/` in `.clinerules/storage.md`); the
  adapter follows the code and reports the ambiguity rather than
  asserting one. The second root (`CLINE_DATA_DIR` → `CLINE_DIR/data` →
  `~/.cline/data`) is now a detector location.
- **Copilot CLI** — **no citation exists.** Four pinned `github/docs`
  pages contain zero occurrences of `workspaceFolder` or
  `workingDirectory`; every `cwd` hit is the `/cwd` slash command, prose,
  an MCP launch key or an ACP wire parameter. `github/copilot-cli` is
  closed source (its repo holds a README, a changelog, an installer and
  issue templates). The three field names this adapter parsed were the
  guess `docs/agent-storage.md` promised never to make. Linkage is now
  `Unresolved` and **no selective action is offered on session state** —
  the removal capability rested on knowing which project a session
  belonged to. Directory names stay confirmed, so the bytes are still
  identified and measured. The test that asserted the link is kept and
  inverted.
- **Continue** — `Session.workspaceDirectory` is required
  (`core/index.d.ts:282`), and `core/util/history.ts:105` writes `""`
  from the `catch` of `load(sessionId)`, so an empty string is an
  *expected* on-disk value. It now resolves to `Unresolved` with a reason
  naming upstream's own fallback, never `Missing` (which would claim a
  path was named and has since disappeared). The stale "no confirmed
  per-session workspace-linkage field" prose is deleted.
- **Claude Code** — the cited page documents `todos/`, `statsig/` and
  `logs/` in one row as "Legacy directories from older versions. No
  longer written", and its what-you-lose table answers "Nothing". The
  adapter called `statsig` community-documented and modelled it as
  auto-regenerating; both halves were wrong. `logs/` is now modelled too,
  and unmatched `todos/` entries — previously in **no unit at all**, with
  a test that asserted the gap — are folded into one `todos (unlinked)`
  unit.
- **Windsurf** — the product was renamed; the read-write profile is
  `~/Library/Application Support/Devin` and `.../Windsurf` is legacy.
  Both are modelled (an installation mid-migration has bytes in each).
  Partly refuted: `~/.codeium/` is **not** legacy — the FAQ states that
  tree is not changing and stays read-write. Stays `Unverified`: the page
  gives the roots and a flat "Contains:" list, no per-workspace layout.
- **Cursor** — re-searched (`cursor.com/docs/llms.txt`, the agent and
  configuration pages; the old chat-history page now redirects to the
  docs root). Zero hits for `state.vscdb`, `workspaceStorage`,
  `globalStorage` or `Application Support`, and Cursor is closed source.
  **Stays Unverified.** Recorded, not invented.
- **Pi** — `PI_CODING_AGENT_DIR` is in
  `packages/coding-agent/docs/environment-variables.md:81`; the row cited
  `settings.md`, which does not contain the string at all (428 lines, 0
  hits). Real repository: `earendil-works/pi`. There is no `logs/`
  directory — a single `pi-debug.log`.
- **Oh My Pi** — re-pinned against `packages/utils/src/dirs.ts`. Two
  relocations upstream supports and this catalog does not model are now
  stated: a named profile (`--profile`/`OMP_PROFILE`) re-roots everything
  under `~/.omp/profiles/<name>/`, and an existing XDG data/state/cache
  directory relocates the corresponding subtrees.
- **Aider** — re-pinned. One correction: `.aider.llm.history` is
  **opt-in** (`--llm-history-file` defaults to `None`; the name appears
  only inside a help string), so listing it as always-present was wrong.
- **Roo Code** — re-pinned; the `workspace` field is optional on
  `HistoryItem` and upstream keys its own linkage off it
  (`getByWorkspace`), which is what the adapter already reads.

## Why CI missed all of it, and what replaced the check

`agent_matrix_matches_docs` asserted the "Verified against" cell was
longer than 30 characters; `matrix.rs`'s own test asserted three strings
were non-empty. Now:

- `upstream_citations_are_checked` — every vendored excerpt hashes to its
  recorded digest (an excerpt edited to fit a claim stops matching) and
  contains every symbol the claim depends on; every `Supported` row's
  citation pins a commit or a date and names a file that is vendored.
  Its manifest parser takes single-quoted symbols so a symbol can itself
  contain a double quote — several must
  (`workspaceDirectory: ""`, `"state", "taskHistory.json"`), and dropping
  the quotes would weaken exactly the citations that need to be
  strongest.
- `no_prose_paragraph_asserts_doubt_a_supported_row_has_resolved` — the
  prose half. A paragraph that names a `Supported` tool and no
  `Unverified` one may not assert "no confirmed", "assumed",
  "community-documented", "not re-fetched", "not modeled yet" or "not
  independently re-confirmed" unless it also marks that doubt as history
  ("previously", "used to", "was wrong", "superseded", …), which is how a
  correction is written. Deliberately *not* triggered by "not confirmed"
  on its own: a partial `Supported` row (Codex desktop's logs-only
  support) legitimately says what it does not model.
  - It found a real one on its first run — the Codex desktop paragraph
    claiming no Windows build was confirmed — and rejects the exact
    Continue sentence the re-review quoted (verified by adding it back).

---

# What this chunk did not do

- **Only Claude Code uses the container seam.** `IdentifyCtx::container`
  is generic and every adapter can call it, but the conversion here is
  one adapter — the one with the 5,000-session fixture. Codex's
  `sessions/<yyyy>/<mm>/<dd>/` is the obvious next one and is *not*
  done, for a specific reason rather than for time: `collect_jsonl_files`
  carries an entry-count bound (`already_seen + out.len()`) shared across
  `sessions/` and `archived_sessions/`, so a day container's output
  depends on how many files the containers before it produced. Replaying
  one would have to feed replayed unit counts back into that bound or the
  bound would mean something different on a reused pass than on a fresh
  one. That is a real change to what the bound guarantees and deserves
  its own review, not a quiet edit at the end of a chunk.
- **The activity-gated reuse** described above (re-identify containers
  whose stored `mtime_max` is recent, replay the dormant tail) is
  proposed, not implemented.
- **`reviewer_cost_measurement_stack2` is still red** on two assertions,
  with the residual attributed above. Nothing else in `scripts/check.sh`
  fails.

- **`mutation_corpus` costs about two minutes** on this machine
  (`every_mutation_fixture_is_rejected_by_its_audit`): it copies the
  whole workspace and re-parses it once *per fixture*, 136 times. Making
  it parse once and apply each fixture in memory is a real speedup and
  is deliberately **not** done in this chunk -- it changes the machinery
  the audits' own evidence rests on, which deserves its own review.
- **`reviewer_counterexamples_stack2` must be run `--test-threads=1`**,
  as `scripts/check.sh` does. Running it under a plain
  `cargo test --workspace` fails
  `a_disabled_detector_must_not_probe_its_tool` with
  `["lsof", "lsof"]`: the test installs PATH shims process-wide and
  counts spawns in a shared file, so a sibling test in the same binary
  that probes occupancy pollutes the count. Pre-existing, unrelated to
  this chunk, and worth a note because the failure reads like a real
  regression.
