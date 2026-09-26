# Agent-tool storage

Coding-agent CLIs and editor extensions (Claude Code, Codex, Oh My Pi,
OpenCode, Gemini CLI, Pi, Aider, GitHub Copilot CLI, Cursor, Windsurf,
Cline, Roo Code, Continue, and others) keep session transcripts,
caches, logs, checkpoints and configuration under their own home
directory (or, for Aider, partly inside each project checkout instead).
This document is the reference for how `swamp` discovers, models,
presents and selectively cleans up that storage
(#90/#91/#92/#93/#94/#95/#96/#97/#98/#99/#100/#101), and what is
deliberately not done yet. Every named tool in the required matrix now
has real identification code -- see "Required tool matrix" below.

**The aim:** help a developer understand where agentic coding tools
consume disk, what grew, which project (if any) it belongs to, and what
they can give up with an honest, specific consequence -- never a
"safe to delete" verdict, and never a guess.

## Privacy contract

This is the hard rule everything else in this document sits on top of:

- Identification reads directory names, file sizes, and modification
  times. For most adapters, a session's project linkage comes from
  reading **only the first line** of the session's own transcript file
  (bounded to 8 KiB), looking for one JSON field (`cwd`). Codex is the
  exception: it reads no rollout JSONL content at all. It joins each
  file's exact path to Codex's read-only SQLite thread index and selects
  only `rollout_path` and `cwd`; missing, conflicting, or stale index
  entries remain unresolved. This avoids reading prompt/instruction
  text and avoids parsing an internal transcript format. Oh My Pi's
  sessions have a fixed 256-byte title slot before that header line and
  this adapter skips it rather than reading it. OpenCode's project
  linkage instead comes from its own small, declared `project.json`
  metadata file (a `worktree` field), never from session content at
  all. Oh My Pi's shared-blob reference accounting is the one adapter
  that reads further into a session body (bounded to 64 KiB per
  session), and only to extract opaque `blob:sha256:<hash>` reference
  tokens -- never any other content, never persisted as text anywhere.
  #96-#99's nine adapters follow the same discipline: Pi reads one
  `cwd` field out of its own documented offset-zero header, and **only**
  that shape -- it no longer falls back to Oh My Pi's title-slot layout,
  because one tool's format change must never silently change another
  tool's identification; Aider, Cursor/Windsurf and GitHub Copilot CLI's
  `session-store.db` never read transcript content at all (declared
  metadata or a protected, never-opened database); GitHub Copilot CLI's
  `session-state/`, Cursor/Windsurf's `workspace.json`, Roo Code's
  `history_item.json` and Continue's session header each read one small,
  bounded metadata file (never the conversation body) looking for one
  declared path field. Cline reads none, because Cline records the
  working directory somewhere this catalog does not read (see its matrix
  row). Nothing else, in any adapter, is ever read.

  Every one of those bounded reads is also *memoised*: the derived value
  is cached against the source file's own `(len, mtime_ns, ctime_ns,
  inode)` plus the adapter version, in the Parquet identification table,
  so an unchanged session costs **zero** bytes on the next pass. That is
  a privacy property as much as a cost one -- the fewer times a header is
  read, the fewer chances there are to read it wrongly.
- No prompt, response, attachment, tool-output, or credential *content*
  is put into a report, a plan, the ledger, a log, or a test fixture.
  Every fixture used to build and test this feature is synthetic:
  invented session ids, invented project paths, placeholder bodies
  like `"[redacted]"`.
- Formats were researched from primary upstream documentation/source
  (linked below). Codex linkage uses its versioned `state_<n>.sqlite`
  thread index, resolved by `sqlite_home`, `CODEX_SQLITE_HOME`, then the
  default Codex home. The adapter opens the selected DB read-only,
  checks the required schema, and queries only `rollout_path` and `cwd`;
  it does not read conversation, title, preview, or prompt columns and
  never falls back to rollout parsing. Rows missing from the index stay
  unresolved. No rollout contents are read for size accounting either.
- Adversarial tests seed a canary string into fixture session bodies
  and assert it never appears in any unit, plan, execute result, or
  ledger entry (`crates/core/src/agents/claude_code.rs`'s
  `a_session_is_identified_with_its_companion_members`,
  `crates/core/tests/agent_units_actions.rs`'s
  `plan_and_ledger_never_contain_the_canary_prompt_content`, and, for
  the nine #96-#99 adapters,
  `crates/core/tests/agent_units_actions_remaining_tools.rs`'s
  `no_canary_anywhere` helper, applied after every execute in that
  file).

## Required tool matrix

Source of truth: `crates/core/src/agents/matrix.rs` (`MATRIX`). The table
below is **parsed back** by
`crates/core/tests/agent_matrix_matches_docs.rs`, which compares every
row's id, support level and "actions" column against that constant and
against `agents::Registry`'s registered adapter ids. The two cannot
drift: a level changed in one place and not the other fails the test.

`Support` is one of:

- **supported** -- an adapter exists *and* the layout it models is
  confirmed against that tool's own source or documentation, cited in the
  row's `Verified against` column. Actions are available.
- **unverified** -- an adapter exists, but the layout it models could not
  be confirmed. Units are still identified and measured; **no action is
  offered and project linkage is reported `unresolved`**, because both
  would be claims resting on a layout we have just said is unconfirmed.
  Enforced once, in `agents::discover_and_measure`, not left to each
  adapter.

Every row said **supported** before 2026-09-21. Two had not earned it,
and three more carried a claim that turned out to be false. What changed
is recorded per row below and in
`.oh/sessions/2026-09-21-foundation-repairs-part-2.md`.

| Tool | Id | Support | Actions | Home / override | Verified against |
|---|---|---|---|---|---|
| Claude Code | `claude-code` | supported | yes | `~/.claude`, or `$CLAUDE_CONFIG_DIR` if set | [claude-directory](https://code.claude.com/docs/en/claude-directory) (retrieved 2026-09-22): the directory table and the `CLAUDE_CONFIG_DIR` override, which the page states re-roots every path on it. The page also documents `todos/`, `statsig/` and `logs/` in one row as **legacy directories from older versions, "No longer written"** -- so they are not auto-regenerating caches, and this catalog's previous "community-documented, regenerates automatically" note for `statsig` was wrong in both halves |
| Codex | `codex` | supported | yes | `CODEX_HOME`, default `~/.codex` (`sessions/`+`archived_sessions/` year/month/day rollout trees, `auth.json`, `history.jsonl`, `config.toml`, `log/`, a deprecated `skills/`, **seven** `*.sqlite` state stores relocatable via `CODEX_SQLITE_HOME`) | openai/codex `main` @ `ac7634b9f73ec1bf96466be7a5869f0949d20b30`: `codex-rs/state/src/sqlite.rs` (`const RUNTIME_DBS: [RuntimeDbSpec; 7]` -- `state_5`, `logs_2`, `goals_1`, `memories_1`, **`memories_v2_1`**, `queue_1`, `thread_history_1`; this row previously said six), `codex-rs/state/src/lib.rs` (`SQLITE_HOME_ENV` only -- **not** the filenames, which this row previously cited it for), `codex-rs/config/src/config_toml.rs` + `codex-rs/core/src/config/mod.rs` (`log/`, overridable by `log_dir`), `codex-rs/ext/skills/src/host_roots.rs` (`skills/`, upstream-deprecated in favour of `~/.agents/skills`), `codex-rs/utils/home-dir/src/lib.rs`, `codex-rs/rollout/src/{lib,list,rollout_file_name,metadata}.rs`, `codex-rs/protocol/src/protocol.rs` (`SessionMeta.cwd`) |
| Codex desktop app | `codex-desktop` | supported | yes | logs only: macOS `~/Library/Logs/<identity>` and Windows `%LOCALAPPDATA%/Codex/Logs`, both day-partitioned `YYYY/MM/DD`; Linux unconfirmed. Settings/session storage beyond logs is unconfirmed and not modeled | openai/codex `main` @ `ac7634b9f73ec1bf96466be7a5869f0949d20b30`: **`codex-rs/cli/src/doctor/desktop.rs`** (`desktop_log_root`). This row previously cited `doctor/desktop/platform.rs`, which contains no `log_root` at all, and claimed no Windows desktop build was confirmed -- the source confirms it; only Linux returns `None` |
| Oh My Pi | `oh-my-pi` | supported | yes | `~/.omp/agent`, or the whole of `PI_CODING_AGENT_DIR` when set. **Unmodelled and stated rather than left silent**: a named profile (`--profile`/`OMP_PROFILE`) re-roots everything under `~/.omp/profiles/<name>/`, and an existing XDG data/state/cache directory relocates the corresponding subtrees | can1357/oh-my-pi `main` @ `fd3f8e3c569b511611081e16b181b740a4c98599`: `packages/utils/src/dirs.ts` (`CONFIG_DIR_NAME = .omp`, `getAgentDir`, `getSessionsDir`, `getBlobsDir`, `getLogsDir`, the `agent.db`/`history.db`/`models.db` stores), `docs/config-usage.md` (the profile and XDG relocations) |
| OpenCode | `opencode` | supported | yes | data `${XDG_DATA_HOME:-~/.local/share}/opencode`; config `${XDG_CONFIG_HOME:-~/.config}/opencode`, also settable by `OPENCODE_CONFIG_DIR`; cache `${XDG_CACHE_HOME:-~/.cache}/opencode` -- config and cache reported as opaque external units. **Unmodelled**: the `state` root (`$XDG_STATE_HOME/opencode`) and `tmp` | sst/opencode `dev` @ `fe3f3a41f79ad292cc3c7c629567385a20ec5130`: `packages/core/src/global.ts` (the roots -- data, config, cache, **state**, tmp, plus derived bin/log/repos; **`OPENCODE_DATA_DIR` does not exist**), `packages/opencode/src/storage/storage.ts` (every key becomes `path.join(dir, ...key) + ".json"`, so `storage/session_diff/<session-id>` is a **file**, not a directory), `packages/opencode/src/session/revert.ts`, `packages/core/src/database/database.ts` (`opencode.db`) |
| Gemini CLI | `gemini-cli` | supported | yes | `~/.gemini`, or the whole of `GEMINI_CLI_HOME` when set (`settings.json`, `GEMINI.md`, `extensions/`, `trustedFolders.json`, `oauth_creds.json`, and **`tmp/bin`** -- not `bin/`); `tmp/<project-id>/` and `history/<project-id>/`, where the id is a legacy `sha256` or a current registry slug. Under `SANDBOX=sandbox-exec` the runtime dir moves to `~/.cache/.gemini` | google-gemini/gemini-cli `main` @ `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`: `packages/core/src/config/storage.ts` (`getGlobalBinDir() = join(getGlobalTempDir(), 'bin')` -- so the cache is at `tmp/bin` and this row's `~/.gemini/bin` was a path no version writes; and the `sandbox-exec` -> `~/.cache/.gemini` runtime move), `packages/core/src/utils/paths.ts` (`GEMINI_DIR` is a constant, not an env var), `packages/core/src/config/projectRegistry.ts` (the `projects.json` slug registry and the hash-to-slug migration) |
| Pi | `pi` | supported | yes | `~/.pi/agent/`, overridable via `PI_CODING_AGENT_DIR` (shared with Oh My Pi's own override) | earendil-works/pi `main` @ `d201760ffee16564aa8d9a759e0c85b70db33674`: **`packages/coding-agent/docs/environment-variables.md`** (`PI_CODING_AGENT_DIR`) -- this row previously cited `settings.md`, which does not contain that string at all; the claim was true, the citation did not establish it. Also `packages/coding-agent/src/config.ts` (`CONFIG_DIR_NAME = .pi`, `getAgentDir`, `getSessionsDir`) and `docs/sessions.md` |
| Aider | `aider` | supported | yes | `~/.aider/caches` and an optional home-level `.aider.conf.yml`; per-repo `.aider.chat.history.md`/`.aider.input.history` and `.aider.tags.cache.v{3,4}/` at the git root -- project-local, attached to the worktree model. `.aider.llm.history` is **opt-in** upstream, so it is identified where present and never assumed | Aider-AI/aider `main` @ `5dc9490bb35f9729ef2c95d00a19ccd30c26339c`: `aider/args.py` (the history filenames and `--llm-history-file` defaulting to `None`), `aider/repomap.py` (`TAGS_CACHE_DIR`), `aider/models.py` (the 24-hour-TTL price cache), `aider/main.py` (`.aider.conf.yml`) |
| GitHub Copilot CLI | `github-copilot-cli` | supported | yes | `COPILOT_HOME` and `COPILOT_CACHE_HOME` supported. Local session directories with `events.jsonl` can be removed; `workspace.yaml` supplies bounded cwd linkage. Unknown formats remain unresolved | GitHub config-directory reference; copilot-sdk @ `4001c1da7d832c51bad1d38619c1a082af390efb` session filesystem tests; upstream CLI #2446 and SDK #1735. See detailed format limits below |
| Cursor | `cursor` | unverified | no | editor-profile storage, macOS only: `~/Library/Application Support/Cursor` (`User/globalStorage/state.vscdb`, `User/workspaceStorage/<id>/{state.vscdb,workspace.json}`, `User/History`), plus `~/.cursor/` (not decomposed) | Re-searched 2026-09-22 -- `cursor.com/docs/llms.txt` (the official 457-line index), `/docs/agent/overview`, `/docs/agent/projects`, `/docs/configuration/worktrees`; the former `docs.cursor.com/en/agent/chat/history` now redirects to the docs root. **NOT CONFIRMED**: zero hits for `state.vscdb`, `workspaceStorage`, `globalStorage` or `Application Support` in any of them, and Cursor is closed source so there is no repository to pin. The only official on-disk path found is unrelated to session state (`cli/reference/configuration`: `~/.cursor/cli-config.json`). Only `forum.cursor.com` community threads corroborate the profile layout |
| Windsurf | `windsurf` | unverified | no | macOS only: the **current** profile at `~/Library/Application Support/Devin` and the **legacy** one at `.../Windsurf` (both modeled -- an installation mid-migration has bytes in each), plus `~/.codeium/windsurf`, which upstream states is *not* changing in the rename and stays read-write | [docs.devin.ai desktop FAQ](https://docs.devin.ai/desktop/devin-desktop-faq), retrieved 2026-09-22 (`docs.windsurf.com` 307-redirects here): **PARTIALLY CONFIRMED** -- both profile roots on macOS/Windows/Linux are named, with `User/settings.json`, `User/keybindings.json`, `User/snippets/`, `globalStorage/`, `Workspaces/` and `argv.json` inside them. `workspaceStorage` is **not** on the page, and neither are the `Cache`/`CachedData`/`CachedExtensionVSIXs`/`logs` siblings this adapter also models, so the row stays unverified |
| Cline | `cline` | supported | yes | VS Code extension global storage, one location per known editor host (Code, Code Insiders, Cursor, Windsurf, `~/.vscode-server` remote): `globalStorage/saoudrizwan.claude-dev/tasks/<task-id>/` plus `state/taskHistory.json`. The second root -- `CLINE_DATA_DIR`, else `CLINE_DIR/data`, else `~/.cline/data` -- **is now modeled** as its own location | cline/cline `main` @ `254f40c4b592d1e662b84f2ba06fe45dca77cab3`: `apps/vscode/src/sdk/legacy-state-reader.ts` (the per-task filenames, and `path.join(resolveDataDir(dataDir), "state", "taskHistory.json")`), `apps/vscode/src/hosts/vscode/vscode-to-file-migration.ts` (taskHistory "is **NOT** migrated here" -- it lives at `{globalStorageFsPath}/state/taskHistory.json`, and for VS Code that path is *not* `~/.cline/data`), `apps/vscode/src/shared/HistoryItem.ts` (`cwdOnTaskInitialization`, optional), `apps/vscode/src/shared/storage/storage-context.ts` + `sdk/packages/shared/src/storage/paths.ts` (the second root), `.clinerules/storage.md`. **Upstream's own spelling is ambiguous** -- `state/taskHistory.json` (migration comment), `tasks/taskHistory.json` (same file's skip list), `~/.cline/data/tasks/taskHistory.json` (`.clinerules/storage.md`) -- so this catalog follows the executable code (`state/`) and says so in the unresolved reason rather than asserting one path |
| Roo Code | `roo-code` | supported | yes | Same per-host modeling as Cline: `globalStorage/rooveterinaryinc.roo-cline/tasks/<task-id>/` (`api_conversation_history.json`, `ui_messages.json`, `task_metadata.json`, `history_item.json`, plus `tasks/_index.json`). The `roo-cline.customStoragePath` setting can relocate `tasks/` entirely, in which case this catalog simply does not find it | RooCodeInc/Roo-Code `main` @ `b867ec9145750d0ae1ff7f02d35406e9bf2a0b16`: `src/shared/globalFileNames.ts`, `src/utils/storage.ts` (`getTaskDirectoryPath` and `getStorageBasePath`'s override), `packages/types/src/history.ts` (the optional `workspace` field), `src/core/task-persistence/TaskHistoryStore.ts` (upstream keys its own linkage off the same field via `getByWorkspace`, written into `history_item.json`, **not** `task_metadata.json`) |
| Continue | `continue` | supported | yes | `~/.continue`, or `CONTINUE_GLOBAL_DIR` when set (`config.yaml`/`config.json`, `sessions/<id>.json` + a `sessions/sessions.json` index, `index/` embeddings/tag caches, `dev_data/` with `devdata.sqlite`) | continuedev/continue `main` @ `5522c6f44ca0ac3528b37244818fbfa39b5af470`: `core/util/paths.ts` (the whole layout and the override), `core/index.d.ts` (`Session.workspaceDirectory` is a **required** field), `core/util/history.ts` (upstream itself writes `workspaceDirectory: ""` from the `catch` of `load(sessionId)`, so an empty string is an expected on-disk value and resolves to `unresolved`, never `missing`) |

### What the 2026-09-21 re-verification changed

Five rows carried a claim that was wrong rather than merely unconfirmed:

- **Cline** read `task_metadata.json`'s `workspace` field for project
  linkage. That field is in neither extension's schema, so it resolved
  nothing, ever -- while reporting an `unresolved` reason that named the
  wrong file. Cline tasks now say which store actually holds the answer.
- **Roo Code** had the same wrong file. It *does* record the workspace
  per task, in `history_item.json`, which the adapter now reads -- so Roo
  Code task linkage works for the first time.
- **Continue** said no per-session workspace field existed. One does, it
  is required, and the adapter now reads it from its bounded header.
- **OpenCode** claimed an `OPENCODE_DATA_DIR` override "honored
  defensively". No such variable exists upstream; the detector no longer
  looks for it.
- **Gemini CLI** described `tmp/<hash>`/`history/<hash>` as
  `sha256(project root)`. That is the legacy form; current versions use
  short registry slugs from `projects.json`. The adapter treats the name
  as opaque either way and does not read `projects.json`, which its
  `unresolved` reason now says.

Two rows lost their **supported** status because the layout they model is
not confirmed: **Cursor** (no official documentation names any of these
paths) and **Windsurf** (the profile root and `globalStorage` are now
officially documented, but `workspaceStorage` and the cache/log siblings
are not). Identification still runs for both; no action is offered and
linkage is `unresolved`.

Every row's citation was re-fetched at a pinned commit (or, for a
documentation page, a retrieval date) on **2026-09-22**, vendored as a
minimal excerpt under `crates/core/tests/fixtures/upstream/` with its
blake3 digest, and listed in that directory's `citations.toml`.
`crates/core/tests/upstream_citations_are_checked.rs` greps every
excerpt for the symbols its claim depends on and fails naming the
citation that does not support it. Before that, CI checked that the
`Verified against` cell was longer than thirty characters -- and six of
twelve `Supported` rows cited something that did not establish the
claim, with the suite green.

Cursor, Windsurf, Cline and Roo Code are macOS-only in this chunk, with
Linux paths deferred to the independent Linux track (#77-#89). Extending
this list for a tool *beyond* the fourteen rows above (Codex and its
desktop app count separately, per #93's "do not extrapolate one client's
schema to all clients" acceptance) is ordinary catalog review, not a
change to this contract.

## Categories

Every identified unit falls into one of these (`AgentCategory` in
`crates/core/src/agents/mod.rs`):

| Category | Meaning | Protected by default? | Supported action |
|---|---|---|---|
| Sessions | Transcripts and their directly-linked recovery material (companion subagent dir, checkpoints, todos, attachments) | Individually (e.g. `history.jsonl`) where an adapter says so | **Session removal** -- exact member set, reference-verified fresh at execution |
| Archived sessions | A session a tool itself moved out of its default/live listing (e.g. Codex's `archived_sessions/`) but did not delete -- kept distinct from Sessions so the two are never silently summed into one total a user cannot decompose against `du` (stack/26's Codex reconciliation defect) | Individually, same as Sessions | **Session removal**, same as Sessions |
| Attachments | Shared or per-session pasted/uploaded content | Often, when shared/not session-scoped | None yet |
| Checkpoints | File-history/backup material not currently linked to any live session | No (unless orphaned and adapter-flagged) | None yet |
| Caches | Regenerated-automatically technical caches | No | **Cache/log Trash move** |
| Logs | Regenerated-automatically debug/diagnostic logs | No | **Cache/log Trash move** |
| Managed worktrees | Git worktrees a tool created | N/A -- reuses existing worktree identity, never separately re-measured | Existing Git/worktree action protections apply, not this module's |
| Plugins | Marketplace configuration and downloaded plugin/skill code | No (the `.trash` staging subdirectories are actionable; the rest is not) | **Cache/log Trash move** for `.trash` only |
| Protected config | Credentials, settings, skills, commands, subagent/automation definitions | **Yes, always** | None -- never actionable |
| Protected databases | SQLite state stores and their `-wal`/`-shm` sidecars (e.g. Codex's `state_5.sqlite`, `logs_2.sqlite`, `thread_history_1.sqlite`) -- Codex's state DB is queried read-only for exact session linkage; no store is a cleanup target | **Yes, always** | None -- never actionable |
| Unclassified | Anything with no specific rule (a residual bucket, never silently dropped) | Case by case | None yet |

Only **cache/log Trash move** and **session removal** have a supported
selective action in this release. Everything else is identification
only: it appears in a report, it can be inspected, and it is
respected by `swamp protect`, but there is no code path that deletes
it. This matches the guardrail this feature was built under: identity
and action capability are independent facts, and the absence of one is
never disguised as the other.

`AgentMember.kind` (`AgentMemberKind`) gained two variants for #93/#94/
#95, both explicitly sanctioned by the handoff's "extend the shared
model only where a tool genuinely needs a new concept" clause:

- **`Database`** -- a SQLite file (or a `-wal`/`-shm` sidecar) backing
  a newer version's unified session/message/state store (Codex's seven
  `*.sqlite` files, OpenCode's `opencode.db`, Oh My Pi's `agent.db`).
  Always folded into one unit with its sidecars as members; never split,
  never actionable (`is_sqlite_like` in
  `crate::actions` refuses any selective action on a path with this
  kind unconditionally). Codex's state index alone is opened read-only
  for exact `rollout_path`/`cwd` linkage; no other columns or SQLite
  stores are queried.
- **`SessionData`** -- a session-keyed companion directory that is
  neither a transcript, a subagent dir, todos, file-history, nor an
  attachment (OpenCode's `storage/message/<session-id>/` and
  `storage/session_diff/<session-id>.json` -- a **file**, not a
  directory), matched to a session by the same exact-id-match
  discipline `claude_code::identify` uses for its own companions --
  never a guess.

#96/#97/#98/#99's nine adapters needed **no new `AgentMemberKind`
variant**: Cursor/Windsurf's `state.vscdb` and GitHub Copilot CLI's
`session-store.db` reuse `Database`; Cline/Roo Code's task directories
and Copilot CLI's/Continue's directory-shaped session entries reuse
`SessionData`; every single-file session (Gemini CLI's saved chats,
Aider's per-repo history files, Pi's/Continue's per-session files, Copilot
CLI's file-shaped session entries) reuses `Transcript` -- direct
evidence that the shared model already covered these tools' shapes
without needing to be widened.

### Cache/log Trash move

A supported single directory is moved into Trash as one unit, recoverable
until Trash is emptied. The move does not imply its contents are rebuildable:
cache recreation may require another tool run or a download; diagnostic logs
lose past records; command recall and editor undo snapshots lose local history.
New logs or history files do not recreate deleted records. Restore those from
Trash or a backup if needed. The preview states this distinction.

### Session removal

A session's exact member set -- the transcript itself, its companion
subagent directory, its `file-history/<session>/` checkpoint data, and
any `todos/`/`image-cache/`/`uploads/` entries matched to it -- is
moved together into one Trash envelope. This is **not** a
rebuildable-cache action: it discards unique resume/rewind/checkpoint
history for that session. The linked project's own files are never
touched -- only the tool's own record of the conversation is affected.
The TUI moves the member paths shown in its confirmation. It does not run
the retired CLI `execute` command or a post-mark membership recheck.
Stop the owning tool first if it may still be writing. Partial move failures
retain their per-member outcomes in the Trash envelope's `restore.json`.

### What is refused, always

- Whole tool-home deletion. An `AgentUnit` never represents the whole
  home; there is nothing to select that would delete it.
- The whole `projects/` directory, or a whole project's session
  directory as one unit -- only individual sessions are units.
- Any path whose filename looks like a SQLite database or its `-wal`/
  `-shm` sidecar. Claude Code has none currently documented, but the
  guardrail is unconditional regardless, and Codex/OpenCode/Oh My Pi
  each have real SQLite stores it actually protects (seven state
  databases, `opencode.db`, `agent.db`).
- A content-addressed blob Oh My Pi's `blobs/` shares across sessions:
  identified with its reference count (or an explicit "coverage
  unknown" note when any session's body exceeded this pass's bounded
  scan), but never offered a selective action at all in this release --
  reference-based GC from incomplete coverage is out of scope here, not
  merely gated.
- OpenCode's `snapshot/<project-id>/` git-backed checkpoint store and
  its `storage/part/` (message parts, keyed by message id, not session
  id): identified and linked where possible, never offered a selective
  action -- removing a snapshot loses `/undo` history, and parts cannot
  be safely correlated to one session without reading message content.
- Anything a human protected with `swamp protect add <path>`.

(2026-09-23: occupancy of an active session is still computed and
shown on the TUI's confirm banner as a fact, but no longer refuses a
move -- see `.oh/guardrails/occupancy-is-tristate-at-sinks.md`.)

## Project linkage

Each unit's `project_link` field is one of:

| State | Meaning |
|---|---|
| `linked` | Resolved to a swamp project/worktree identity (`crate::git`'s own object-store-based identity -- never a filesystem path or a basename match), with `declared` or `inferred` provenance. `declared`: the tool's own metadata named the path and it resolved to a checkout. `inferred` (Claude Code only, 2026-09-25): the session's `projects/<slug>` folder name re-encoded **exactly one** of this pass's known worktree paths (`[^A-Za-z0-9]` -> `-`, compared, never decoded), and that path still resolves to a checkout. It is used only when the declared `cwd` is absent, missing, or not a checkout; a successfully linked `cwd` always wins. The JSON retains the failed `cwd` reason in `fallback_reason`; the TUI labels the link inferred. It is re-derived against the current known worktrees on every pass -- never replayed from the store. Limits, stated rather than papered over: the encoding is lossy (`/a/b-c` and `/a-b/c` share a slug), so a slug matching two known paths is not used, and a session whose real path collides with a *different* known path that happens to be the only match cannot be told apart from a true match -- which is why the result is labelled inferred and never treated as declared. A path rename is not recovered (the old slug matches nothing known). |
| `unresolved` | No project linkage could be established; the reason distinguishes missing or invalid metadata and unavailable linkage sources. For Claude Code, a missing or non-project declared `cwd` may be rescued only by one exact full-path folder-slug match; when that fails, the original typed `missing`/`not-a-project` state remains. Ambiguous slugs never choose a candidate. Codex sessions have no folder signal (their directories are date-partitioned): linkage requires an exact `rollout_path` row in the versioned state DB and a unique absolute `cwd`. Missing, conflicting, stale, or unsupported DB rows stay unresolved; transcript contents are never scanned as fallback. |
| `missing` | Metadata names a path that no longer exists on disk. |
| `not-a-project` | Metadata names a path that exists but is not (or is no longer) a Git checkout/worktree. |
| `moved` | The declared path used to resolve to one project identity and now resolves to a different one (or none). *Not currently populated by the Claude Code adapter* -- it has no record of a session's previous linkage to compare against. |
| `remote` | The declared path is on a different host than this observation runs on. *Not currently populated* -- no reliable remote-host signal exists in a session header. |
| `shared` | A unit's members collectively name more than one project. *Not populated by the Claude Code, Codex or OpenCode adapters* (one session always has exactly one declared `cwd`/`worktree`); the Oh My Pi adapter does populate it, when a session's `additionalDirectories` names a workspace root resolving to a different project than its primary `cwd`. |
| `not-applicable` | This unit is inherently tool-wide (a cache, a log directory, protected config): project linkage does not apply, which is a different, more honest fact than "we tried and could not tell." |

Resolution never decodes the `projects/<encoded-cwd>` directory name
back into a path: that encoding is lossy in the reverse direction, so
it is used only by re-encoding the bounded set of known worktrees and
requiring exactly one match. A resolved transcript `cwd` remains the
authoritative link; folder evidence is explicitly inferred and retains
the failed cwd reason. No bare project-basename match is used.

Bidirectional navigation:

- **Project → agent storage:** `swamp report --view agents --project
  <name>` narrows to units whose linkage names that project.
- **Agent unit → project:** every unit's own `project_link` field
  names its project (or explains why it cannot).

Linked bytes are never added to the linked project's own filesystem
total: an agent unit lives under the tool's external-unit home, a
separate accounting basis, shown as a reference, not summed in.

## Claude Code (#92)

Layout researched from Anthropic's own documentation (linked in the
matrix above); the transcript JSONL schema itself is explicitly
documented upstream as internal and unstable across versions, so this
adapter reads at most one field (`cwd`) off a session's first line and
treats anything else as an unknown, never a parse panic or a guess.

- **Sessions:** `projects/<encoded-cwd>/<session-id>.jsonl`, its
  companion `projects/<encoded-cwd>/<session-id>/` directory
  (subagents/tool-results), `file-history/<session-id>/`, and matching
  `todos/`, `image-cache/<session-id>/`, `uploads/<session-id>/`
  entries. The `todos/` match is a **documented heuristic** (filename
  prefix match on the session id): the exact naming convention is not
  in Claude Code's official documentation. Session ids are UUIDv4, so
  collision risk is negligible; an ambiguous match is excluded rather
  than guessed. A `todos/` entry no session claims is folded into one
  `todos (unlinked)` unit -- previously those bytes were in no unit at
  all, and a test asserted the gap instead of closing it.
- **Caches (actionable):** `shell-snapshots/`, `plugins/.trash/`,
  `skills/.trash/`.
- **Legacy, no longer written (actionable):** `statsig/`, `logs/`, and
  any `todos/` entry no live session claims. The cited page documents
  all three in one row as "Legacy directories from older versions. No
  longer written", and answers "Nothing" in its own what-you-lose
  table. So removing them costs no regeneration -- they are residue,
  not a cache that will come back. This catalog previously called
  `statsig` "community-documented, not found in the official settings
  fetch used here" and modelled it as regenerating automatically; both
  halves were wrong, and the page it already cited said so.
- **Logs (actionable):** `debug/`.
- **Protected config:** `settings.json`, `.credentials.json`,
  `keybindings.json`, `themes/`, `rules/`, `skills/`, `commands/`,
  `agents/`, `workflows/`, `output-styles/`, `agent-memory/`.
- **Individually protected, not by category:** `history.jsonl` (every
  prompt typed, kept for recall/search -- category `sessions`, but
  flagged protected because its content is prompt text and it has no
  supported action).
- **Not modeled, a documented gap:** `~/.claude.json` lives *beside*
  `~/.claude/`, not inside it. An agent unit's identity is a path
  under the tool home by construction; a sibling file does not fit
  that model, so it is left out rather than forced in.
- **Not separately re-measured:** git worktrees Claude Code creates
  (`--worktree`, `EnterWorktree`, `isolation: worktree`). Claude Code
  documents no fixed on-disk location for them; they are ordinary Git
  worktrees the normal project scan already discovers and measures.
  This adapter cross-references via project linkage rather than
  inventing a second measurement of the same bytes.
- **Active-session detection:** an `lsof`-style occupancy check on a
  session's transcript file (`crate::occupancy::occupied`), run only
  when proposing/executing an action on that specific unit -- never
  during ordinary identification, which would mean hundreds of process
  spawns on an otherwise-cheap `report`.

## Codex (#93)

Session linkage is sourced from the versioned Codex state DB, not from
rollout files. Swamp resolves the SQLite home from `sqlite_home` in
`config.toml`, then `CODEX_SQLITE_HOME`, then the Codex home. It picks
the highest numeric `state_<n>.sqlite` filename, opens that database
read-only, verifies the `threads` schema, and selects only
`rollout_path` and `cwd`. Matching is by exact rollout path; basename,
thread title, prompt text, and Git origin are not substitutes. Missing
or conflicting rows remain unresolved. The DB is an index, not a
perfect ledger: sessions absent from its current table will remain
unlinked, and Swamp does not scan their transcript contents to fill the
gap. The normal stat-only walk still measures rollout file sizes. The
day-container cache stores that size/member inventory independently of
the index: Swamp reloads the small SQLite mapping on each pass and
refreshes project links on replay, without making an index update
invalidate and re-stat every session day.

The path and SQLite config sources are researched from `openai/codex`'s
own `codex-rs` source (current `main`; see the matrix above for exact
file links). The transcript envelope is deliberately irrelevant to
linkage now: no rollout line or field is parsed.

- **Sessions:** `sessions/<year>/<month>/<day>/rollout-<timestamp>-
  <thread-id>[_<rollout-id>].jsonl` -- one file per session, no
  documented companion directory, so a session's member set is always
  exactly that one file.
- **Archived sessions** (category `archived-sessions`, distinct from
  `sessions` since stack/26): `archived_sessions/` in the same
  date-tree shape, identified the same way with an explicit "archived,
  not evidence of disuse" note -- archiving is a Codex-side visibility
  change, not a deletion. Reported under its own category rather than
  folded into `sessions` so the two are never silently summed into one
  total a user cannot decompose against `du` (the reconciliation defect
  this chunk fixed: `--view agents` had shown a "sessions" total that
  was actually live + archived + every SQLite store combined).
- **SQLite state stores (a version boundary), category
  `protected-databases` (not `sessions`, since stack/26):**
  `state_5.sqlite`, `logs_2.sqlite`, `goals_1.sqlite`,
  `memories_1.sqlite`, `memories_v2_1.sqlite`, `queue_1.sqlite`,
  `thread_history_1.sqlite`, each folded with its `-wal`/`-shm`
  sidecars into one protected, non-actionable unit. That is **seven**,
  which is what upstream's `const RUNTIME_DBS: [RuntimeDbSpec; 7]`
  declares; this catalog modelled six, because `memories_v2_1.sqlite`'s
  filename is an inline literal rather than one of the six
  `*_DB_FILENAME` constants, so it was neither folded with its
  sidecars nor protected. The sidecars are also excluded from the
  unclassified residual now -- previously their bytes were counted
  twice, once in the store's unit and once there. The linkage query
  chooses the highest numeric `state_<n>.sqlite`; future numbered state
  DBs are protected and folded too. `CODEX_SQLITE_HOME` or configured
  `sqlite_home` can relocate all seven *outside* `CODEX_HOME`; the
  lookup follows that setting. Relocated stores are not included in the
  Codex-home size breakdown by this adapter.
- **Protected config:** `config.toml`, `auth.json`, `skills/` --
  upstream comments this last one "Deprecated user skills location" and
  puts the current root at `~/.agents/skills`, which is outside
  `CODEX_HOME` and is not modeled here.
- **Plugins (category `plugins`, since stack/26):** `plugins/`, the
  installed-plugin cache (`~/.codex/plugins/cache/<marketplace>/
  <plugin>/<version>/` per developers.openai.com/codex/plugins/build).
  Previously fell into the unclassified residual.
- **Individually protected, not by category:** `history.jsonl`
  (cross-session prompt history, category `sessions`).
- **Logs (actionable):** `log/` (name carried over from this epic's
  prior research, not independently re-confirmed by source this
  chunk).
- **Not confirmed, not modeled:** no managed-worktree creation by the
  CLI itself was found in this chunk's source research, so
  `AgentCategory::ManagedWorktrees` is never populated by this adapter
  -- an honest absence, not a silent gap.

### Codex desktop app (#93)

The desktop app (`Codex.app`, bundle id `com.openai.codex`) is a
materially different client with its own storage; this chunk does
**not** extrapolate the CLI's `CODEX_HOME` schema onto it. Only its log
directory is confirmed by primary source
(`codex-rs/cli/src/doctor/desktop.rs`'s `desktop_log_root` -- the matrix row
used to cite `doctor/desktop/platform.rs`, which contains no `log_root`
at all): macOS `~/Library/Logs/com.openai.codex`, itself a `%Y/%m/%d`
date tree. That
directory is identified as one folded, actionable Logs-category unit.
Settings/session storage beyond logs is not confirmed and is not
modeled -- a deliberately partial `Supported` row, stated explicitly
rather than silently treated as empty.

`desktop_log_root` matches exactly two platforms and returns `None` for
everything else, so **Windows is confirmed too**:
`%LOCALAPPDATA%/Codex/Logs`, falling back to
`%USERPROFILE%/AppData/Local/Codex/Logs`, day-partitioned the same way.
This document used to say no Windows desktop build was confirmed; the
source it cited confirms it, and **Linux** is the only genuinely
unconfirmed platform. The Windows root belongs to the Windows track, so
this detector still reports `not-present` there -- an unimplemented
platform, which is a different statement from an unknown path.

## Oh My Pi (#94)

User-confirmed identity: a fork of `badlogic/pi-mono`'s `pi` coding
agent. Layout researched from `can1357/oh-my-pi`'s own docs (current
`main` as of this chunk).

- **Unknown-format disambiguation:** `~/.omp` is also a plausible home
  for unrelated tools (the issue names oh-my-posh as one to check).
  Resolving `~/.omp/agent` specifically (not the bare `~/.omp` wrapper)
  already avoids most collision risk, and this adapter adds a second,
  independent check on top: before identifying anything, it looks for
  at least one of this format's own content markers (`config.yml`,
  `config.yaml`, `agent.db`, `sessions/`, `blobs/`). Absent all of
  them, it reports one non-actionable "unknown format" unit for the
  whole directory rather than guessing.
- **Sessions:** `sessions/<encoded-cwd>/<timestamp>_<session-id>.jsonl`
  -- files begin with a fixed 256-byte `type: "title"` slot, then the
  session header (`cwd`, `additionalDirectories`); this adapter skips
  the title slot and reads only the header line, same one-field-at-a-
  time discipline as every other adapter here.
- **Shared content-addressed blobs:** `blobs/<sha256>`, referenced from
  session bodies as `blob:sha256:<hash>`. Establishing *complete*
  reference coverage would mean reading every session body in full,
  which this adapter deliberately does not do: each session's body is
  scanned only up to a bound (64 KiB), extracting reference tokens
  only -- never persisted or logged as text. A session exceeding the
  bound marks the whole pass's blob-reference coverage as unknown
  rather than reporting a possibly-wrong count. No blob is ever offered
  a selective action in this chunk regardless of its reference count --
  reference-based GC is out of scope here, not merely gated.
- **Terminal breadcrumbs (actionable):** `terminal-sessions/`.
- **Protected config:** `config.yml`/`config.yaml`, `models.yml`,
  `agent.db` (a SQLite auth store, doubly protected -- by category and
  by `is_sqlite_like`).
- **Project-local, not modeled:** `<cwd>/.omp/config.yml` lives outside
  the agent home entirely (same class of documented gap as Aider's own
  project-local files in the matrix above).

## OpenCode (#95)

Layout researched from `sst/opencode`'s own source and DeepWiki-indexed
documentation (current as of this chunk). Unlike the other three tools,
OpenCode keeps data, config and cache as three *independent* roots
(`crate::locations::opencode`); only the data root is decomposed into
`AgentUnit`s -- config and cache are reported as opaque external units
with their own byte totals, since neither carries session/project
linkage.

- **Version-aware boundary:** `identify` checks for
  `opencode.db`/`storage/`/`snapshot/`/`auth.json`/`log/` before doing
  anything else. A resolved, non-empty data root matching none of them
  reports one "unsupported layout version" unit rather than guessing
  at either schema below.
- **Newer/SQLite layout:** `opencode.db` (+ `-wal`/`-shm`), folded into
  one protected, non-actionable unit -- never opened. Codex's state DB
  is the narrow exception elsewhere: read-only linkage metadata query
  of exactly `rollout_path` and `cwd`.
- **Older/file-tree layout:** `storage/session/<project-id>/
  <session-id>.json`, with `storage/message/<session-id>/` (a
  directory) and `storage/session_diff/<session-id>.json` (a **file**)
  companions matched by the exact
  session-id-keyed discipline `claude_code::identify` uses for its own
  companions. `storage/part/<message-id>/*.json` is keyed by *message*,
  not session, id and is never correlated to individual sessions
  without reading message content -- folded whole into one protected,
  non-actionable unit instead, the same honest-gap discipline Claude
  Code's own `paste-cache` uses.
- **Project linkage, declared and cheap:** `storage/project/
  <project-id>.json`'s `worktree` field is a real filesystem path, read
  once per project directory and reused for every session under it --
  no session-body scan needed at all for this adapter's project
  linkage, unlike every other adapter in this document.
- **Git-backed checkpoint snapshots (both layouts):**
  `snapshot/<project-id>/<hash>` -- an internal git object store,
  decoupled from the project's own `.git`, capturing a tree snapshot
  before/after every agent step so `/undo` can revert. Unique
  checkpoint/recovery state, identified and linked, never offered a
  selective action in this chunk.
- **Protected config:** `auth.json` (directly under the data root, not
  the separate config root).
- **Logs (actionable):** `log/` (also directly under the data root).

## Gemini CLI (#96)

Layout researched from `google-gemini/gemini-cli`'s own docs and source
(current `main` as of this chunk). The project-hash algorithm
(`getProjectHash(projectRoot) = sha256(projectRoot).hex()`) is confirmed
directly in `packages/core/src/utils/paths.ts` -- not guessed -- but it
is one-way: this adapter has no candidate project-path catalog to hash
and compare against, so every `tmp/<hash>`/`history/<hash>` unit's
`project_link` is `unresolved`, naming the algorithm explicitly, rather
than a fabricated match or a silently dropped fact.

- **Protected config:** `settings.json`, `GEMINI.md`, `trustedFolders.json`,
  `extensions/`. OAuth/account credential file names are not documented
  on any reachable page this chunk (`docs/cli/authentication.md` and
  `docs/get-started/authentication.md` both 404 against current `main`),
  so any top-level file whose name contains `oauth`/`cred` is protected
  defensively by filename pattern instead of an exact confirmed name.
- **Caches (actionable):** `tmp/bin` (downloaded runtime tools, e.g.
  LiteRT-LM). Not `~/.gemini/bin`: upstream builds it as
  `getGlobalBinDir() = join(getGlobalTempDir(), 'bin')`, so the path
  this adapter used to look at does not exist in any version.
- **Per-project-hash `tmp/<hash>/`:** `shell_history` (Logs, actionable),
  `checkpoints/` (Checkpoints, not actionable -- tool-call recovery
  state for `/restore`), `chats/*` (Sessions, one unit per saved chat
  file, actionable -- `/chat save`/`/resume`).
- **`history/<hash>/`:** a shadow Git repository, independent of the
  project's own `.git`, backing the same `/restore` checkpoints. Unique
  recovery state, identified and linked (honestly unresolved), never
  offered a selective action this chunk.

## Pi (#96)

`badlogic/pi-mono`'s `coding-agent` package (also published as
`earendil-works/pi`) -- **distinct from Oh My Pi**, which is a fork of
it, even though both currently document the same override variable name
(`PI_CODING_AGENT_DIR`, confirmed by this chunk's own primary-source
read of Pi's README, correcting both this issue's own `PI_AGENT_DIR`
guess and superseding reliance on Oh My Pi's docs alone). If a human
sets that variable while both tools are installed, both detectors
resolve to the same path -- a disclosed, not silently patched,
limitation (see `crate::locations::pi`'s doc comment).

- **Explicit format/version detection:** Pi's own README documents
  session files only as JSONL with `id`/`parentId` tree structure -- it
  does **not** document Oh My Pi's 256-byte title slot. This adapter
  therefore parses **only** Pi's own offset-zero JSON header, and reports
  an explicit unknown-format outcome (`unresolved`, naming the shape it
  expected) when that does not match.

  It used to fall back to Oh My Pi's title-slot shape "as an explicit
  fallback (reused, not assumed)". That was adapter-to-adapter
  knowledge: a change to Oh My Pi's format would have changed *Pi's*
  identification, for no reason a reader of either file could see. The
  shared byte-offset mechanics now live in the neutral
  `crate::agents::pi_family`, which names no tool, and each adapter
  passes only the layouts its own tool documents
  (`.oh/guardrails/agent-adapters-are-pluggable.md`).
- **Sessions:** `sessions/`, organized by working directory per Pi's own
  docs; this adapter does not decode a directory name into a project
  path (no encoding scheme is confirmed), relying only on each session
  file's own declared `cwd`.
- **Protected config:** `settings.json`, `trust.json`, `models.json`.
- **Caches (actionable):** `npm/` (user-scoped package installs,
  reinstallable).

## Aider (#96)

Materially different shape from every other tool in this catalog: most
of Aider's storage is **not** under any tool home at all.
`aider/args.py`/`repomap.py` (current `main` of `Aider-AI/aider`) place
`.aider.chat.history.md`, `.aider.input.history` and
`.aider.tags.cache.v{3,4}/` at each project's own git root. Per this
issue's explicit acceptance, these are attached to the existing
worktree/project model as an agent category -- **not** modeled as
tool-home units -- via `crate::agents::aider::identify_repo_units`,
called once per known project worktree root by
`crate::agents::discover_and_measure`'s `project_worktrees` parameter
(itself built from the already-loaded `Report`'s projects/worktrees at
the two read call sites, or, for `swamp propose-agents --path` which
computes no report, by walking upward from each requested path for its
own `.git` root).

- **Home-level (`~/.aider`):** `caches/model_prices_and_context_window.json`
  and `caches/versioncheck` (both wholly re-downloadable, confirmed
  directly in `aider/models.py`/`versioncheck.py`), plus an optional
  home-level `.aider.conf.yml`.
- **Per-repo (project-linked, `Sessions` category):**
  `.aider.chat.history.md`, `.aider.input.history` -- unique, not
  regenerated by re-running Aider.
- **Per-repo (project-linked, `Caches` category):**
  `.aider.tags.cache.v{3,4}/` (the version number reflects whether the
  optional TSL pack is in use; both are checked directly, not guessed at
  one fixed number) -- regenerated on the next Aider run.
- Disabling the `aider` detector (`disabled_detectors`) turns off *both*
  halves together, home-level and per-repo, so a human's "stop looking
  at this tool" always means the whole tool.

## GitHub Copilot CLI (#97)

Directory layout sourced directly from GitHub's own reference page,
re-pinned 2026-09-22 to `github/docs` @
`72e940d15a9aff06b6e84216f3c97dac25c47d9b`, correcting this issue's own
guessed directory name (`history-session-state/`, which upstream's
changelog shows is the *pre-0.0.342* name, migrated on `--resume`) to
the real `session-state/` and `command-history-state/`.
The [current directory reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)
documents session-ID directories containing `events.jsonl` and workspace
artifacts. Each such directory is an exact local-session Trash selection;
unknown project linkage does not prohibit choosing it. This loses local
conversation/checkpoint history, not a rebuildable cache. Remote copies,
if any, are outside this action.

- **Protected config:** authentication, settings, instructions, extensions,
  hooks and secrets remain protected.
- **Sessions:** immediate directories with `events.jsonl` support removal.
  Unrecognized files/directories remain inspection-only.
- **Project linkage:** one bounded, cached read of `workspace.yaml` extracts
  its top-level absolute `cwd` in supported single-line scalar forms.
  Missing or unfamiliar metadata remains unresolved. No transcript search
  or arbitrary adjacent JSON scan occurs. The SDK tests establish the file
  ([pinned source](https://github.com/github/copilot-sdk/blob/4001c1da7d832c51bad1d38619c1a082af390efb/nodejs/test/e2e/session_fs.e2e.test.ts));
  upstream reports show the on-disk field in CLI 1.0.14
  ([#2446](https://github.com/github/copilot-cli/issues/2446),
  [SDK #1735](https://github.com/github/copilot-sdk/issues/1735)).
  These are format evidence, not a guarantee for every CLI version.
- **`session-store.db`** (+ `-wal`/`-shm`): protected, non-actionable,
  same discipline as every other tool's cross-session SQLite store.
- **Local history (actionable):** `command-history-state/` (reverse-search
  command recall, not conversation content).
- **Logs (actionable):** `logs/`.
- **`ide/`** (IDE integration state/lock files): identified but never
  actionable this chunk -- a lock file backing an active integration is
  a real corruption risk, and no documented signal distinguishes an idle
  entry from a live one.
- **Cache, separately:** platform-conventional (`~/Library/Caches/copilot`
  on macOS), independent of `COPILOT_HOME`, overridable via
  `COPILOT_CACHE_HOME`; reported as an opaque external unit.

## Cursor and Windsurf (#98)

Both use the shared `crate::agents::vscode_family` module (Cursor and
Windsurf are VS Code forks with the same underlying storage
conventions -- one real implementation, per the handoff's "extend the
shared model only where a tool genuinely needs a new concept").
Cursor's layout is community-reverse-engineered only: re-searched
2026-09-22 across `cursor.com/docs/llms.txt` and the agent/configuration
pages with zero hits for `state.vscdb`, `workspaceStorage`,
`globalStorage` or `Application Support`, and Cursor is closed source,
so there is no repository to pin. Windsurf's **profile roots** are
primary-source confirmed -- `docs.windsurf.com` 307-redirects to
`docs.devin.ai`, whose desktop FAQ names the rename and both locations
(`~/Library/Application Support/Windsurf` legacy -> `.../Devin` current)
along with `User/settings.json`, `User/keybindings.json`,
`User/snippets/`, `globalStorage/`, `Workspaces/` and `argv.json`. What
that page does **not** name is `workspaceStorage/` or the
`Cache`/`CachedData`/`CachedExtensionVSIXs`/`logs` siblings, which is
why the row stays `unverified`. Both roots are modeled, because an
installation mid-migration has bytes in each. Both tools are macOS-only
this chunk; Linux is the independent Linux track's job (#77-#89).

- **`User/globalStorage/state.vscdb`** (+ `-wal`/`-shm`): protected,
  metadata-only, never opened while writable -- holds every project's
  chat/composer content (`ItemTable`/`cursorDiskKV` key-value stores).
- **`User/workspaceStorage/<id>/state.vscdb`**: a per-workspace index
  (not the content itself), linked via the sibling `workspace.json`'s
  `folder` field (a real `file://` URI the editor itself wrote, not a
  basename guess) -- also protected and non-actionable, but carries real
  project linkage as identification evidence.
- **`User/History/`** (actionable): local file-history/undo snapshots,
  unrelated to AI chat content.
- **`Cache/`, `CachedData/`, `CachedExtensionVSIXs/`** (actionable
  caches) and **`logs/`** (actionable log): Electron-conventional
  siblings of `User/`.
- **Unrecognized layout:** one explicit "(unsupported layout version)"
  residual, never a guess -- this is how a Windsurf installation that
  does not actually match the assumed VS-Code-fork shape shows up,
  rather than a silent miscount.
- Cursor's separate `~/.cursor/` and Windsurf's `~/.codeium/windsurf`
  are reported as opaque external units, not decomposed (their interior
  shape is not documented). `~/.codeium/` is *current*, not legacy: the
  FAQ states that tree is not changing in the rename and stays
  read-write.

## Cline, Roo Code and Continue (#99)

Cline and Roo Code are VS Code **extensions** (not forks): the same
extension id can be installed into several editor hosts at once, each
with its own, genuinely separate, on-disk `globalStorage`. Per this
issue's explicit acceptance, `crate::locations::vscode_hosts` proposes
one location *per known host* (Code, Code Insiders, Cursor, Windsurf,
plus `~/.vscode-server` for a remote/devcontainer target), and
`crate::agents::discover_and_measure` decomposes *every* resolved
location for these two tool ids (`multi_location_tool`), unlike every
other tool in this catalog, which only decomposes the first. Both
adapters reuse `crate::agents::vscode_family::identify_extension_globalstorage`,
which tags every unit's `relative_path` with its host label (e.g.
`"VS Code/tasks/<id>/..."`, `"Cursor/tasks/<id>/..."`) so two hosts'
task directories -- which can share the exact same UUID-shaped name --
never collide in identity or display.

- **Sessions:** `globalStorage/<extension-id>/tasks/<task-id>/`, folded
  as one unit per task. Project linkage is **per extension**, because
  the two extensions do not record it in the same place -- so the shared
  module takes a declared `TaskLinkSource` from each adapter rather than
  assuming one field name for everybody:
  - **Roo Code** declares `tasks/<id>/history_item.json`'s `workspace`
    field, confirmed in `src/core/task-persistence/TaskHistoryStore.ts`.
  - **Cline** declares a *shared history file*:
    `{globalStorage}/state/taskHistory.json`, an array of `HistoryItem`
    keyed by task id whose `cwdOnTaskInitialization` field is the
    working directory. It is read **once per host**, not once per task,
    through the same capped reader as everything else; a history longer
    than the bound reports the tasks past it as truncated rather than
    as declaring nothing. This catalog previously said the answer lived
    "inside `state.vscdb` -- the SQLite store this catalog refuses to
    open", which was wrong: the file sits inside the directory the
    adapter already walks. `cwdOnTaskInitialization` is optional
    upstream, so an absent entry is still `unresolved`, never a guess
    from the task id.

    **Upstream spells the store three ways at the same commit** --
    `{globalStorageFsPath}/state/taskHistory.json` (the migration
    file's comment), `tasks/taskHistory.json` (the same file's skip
    list) and `~/.cline/data/tasks/taskHistory.json`
    (`.clinerules/storage.md`). This catalog follows the executable
    code (`state/`) and records the ambiguity in the `unresolved`
    reason rather than asserting one path. The `~/.cline/data` root is
    modeled separately, and upstream states it does *not* hold the VS
    Code host's task history.

  Both adapters previously read `task_metadata.json`'s `workspace`
  field. That field exists in neither extension's schema, so it resolved
  nothing, ever, while reporting a reason that named the wrong file.
- Roo Code's own community reports note a task directory can embed a
  full Git checkpoint repository, so its folded byte total is not
  necessarily small the way a Claude Code session usually is --
  `SessionRemoval`'s existing generic loss warning already covers this.

Continue is unrelated to the VS Code storage conventions above --
its own `~/.continue` home, config confirmed by primary docs:

- **Protected config:** `config.yaml`/`config.json`.
- **Sessions (actionable):** `sessions/<id>` -- one unit per entry,
  excluding index-like filenames (`sessions.json`/`index.json`), which
  are protected separately instead (removing the index alongside a kept
  session would otherwise corrupt it for every session that remains).
  Linkage comes from `Session.workspaceDirectory`, a **required** field
  in `core/index.d.ts` which the adapter reads from the bounded header.
  Upstream itself writes `workspaceDirectory: ""` from the `catch` of
  `load(sessionId)`, so an empty string is an expected on-disk value:
  it resolves to `unresolved`, never `missing`, because `missing` would
  claim a path was named and has since disappeared. A field past the
  bounded read's cap is also `unresolved`, never a guess from the
  session id.
- **Caches (actionable):** `index/` (embeddings/tag caches).
- **Logs (actionable):** `dev_data/` (anonymized usage events).
- `sessions/index/dev_data`'s presence is treated as a version marker
  (this catalog's own prior research, not independently re-confirmed
  this chunk) rather than an asserted schema.

## Scan cost

Identification reads directory names and bounded metadata (file
`stat`, and adapter-specific small metadata sources). Codex session
linkage queries two columns in its local SQLite index and reads zero
rollout transcript bytes; the session tree is still stat-walked for
storage totals. It never uses a full
directory content walk with `crate::walk::resize_artifact`'s
parallel-pool machinery, which is tuned for a handful of potentially
huge artifact roots, not hundreds of small per-session directories.
Each adapter has its own `identification_cost_is_bounded_for_many_sessions`
test, measuring identification of 500 synthetic sessions and asserting
completion in well under 10 seconds:

| Adapter | Measured (this chunk's dev machine) |
|---|---|
| `claude_code` | ~215ms (500 synthetic sessions, 200KB bodies each) |
| `codex` | ~209ms |
| `oh_my_pi` | ~368ms (includes the bounded per-session blob-reference scan) |
| `opencode` | ~6ms |
| `gemini_cli` | ~161ms (300 synthetic project-hash directories) |
| `pi` | ~203ms |
| `aider` | ~6ms (`identify_repo_units` over a 2000-file tags cache) |
| `copilot_cli` | ~172ms |
| `vscode_family` (Cursor/Windsurf/Cline/Roo Code) | ~132ms (500 synthetic tasks) |
| `continue_dev` | ~5ms |

See `.oh/sessions/2026-09-21-agent-storage-claude-code.md` for the
Claude Code number's original recording,
`.oh/sessions/2026-09-21-agent-storage-codex-omp-opencode.md` for
Codex/Oh My Pi/OpenCode, and
`.oh/sessions/2026-09-21-agent-storage-remaining-tools.md` for the nine
tools this chunk added.

## Interfaces

| Surface | Command |
|---|---|
| CLI text | `swamp report --view agents [--project NAME] [--all]` |
| CLI JSON | `swamp report --view agents --json` (`{units, total_bytes}`) |
| TUI (read) | `v` (cycle) reaches the Agents view; no dedicated digit (`0` is "clear filter") |
| TUI (act) | `Space`/`Backspace` mark the selected agent unit and open the confirm banner (`App::mark_row`'s agent-storage branch); `Shift+A` (`mark_all_in_view`) marks every markable row in the Agents view the same way, skipping protected/unmarkable ones and naming the skip in the footer; `Enter` moves it to the Trash through the ordinary background-worker path (`execute_plan_progress`), never blocking the event/render thread. A protected row, or one whose category has no Trash move, cannot be marked; the footer names `propose_agents`'s own refusal reason. |
| Protect | `swamp protect add\|remove\|list [--json] <path>` |
| Removal | TUI only (2026-09-23: there is no `propose`/`propose-agents`/`approve`/`execute` command any more). |
| Skill | `skills/swamp/references/agent-storage.md` |
| Project tree | `swamp report --project <name>` (text) and `--project <name> --json` (no `--view` needed) both include this project's linked agent storage: a collapsed "Agent storage (linked)" row per contributing tool in the text tree (`crate::tree::agent_rows_for_project`, shared by the CLI drill and the TUI's own Tree view), and an `agent_storage: {units, total_bytes}` object in the JSON envelope. |

## Known gaps, recorded rather than hidden

- Every named tool in the matrix now has real identification code (see
  the matrix above) -- the epic's full-catalog acceptance is met at the
  identification/project-linkage layer; independent validation (#102)
  is still a separate, unchecked box.
- TUI bulk marking (`Shift+A`, `mark_all_in_view`) now recognizes agent
  rows too (reusing `App::mark_row`'s own per-row protected/unsupported/
  active refusal, never a duplicated refusal path): the actionable rows
  in view are marked, and a footer names how many were skipped and why
  when at least one was. Marking one agent unit at a time
  (`Space`/`Backspace` on the selected row) still works exactly as
  before.
- **Resolved this chunk:** `swamp propose --path` (and the deprecated
  `propose-agents` alias) used to skip computing a full `Report` for
  speed, discovering Aider's per-repo units only by walking upward from
  each requested path's own worktree root
  (`crate::agents::worktree_root_containing`). That meant a path whose
  worktree root was not itself part of the request could not surface an
  Aider unit at all. The unified `propose --path` route now runs the
  same real report walk `swamp report --view agents` does (every known
  project worktree, not just the ones implied by the requested paths)
  before resolving agent units, closing this gap at the cost of a full
  scope walk instead of a handful of `stat`s -- deliberate, since
  `propose` without a `root` is not a hot path. See
  `discover_agent_units_for_propose` in `crates/cli/src/main.rs`.
- Windsurf's editor-profile shape and the `task_metadata.json`
  `workspace` field Cline/Roo Code used for project linkage were this
  chunk's own best-available research. **Both were re-checked against
  upstream on 2026-09-21 and neither survived intact**: the
  `task_metadata.json` field does not exist in either schema (Roo Code's
  is in `history_item.json`; Cline's is not in any file this catalog
  reads), and Windsurf's `workspaceStorage` is still not named by the
  official documentation that now covers the rest of that profile. See
  the support matrix for the citations. Windsurf and Cursor are
  `unverified` as a result.
- Cursor, Windsurf, Cline and Roo Code are macOS-only this chunk; their
  Linux paths are the independent Linux track's job (#77-#89), not
  re-derived here as a guess.
- **Resolved this chunk:** `swamp propose`'s `root` is now `Option<PathBuf>`.
  Omitting it and passing `--path` routes to the agent-storage proposer
  or the external-unit proposer automatically (see the Interfaces table
  above); `--external` forces the latter explicitly. `propose-agents`
  remains only as a thin, deprecated alias so existing scripts keep
  working.
- Plugins/marketplace removal beyond the `.trash` staging directories
  is not supported; native marketplace-aware removal is the preferred
  future mechanism, not a guessed directory delete.
- `paste-cache/` is treated conservatively (protected, no action) even
  though Claude Code's own retention policy treats it as ephemeral,
  because it is not scoped to one session and this adapter has no
  per-session reference evidence for it.
- Oh My Pi's shared blobs and OpenCode's git-backed snapshots/`storage/
  part` are identified and linked where possible but never offered a
  selective action in this chunk (see their sections above) -- this is
  a deliberate scope boundary, not an oversight: reference-based blob
  GC and per-hash snapshot removal both need reference/coverage
  guarantees this chunk does not implement.
- Both of the env var names this list once called unconfirmed were
  checked against upstream source on 2026-09-21 and are now settled:
  Codex's `CODEX_SQLITE_HOME` is real (`codex-rs/state/src/lib.rs`,
  openai/codex `main` @ `30daed37ad8035f041f65a4c4615fbc590dc8552`),
  and OpenCode's `OPENCODE_DATA_DIR` is **not** -- no such variable
  exists (`packages/core/src/global.ts` and
  `packages/core/src/flag/flag.ts`, sst/opencode `dev` @
  `fe3f3a41f79ad292cc3c7c629567385a20ec5130`), so the detector no
  longer looks for it and the data root is `$XDG_DATA_HOME/opencode`
  unconditionally. Earlier chunks corrected two more env var guesses
  against primary source:
  Gemini CLI's real override is `GEMINI_CLI_HOME` (not
  `GEMINI_CONFIG_HOME`), and Pi's real override is `PI_CODING_AGENT_DIR`
  (not `PI_AGENT_DIR`, which is still honored defensively as an
  unconfirmed secondary override).
- GitHub Copilot CLI's `ide/` and Aider's home-level `.aider.conf.yml`/
  residual entries are identified but never actionable this chunk (the
  former: real corruption risk from touching an active integration's
  lock file with no documented idle signal; the latter: simply out of
  named scope) -- an explicit `None` action, not a missing feature
  disguised as empty.

## #100/#101 completion and #102 validation (this chunk)

This chunk audited every #100/#101 acceptance line against the
implementation left by the prior four chunks (Claude Code; Codex/Oh My
Pi/OpenCode; the remaining nine tools), fixed the gaps it found, and
added the independent #102 validation pass. Landed:

- **Project-tree agent linkage (#100 gap).** `crate::tree::ProjectTree`
  gained `agent_rows: Vec<ProjectAgentToolRow>` (`crate::tree::
  agent_rows_for_project`), a per-tool collapsed summary of the agent
  storage linked to one project. `render_project_tree_with_agents`
  (CLI text) and `model::tree_rows_with_agents` (TUI Tree view) both
  render one "Agent storage (linked)" line per contributing tool,
  informational only (`unit: None` in the TUI -- acting on a specific
  unit stays the dedicated Agents view's job, with its own per-unit
  protections/occupancy checks). Previously this information existed
  only in the flat `--view agents` list, with no way to see "does this
  project have any linked agent storage" from the project drill itself.
- **`report --project NAME --json` agent linkage (#100 gap).** The
  project-scoped JSON envelope (the `None`-view branch of
  `report_json_envelope`) now includes an `agent_storage: {units,
  total_bytes}` object whenever agent units were computed, not only
  when `--view agents` was also passed. `agent_units` is now computed
  whenever `view == Some(View::Agents) || project.is_some()`.
- **Unified `propose` entry point (#101).** `swamp propose`'s `root` is
  now `Option<PathBuf>`; a `--path` with no `root` is routed to the
  agent-storage proposer (if it matches a discovered agent unit), else
  the external-unit proposer, else refused by name (never silently
  falling through to a filesystem interpretation it cannot support
  without a root). `--external` forces the external-unit route
  explicitly, satisfying the "propose --external ... inspection-only
  refusal at execution" requirement B2 had left at the Rust API level
  with no CLI surface. `propose-agents` is now a thin, deprecated alias
  that delegates into the identical routing function
  (`propose_unified` in `crates/cli/src/main.rs`) and prints a one-line
  deprecation note.
- **Aider discovery fixed in the unified route (chunk E follow-up).**
  The old `propose-agents --path` fast path discovered Aider's per-repo
  units only by walking upward from the requested paths themselves,
  which could not find an Aider unit whose worktree root was not
  implied by the request. The unified route now runs the same real
  report walk `report --view agents` uses to supply every known project
  worktree first.
- **Refusal matrix hardening (#101).** `propose_agents` now refuses an
  agent-storage plan whose selected units' own paths nest (parent/child
  overlap), the same discipline `propose`'s Cargo-group check already
  applied to filesystem units -- previously nothing enforced this for
  agent units. A table-driven test
  (`crates/core/tests/agent_refusal_matrix.rs`) exercises every named
  refusal reason (protected category, human protect flag, unsupported
  action, database-like path, active session, whole-home/whole-
  projects-dir path, overlap, plan scope drift at execution) across all
  14 named tool ids.
- **Recoverable moves and partial-failure accounting (#101).** Session
  removal now writes a `restore.json` recovery manifest into its Trash
  envelope before any member moves, and rewrites it after each
  successful move -- so a partial failure (some members moved, then a
  later rename fails) leaves an accurate, on-disk record of exactly
  what happened, and `execute`'s own outcome now names the envelope and
  the bytes that really did move (`PartialAgentRemoval` in
  `crates/core/src/actions.rs`) instead of only a bare error string. All
  agent-storage actions remain Trash moves (recoverable), never a
  permanent native operation.
- **#102 validation suite.** `crates/core/tests/agent_storage_validation.rs`
  (custom-root redirection, malformed/truncated metadata across three
  adapters, unknown-schema-never-actionable across three adapters,
  shared-blob reference-state correctness, an integrated canary sweep
  across render text/JSON/plan/execute-result/ledger for two
  differently-shaped adapters, nested-accounting agreement between the
  flat Agents view and the project tree, incremental/unchanged-refresh
  growth history, and stable history after relinking a session to a
  different project) and
  `crates/tui/tests/agent_storage_validation.rs` (a real, on-disk
  fixture run through real identification into the TUI's own rendered
  frames -- both the Agents view and the new project-tree row -- to
  prove real session content never reaches what a human actually sees,
  not just the JSON/plan/ledger surfaces the prior chunks' tests
  already covered).
- **Benchmarks** (recorded in
  `.oh/sessions/2026-09-21-agent-storage-validation.md`): 300 synthetic
  Claude Code sessions (each padded past 200 KB so only a bounded read
  is ever exercised) scan in tens of milliseconds; an unchanged refresh
  and a one-session append cost about the same as the initial scan
  (this adapter re-scans its tool home each call rather than
  incrementally diffing it -- see that session note for why this is an
  honest, not-yet-optimized fact, not a regression).

## Human review still needed

Per this chunk's own instruction not to claim human review happened:
**no human has reviewed any of the following.** They are exactly what
Muness (or another maintainer) should check before treating this
feature as fully validated, carried forward from prior chunks' session
notes and `FOLLOWUPS.md` rather than silently resolved by guessing:

- **Retention-consequence wording.** Do the loss-of-resume/rewind
  warnings (`unit_from_agent`'s `warnings` in `crates/core/src/actions.rs`,
  and this doc's own "Session removal" section) actually read as clear
  and honest to a human deciding whether to delete a session, or are
  they too technical, too reassuring, or missing a consequence a real
  user would care about? Not evaluated by any automated test.
- **Format assumptions, as of the 2026-09-21 re-verification.** The
  previous tracking note read: "Windsurf layout assumed; Cline/Roo Code
  `task_metadata.json` workspace field unconfirmed; Gemini OAuth
  credential filename unconfirmed; Cursor/Windsurf/Cline/Roo Code
  macOS-only paths (Linux track)." Three of the four were resolved, and
  not in the direction the note expected: the Gemini credential filename
  is confirmed (`oauth_creds.json`), the `task_metadata.json` workspace
  field was **refuted** for both extensions, and Windsurf's profile root
  is now officially documented while its `workspaceStorage` is not.
  Cursor and Windsurf are `unverified` in consequence; the macOS-only
  paths remain a Linux-track item. What is left still degrades to an
  honest "unresolved"/"(unsupported layout version)" outcome rather than
  a
  wrong guess when a real installation does not match, but whether the
  *assumption itself* is correct needs a maintainer with an actual
  installation of the tool in question (or a reachable primary-source
  layout doc this chunk could not find) to confirm.
- **Other named unknowns, verbatim from `FOLLOWUPS.md`**: "`~/.claude.json`
  sibling file and `todos/` prefix heuristic are named unknowns"
  (Claude Code); Codex's `skills/`/`log/` names and `CODEX_SQLITE_HOME`
  are now confirmed, and OpenCode's `OPENCODE_DATA_DIR` is confirmed
  **not to exist** (see the support matrix's `Verified against` column
  for both); "Oh My Pi
  blob GC and OpenCode snapshot/`storage/part` removal deliberately not
  offered (complete-reference requirement)" remains a scope boundary,
  not a bug.
- **No real installed tool has ever been checked.** Every fixture in
  this entire feature (all four chunks plus this one) is synthetic, by
  the hard privacy rule the handoff sets. That means no adapter's
  format assumptions have been verified against an *actual* `~/.claude`,
  `~/.codex`, `~/.omp`, or any other real tool home on any real machine
  -- only against primary-source documentation/code and this project's
  own hand-built fixtures. Whether the real, currently-installed version
  of each tool on Muness's own machine actually matches what this
  adapter expects is exactly the kind of outside check the handoff asks
  to be surfaced, not fabricated. A reasonable first step: run `swamp
  report --view agents` against a real machine with one or two of these
  tools installed and compare the reported categories/sizes against
  what `du`/`ls` show by hand for the same directory -- without ever
  feeding a real transcript's content into a bug report, fixture, or
  test.
- **Linux paths** for Cursor/Windsurf/Cline/Roo Code (and re-verification
  of every other adapter's Linux path) are explicitly out of scope for
  this macOS-only chunk; the independent Linux track (#77-#89) owns
  that work, not a guess made here.
- **Whether the epic can actually close.** #90/#102's own acceptance
  requires every named tool tested *and* independent validation *and*
  human review of usefulness. The first two are now substantially
  covered (the per-adapter test files, this chunk's refusal-matrix and
  validation suites); the third -- an actual human reading actual
  output and judging whether it is useful, not vague, not overwhelming
  with caveats, and not misleading -- has not happened and cannot be
  simulated by an agent. This is the honest remaining blocker to
  closing #90, not a technical gap.
