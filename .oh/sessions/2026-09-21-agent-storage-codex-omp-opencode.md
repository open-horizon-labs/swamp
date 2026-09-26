# Agent-tool storage: Codex, Oh My Pi, OpenCode, and TUI action wiring (#93/#94/#95/#100/#101)

## Aim

Implement #93 (Codex + its desktop app), #94 (Oh My Pi), #95 (OpenCode)
identification -> project linkage -> CLI/TUI exposure -> supported
cleanup, each to the same completeness as the existing Claude Code
adapter (#92), plus the TUI Space/Backspace/Enter action wiring for
agent units that the Claude Code chunk left as a named, deliberate gap.
Worked in `integration/full-scope`, per
`.oh/handoffs/2026-09-21-claude-full-scope.md` and the chunk brief that
pointed here. Privacy is a hard rule throughout: no real `~/.codex`,
`~/.omp`, or OpenCode data directory was ever read; formats came from
primary upstream source (`WebFetch`/`gh api` against public repos), and
every fixture is synthetic with a canary string tests assert never
leaks into any serialized output.

## Research approach

Fetched primary source directly rather than trusting only the prior
chunk's `matrix.rs` notes (which were themselves honest about being
un-independently-verified for these three tools):

- **Codex**: `openai/codex`'s `codex-rs` source tree via `gh api`/`curl`
  (raw GitHub, since `developers.openai.com/codex/*` 404'd from this
  environment). Found `CODEX_HOME`'s exact resolver
  (`codex-rs/utils/home-dir/src/lib.rs`), `sessions`/`archived_sessions`
  subdir constants and their year/month/day tree shape
  (`codex-rs/rollout/src/list.rs`), the `rollout-<ts>-<id>[_<id>].jsonl`
  filename grammar, the `session_meta` entry's `cwd` field, and --
  unexpectedly -- six named SQLite state databases
  (`codex-rs/state/src/sqlite.rs`) with their own independent
  `CODEX_SQLITE_HOME` override, plus a separate desktop-app log
  location (`codex-rs/cli/src/doctor/desktop.rs`,
  `~/Library/Logs/com.openai.codex`) confirming the CLI and desktop
  client really are materially different storage shapes, as the issue
  anticipated.
- **Oh My Pi**: `can1357/oh-my-pi`'s `docs/session.md`/`docs/settings.md`
  directly. Confirmed the fixed 256-byte title-slot header framing, the
  `cwd`/`additionalDirectories` header fields, the content-addressed
  `blobs/<sha256>` store and its `blob:sha256:<hash>` reference tokens,
  and that the *actual* agent directory is `~/.omp/agent` (not the bare
  `~/.omp` the issue named as the user-confirmed identity) -- `~/.omp`
  is the wrapper; `PI_CODING_AGENT_DIR` relocates `agent/` as a whole.
- **OpenCode**: `sst/opencode`'s `storage.ts` (file-tree layout) plus
  its own DeepWiki-indexed documentation, which surfaced a **newer,
  SQLite-backed layout** (`opencode.db`, `Global.Path.data/opencode.db`)
  this epic's prior research had not found -- exactly the "version-aware
  boundaries" #95 asked for. Also confirmed `storage/project/<id>.json`
  carries a real `worktree` filesystem path, meaning OpenCode's project
  linkage never needs a session-body read at all, unlike every other
  adapter here.

## Decisions

- **Extracted `agents::resolve_declared_path`** out of
  `claude_code::resolve_project_link`'s body (behavior-preserving
  refactor, Claude Code's own tests still pass unchanged) once three
  more adapters needed the identical "declared path -> `crate::git`
  identity" walk. One `.git`-walk bug fixed once now fixes it for every
  adapter, not four independent copies.
- **Extended `AgentMemberKind`** with `Database` (a SQLite file + its
  `-wal`/`-shm` sidecars, folded into one unit -- Codex's six stores,
  OpenCode's `opencode.db`, Oh My Pi's `agent.db`) and `SessionData` (a
  session-keyed companion that is not a transcript/subagent-dir/todos/
  file-history/attachment -- OpenCode's `message/`/`session_diff/`).
  Both explicitly sanctioned by the brief's "extend the shared model
  only where a tool genuinely needs a new concept" clause; no other
  extension to the shared `agents` module was needed.
- **Codex desktop app is its own `AgentToolId`/matrix row**
  (`CodexDesktop`), not folded into `Codex`'s. It is a deliberately
  *partial* `Supported` row: only the confirmed log directory is
  modeled; settings/session storage is stated as unconfirmed and not
  guessed at. This directly satisfies #93's "do not extrapolate one
  client's schema to all clients; model the desktop app storage
  location as a distinct detector with its own supported/unknown
  status" acceptance line.
- **Oh My Pi unknown-format disambiguation**: rather than scanning the
  bare `~/.omp` and guessing, the detector resolves `~/.omp/agent`
  specifically (a path an unrelated `~/.omp` user like oh-my-posh has
  no reason to create), and the adapter *additionally* verifies content
  markers (`config.yml`/`config.yaml`/`agent.db`/`sessions/`/`blobs/`)
  before identifying anything, reporting one explicit "unknown format"
  residual otherwise. Defense in depth, not reliance on the path shape
  alone, per the issue's explicit "verify!" callout.
- **Oh My Pi shared-blob reference accounting is bounded, not
  complete, and never actionable this chunk.** Establishing complete
  reference coverage would mean reading every session body in full;
  instead each session's body is scanned up to 64 KiB extracting only
  `blob:sha256:<hash>` tokens (never persisted as text), and if *any*
  session in a pass exceeded the bound, every blob's reference count is
  reported as unknown rather than a possibly-wrong number. No
  `AgentActionCapability` other than `None` is ever produced for a
  blob -- reference-based GC is out of scope here, not merely gated by
  a runtime check that could itself be wrong.
- **OpenCode's three independent roots (data/config/cache)** are all
  proposed by one detector, data first, since `agents::discover_and_measure`
  only decomposes a detector's *first* `Resolved` location into
  `AgentUnit`s. Config and cache remain visible as ordinary opaque
  external units (their own byte totals, via the pre-existing
  `external.rs` path) rather than being forced into the `AgentUnit`
  model where they carry no session/project linkage anyway.
- **OpenCode version boundary**: `identify` checks for `opencode.db`/
  `storage/`/`snapshot/`/`auth.json`/`log/` before doing anything else;
  none present but the directory non-empty means one explicit
  "unsupported layout version" unit, never a guess at either schema.
  When `opencode.db` is present, the file-tree `storage/session/` walk
  is skipped entirely (no double-counting between the DB-backed store
  and legacy on-disk debris); `storage/message/`/`storage/session_diff/`
  entries unclaimed by no in-scope session-file walk are still folded
  into an explicit residual, never silently dropped.
- **TUI wiring**: added `MarkedUnit::agent_plan`, a field kept
  *separate* from `cargo_plan` rather than reusing it, specifically
  because `execute_one`'s Cargo-only `keep_executables` conflict check
  must never wrongly refuse an unrelated agent-storage removal just
  because the human also has "keep executables" toggled on.
  `model::agent_rows` now sets `Row.unit: Some(...)` for **every** row,
  protected ones included, so `app::mark_row`'s new agent branch always
  reaches `actions::propose_agents` and that call's own refusal text
  becomes the footer -- never a generic "nothing to delete on this
  row" for a unit the human can plainly see. Marking already runs
  inside the existing `review_in_background` worker-thread indirection
  (confirmed by reading `review_in_background`'s body before wiring
  anything), so no new `tui_nonblocking` guard entry was needed -- the
  source audit still passes all 20 checks unchanged.
- **Bulk marking (`Shift+A`) intentionally left unextended to agent
  rows** this chunk: `mark_all_in_view` only recognizes a
  `row.kind`/`ArtifactKind` or a projects-view `row.project`, and
  teaching it a third shape was out of the requested scope (Space/
  Backspace/Enter on the *selected* row). Recorded as a named gap in
  `docs/agent-storage.md`/`docs/architecture.md`/`DESIGN.md`, not
  silently dropped.

## Measured scan cost (500 synthetic sessions each, this chunk's dev
machine, oversized bodies to prove only bounded reads happen)

```
[measured] codex identify() over 500 synthetic sessions took 68.132ms
[measured] oh_my_pi identify() over 500 synthetic sessions took 110.02ms
[measured] opencode identify() over 500 synthetic sessions took 7.889ms
[measured] identify() over 500 synthetic sessions took 74.501ms   (claude_code, for comparison)
```

Oh My Pi is the slowest of the four because it is the only adapter that
scans session *bodies* (bounded to 64 KiB each) for blob references, on
top of the header read every adapter does. OpenCode is fastest because
its project linkage reads one small `project.json` per project
directory rather than any per-session content.

## What was not independently re-verified

- Codex's `skills/` directory name and `log/` directory name (carried
  over from this epic's prior research, found in source search but not
  pinned to one official docs page this session).
- Codex's `CODEX_SQLITE_HOME` override is not followed by this adapter
  (if set, the six state databases are simply not found here).
- OpenCode's `OPENCODE_DATA_DIR` env var name itself (the *default*
  `XDG_DATA_HOME`/`~/.local/share/opencode` path and the config/cache
  paths are independently confirmed via `opencode.ai/docs/troubleshooting`
  and two GitHub issue threads; the override name is carried over from
  the prior chunk's research and honored defensively, but not
  independently re-confirmed against source this session).

## Verification

`cargo fmt --all --check`, `cargo test --workspace --locked`, `cargo
clippy --workspace --all-targets --locked -- -D warnings`, `cargo run
--locked -p swamp-source-audit` (all 20 audits `ok`, including
`tui_actions_off_event_thread`), and `scripts/check.sh` all passed on
this chunk's final state -- see the integration owner's report for
exact commands/output and the commit hashes.
