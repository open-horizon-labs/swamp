# Agent-tool storage: the nine remaining named tools (#96/#97/#98/#99), plus Shift+A for agent rows

## Aim

Implement #96 (Gemini CLI, Pi, Aider), #97 (GitHub Copilot CLI), #98
(Cursor, Windsurf) and #99 (Cline, Roo Code, Continue) identification
-> project linkage -> CLI/TUI exposure -> supported cleanup, each to
the same completeness as the five existing adapters (#92-#95), closing
the full required 14-row matrix (#90/#91). Also extended
`App::mark_all_in_view` (`Shift+A`) so it recognizes supported agent
rows, a named gap the prior chunk's session note and
`docs/agent-storage.md` both flagged rather than silently left. Worked
in `integration/full-scope`, per
`.oh/handoffs/2026-09-21-claude-full-scope.md`. Privacy is a hard rule
throughout: no real `~/.gemini`, `~/.pi`, `~/.aider`, `~/.copilot`,
Cursor/Windsurf/VS-Code-family profile, or `~/.continue` was ever read
on this machine; formats came from primary upstream source (`WebFetch`
against public docs/source), and every fixture is synthetic with a
canary string tests assert never leaks into any serialized output.

## Research approach and corrections to prior (planned-row) research

`crates/core/src/agents/matrix.rs` already carried sourced-but-unverified
`Planned` rows for all nine tools from the epic's earlier planning pass.
Re-verifying them against primary source this chunk found several real
corrections, each recorded in the adapter/detector doc comments, not
just here:

- **Gemini CLI**: the real config-home override is `GEMINI_CLI_HOME`,
  not the matrix's prior `GEMINI_CONFIG_HOME` guess
  (`docs/reference/configuration.md`). The project-hash algorithm is
  confirmed directly in source
  (`packages/core/src/utils/paths.ts`'s `getProjectHash`):
  `sha256(projectRoot).hex()`. `docs/cli/checkpointing.md` and
  `docs/cli/session-management.md` clarified that `chats/` actually
  lives at `tmp/<hash>/chats/`, not as a bare top-level directory as the
  matrix's prior note implied, and that checkpoints span *two* places:
  `tmp/<hash>/checkpoints/` and a separate shadow-Git repo at
  `history/<hash>/`. OAuth/account credential file names were not found
  on any reachable page (`docs/cli/authentication.md` and
  `docs/get-started/authentication.md` both 404 against current `main`)
  -- handled with a defensive filename-pattern protect rather than an
  invented exact name.
- **Pi**: `badlogic/pi-mono`'s own README documents
  `PI_CODING_AGENT_DIR` as the override -- the *same* name Oh My Pi's
  own docs use for its own override, not `PI_AGENT_DIR` as the issue
  text guessed. This is a genuine, disclosed collision risk (see
  `crate::locations::pi`'s doc comment), not silently resolved. Pi's
  README also does not document Oh My Pi's 256-byte title-slot session
  header -- confirming the issue's own instruction to "detect version/
  format explicitly" was load-bearing, not boilerplate: this adapter
  tries Pi's own offset-zero header first, Oh My Pi's shape only as an
  explicit fallback.
- **Aider**: `aider/models.py`/`versioncheck.py` confirmed
  `~/.aider/caches/model_prices_and_context_window.json` and
  `~/.aider/caches/versioncheck` exactly; `aider/repomap.py` confirmed
  `.aider.tags.cache.v{3,4}` (version depends on whether the optional
  TSL pack is active). This tool's storage really is split between a
  home directory and each project checkout, as the matrix's prior note
  already flagged -- the per-repo half required a new orchestration
  parameter (`project_worktrees`) rather than fitting the existing
  home-only adapter contract.
- **GitHub Copilot CLI**: `docs.github.com`'s own reference page (found
  and fetched cleanly, no 404) corrected the matrix's guessed
  `history-session-state/` to the real `session-state/` and
  `command-history-state/`, and surfaced considerably more structure
  than the matrix's prior note had (`mcp-oauth-config/`, `mcp-secrets/`,
  `permissions-config.json`, `providers.json`, `session-store.db`,
  `ide/`, a separate platform-conventional cache independent of
  `COPILOT_HOME`).
- **Cursor**: corroborated by two independent community sources
  (`cursaves`, `cursor-chat-browser`) describing the same
  `state.vscdb`/`ItemTable`/`cursorDiskKV`/`workspace.json` shape --
  higher confidence than a single source, still below the primary-
  source-backed rows.
- **Windsurf**: `docs.windsurf.com` redirected to `docs.devin.ai` during
  this chunk's research (consistent with Windsurf's acquisition/product
  consolidation) -- no current primary documentation of its on-disk
  layout was reachable. Modeled by *assuming* the same VS-Code-fork
  shape Cursor uses (via the shared `agents::vscode_family` module) and
  saying so explicitly, both in code comments and in the matrix/docs;
  an installation that does not match surfaces as an honest
  "(unsupported layout version)" residual rather than a wrong guess.
- **Cline/Roo Code**: both remain community-documented only (GitHub
  issue threads, no official layout-reference page). The Roo Code issue
  (#4174) directly confirmed the remote/server hosting path
  (`~/.vscode-server/data/User/globalStorage/...`), which generalizes
  cleanly to Cline too. Neither issue's thread confirmed
  `task_metadata.json`'s exact field names for workspace linkage; the
  `workspace` field name used here is the issue text's own prior
  research, carried through honestly labeled as unconfirmed rather than
  re-guessed.
- **Continue**: `docs.continue.dev`'s configuration page confirmed
  `config.yaml`/`config.json`'s location; `sessions/`/`index/`/
  `dev_data/` remain this catalog's own prior research, not
  independently re-confirmed this chunk -- treated as version markers
  (presence-checked, not schema-asserted).

## Decisions

- **Aider's per-repo files are not tool-home units.** Per the issue's
  own explicit acceptance, `crate::agents::aider::identify_repo_units`
  is a second, separate function from `identify`, called once per known
  project worktree root. This required extending
  `agents::discover_and_measure`'s signature with a
  `project_worktrees: &[PathBuf]` parameter -- the only orchestration
  signature change this chunk made, and it is additive/backward
  compatible in spirit (every other adapter ignores it). Three call
  sites needed updating (`swamp report --view agents`, TUI startup,
  `swamp propose-agents --path`); the last of these deliberately
  computes no `Report` (to stay fast for an already-known exact path),
  so it derives worktree roots by walking upward from each requested
  path instead (`agents::worktree_root_containing`, reusing
  `resolve_declared_path`'s own `.git`-search logic) rather than paying
  for a whole-scope walk.
- **Cline/Roo Code decompose every resolved host location, not just the
  first.** Every other tool in this catalog (including OpenCode's own
  three-location detector) only has its detector's *first* `Resolved`
  location decomposed into `AgentUnit`s by
  `discover_and_measure`'s existing contract. Cline/Roo Code genuinely
  need all of them (an extension installed into several editor hosts at
  once has separate, real storage under each). Added
  `agents::multi_location_tool` as a narrow, named-tool-id opt-in
  rather than changing the default for every adapter -- OpenCode/
  Copilot CLI/Cursor/Windsurf's own secondary locations stay
  deliberately un-decomposed, per each one's own doc comment.
- **One shared `agents::vscode_family` module for Cursor, Windsurf,
  Cline and Roo Code.** All four sit on the same underlying VS Code
  storage conventions (`state.vscdb` SQLite key-value stores,
  `workspace.json`/`task_metadata.json` declared-path linkage). Per the
  handoff's "extend the shared model only where a tool genuinely needs
  a new concept" clause, this is one real implementation
  (`identify_editor_profile` for the two forks, `identify_extension_globalstorage`
  for the two extensions) with four thin, honestly-labeled wrapper
  adapters, rather than four independent re-implementations of the same
  SQLite-folding and JSON-field-linkage logic.
- **Host labelling from the path itself, not a new field.** Rather than
  threading a "which host" parameter through the shared
  `identify_for_tool` dispatch (which every other tool also goes
  through, unmodified), `vscode_family::host_label` derives the label by
  pattern-matching known directory-name fragments in the ancestors of
  the path it was already given. Keeps the dispatch contract identical
  for all fourteen tool ids.
- **No new `AgentMemberKind` variant.** Every one of the nine new
  adapters' session-shaped units reuses `Transcript` (single file) or
  `SessionData` (single folded directory); every SQLite store reuses
  `Database`. Direct evidence the shared model from #91-#95 already
  covered these tools' shapes.

## A real bug this chunk's own adversarial tests caught

Five of the nine new adapters (Aider, Gemini CLI, GitHub Copilot CLI,
`vscode_family`, Continue) initially left `CandidateAgentUnit.members`
empty for their `SessionRemoval`-capable units, populating only the
top-level `path`/`bytes`. Identification-only unit tests did not catch
this (they only assert category/action/linkage facts, not that the
member set matches the path). `execute_agent_session_removal`
(`crate::actions`) builds its moved-member list from
`AgentMember`s, though, so an empty `members` list meant the removal's
final move loop had nothing to move: `execute` would report
`"completed"` while silently leaving the file/directory in place -- a
"phantom success" bug, exactly the shape of shortcut this epic's own
guardrails warn against, caught only because
`crates/core/tests/agent_units_actions_remaining_tools.rs`'s tests
exercise the full propose -> approve -> execute -> assert-not-exists
path for a real removal, not just identification. Fixed by populating
one self-referential `AgentMember` per single-unit session (kind
`Transcript` for a file, `SessionData` for a directory) in each of the
five adapters. Recorded here as the concrete argument for why this
chunk's tests execute real removals end-to-end rather than stopping at
"the unit was identified with the right fields."

## Measured identification cost (500 synthetic sessions/units unless noted; this chunk's dev machine)

| Adapter | Measured |
|---|---|
| `gemini_cli` | ~161ms (300 synthetic project-hash directories) |
| `pi` | ~203ms |
| `aider` (`identify_repo_units`, 2000-file tags cache) | ~6ms |
| `copilot_cli` | ~172ms |
| `vscode_family` (`identify_extension_globalstorage`) | ~132ms |
| `continue_dev` | ~5ms |

All well under the 10s bound each adapter's own
`identification_cost_is_bounded_for_many_*` test asserts.

## Known gaps and explicit unknowns (not fabricated, not hidden)

- Windsurf's editor-profile shape is assumed (VS Code fork), not
  independently confirmed -- `docs.windsurf.com` redirected away during
  this chunk's research.
- Cline/Roo Code's `task_metadata.json` `workspace` field is this
  chunk's own prior research (from the issue text), not independently
  re-confirmed against an official schema doc.
- Cursor, Windsurf, Cline and Roo Code are macOS-only this chunk; Linux
  paths are the independent Linux track's job (#77-#89), not guessed
  here.
- Gemini CLI's OAuth/account credential file name is unconfirmed;
  protected defensively by filename pattern instead.
- `swamp propose-agents --path` cannot surface Aider units for a
  worktree root that is not itself derivable from the requested path
  (no full-scope walk in that fast CLI path); `report --view agents`
  and the TUI have no such limitation, since both already compute a
  `Report`.
- Independent validation (#102) across the full 14-tool catalog is a
  separate, still-open item; this chunk closes identification/linkage/
  action-boundary completeness, not that validation pass.

## Verification

`cargo fmt --all --check`, `cargo test --workspace --locked`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo run -p swamp-source-audit`, and `scripts/check.sh` all run
against `integration/full-scope` in
`/Users/muness1/src/open-horizon-labs/swamp-tui-build-decisions`; see
the worker's final report for exact command output and commit hashes.
