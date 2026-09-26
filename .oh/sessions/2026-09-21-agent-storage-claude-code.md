# Agent-tool storage: shared model and Claude Code (#91/#92/#100/#101)

## Aim

Implement #91 (shared agent-tool storage discovery/model/history), #92
(Claude Code identification), and the #100/#101 CLI slices for Claude
Code, in the `integration/full-scope` sequential chain, per
`.oh/handoffs/2026-09-21-claude-full-scope.md` and the chunk brief that
pointed here. Also fill chunk B2's explicitly recorded TUI gap
(`DESIGN.md`'s "Coverage line and external rows (not yet implemented)")
by adding the External view alongside the Agents view this chunk needed
anyway, since both are the same shape of minimal read-only addition.

## What landed

### #91: shared model (`crates/core/src/agents/mod.rs`)

- `AgentCategory` (sessions, attachments, checkpoints, caches, logs,
  managed-worktrees, plugins, protected-config, unclassified), with
  `default_protected()` naming exactly one category
  (`ProtectedConfig`) as protected by category default -- everything
  else is protected only individually (an adapter's own judgment, or a
  human `swamp protect` entry).
- `ProjectLinkState`: `Linked{project_id, project_name, project_path,
  source: Declared|Inferred, worktree_kind}`, `Unresolved{reason}`,
  `Missing{path}`, `NotAProject{path}`, `Moved{from,to}`,
  `Remote{host,path}`, `Shared{project_ids}`, `NotApplicable`. Claude
  Code populates `Linked`/`Unresolved`/`Missing`/`NotAProject`/
  `NotApplicable`; `Moved`/`Remote`/`Shared` are modeled (per #91's
  acceptance) but not produced by this adapter -- no reliable signal
  exists for them from a Claude Code session header, and this is
  recorded rather than guessed.
- `AgentUnit`: identity `(tool_id, category, relative_path)`,
  `tool_home` (per-unit, not assumed shared across a mixed selection --
  see the decision below), `members: Vec<AgentMember>` (a session's
  transcript/subagent-dir/file-history/todos), `bytes`,
  `growth_bytes`/`regrowth_count` from history, `protected`/
  `protect_reason`, `action: AgentActionCapability` (`None`/
  `CacheOrLogTrash`/`SessionRemoval`).
- History reuse is literal, not just architectural: `unit_key` builds
  the exact same `growth::external_row_key` the home-level `ExternalUnit`
  already uses, with an `"agent:"`-prefixed category string so an agent
  category can never collide with `locations::StorageCategory`'s own
  kebab strings in the same Parquet store.
  `observe_and_annotate_external`/`annotate_readonly_external` are
  called directly -- no new growth-store key family, no new store.
- `folded_bytes`: a small hand-rolled, bounded stat walker (directory
  names + `stat`, no content read), deliberately not
  `walk::resize_artifact`'s parallel-pool machinery -- that machinery
  is tuned for a handful of potentially huge artifact roots, and
  spinning its thread pool up per session (hundreds of them) would
  itself be the "unacceptable scanning cost" #91 guards against.
- `swamp protect`: `agent_protect.json`, a small JSON sidecar under
  `$SWAMP_DIR`, deliberately decoupled from the growth store (mirrors
  `external_consumers.json`'s own precedent) -- touching it can never
  affect an `AgentUnit`'s bytes or history.
- `crates/core/src/agents/matrix.rs`: the required 13-tool matrix.
  Claude Code is `Supported`; the other twelve are `Planned` with a
  sourced `home_note` researched from primary upstream documentation
  during this session (never guessed, never learned from a real
  `~/.claude`-style directory). See `docs/agent-storage.md` for the
  rendered table and every source URL.

### #92: Claude Code (`crates/core/src/agents/claude_code.rs`,
`crates/core/src/locations/claude_code.rs`)

- Home resolution: `CLAUDE_CONFIG_DIR` override, else `~/.claude`,
  registered as a `locations::Detector` exactly like Cargo home/
  Homebrew/rustup -- the home directory itself gets external-unit
  identity/history for free, no new mechanism.
- Session identification: `projects/<encoded-cwd>/<session-id>.jsonl`
  plus its companion `<session-id>/` directory (exact match, same
  basename), `file-history/<session-id>/` (exact, documented path),
  `todos/<session-id>*` (a **documented heuristic**: filename-prefix
  match, since Claude Code's own documentation does not specify the
  todos naming convention -- session ids are UUIDv4, so collision risk
  is negligible, and an ambiguous match is excluded rather than
  guessed), `image-cache/<session-id>/`, `uploads/<session-id>/` (exact,
  documented per-session subdirectories).
- Project linkage: reads only a session transcript's first line
  (bounded to 8 KiB), looks for a `cwd` field, then walks upward from
  that path calling `crate::git::classify_main_checkout`/
  `classify_git_file` (made `pub(crate)` already; reused verbatim) --
  never decodes the `projects/<encoded-cwd>` directory name back into a
  path, since that encoding is lossy in the reverse direction (a
  literal hyphen in a real path is indistinguishable from an encoded
  path separator).
- Static category table for everything else (protected config, caches,
  logs, checkpoints, attachments, unclassified), plus a genuine
  unclassified-residual catch for any top-level entry with no specific
  rule -- so a future Claude Code version adding a new top-level file
  never silently vanishes from a report.
- Sources: `code.claude.com/docs/en/claude-directory` (the directory
  table this adapter's category assignments are built from),
  `.../settings`, `.../checkpointing` (`file-history/`, `/rewind`),
  `.../authentication` (`.credentials.json`, Keychain migration). Each
  is cited in the module doc comment and `docs/agent-storage.md`.

### #100/#101: CLI, actions, and TUI

- `swamp report --view agents [--project NAME] [--all] [--json]`:
  tool → category → unit drill-down (`render::render_view_agents`),
  bounded to 20 rows per category unless `--all`, oldest-modified
  first. `--project` narrows to units whose linkage names that
  project (`agent_unit_matches_project`), consistently between text
  and JSON.
- `swamp protect add|remove|list [--json] <path>`.
- `actions::propose_agents`/`unit_from_agent`/`agent_refusal`: unlike
  `propose_external` (inspection-only by construction for every unit),
  a supported agent category becomes a *real* `PlanUnit` carrying
  `agent_meta: AgentPlanMeta`; `agent_refusal` refuses at proposal time
  for protected paths/categories, unsupported categories,
  SQLite/WAL/SHM-like filenames, and active sessions.
  `execute_with_trash_opts`'s new branch rechecks occupancy again
  (authoritative) and dispatches to `execute_agent_cache_trash`
  (single-path move) or `execute_agent_session_removal` (re-derives
  the session's current membership fresh, refuses on any drift, then
  moves every member into one Trash envelope -- pre-flight-stats every
  member before any rename, to shrink the partial-failure window).
- `swamp propose-agents --path <unit-path> [--json]`: a new, dedicated
  CLI subcommand (not a `--agent` mode on the existing `Propose`,
  which requires a walked report `root: PathBuf` -- see the decision
  below). `swamp approve`/`swamp execute` needed **no changes**: both
  already operate generically on any `plan_id`, so
  `propose-agents -> approve -> execute` is a complete, working path
  verified against the real built binary
  (`crates/cli/tests/agent_storage_cli.rs`).
- TUI: `ViewKind::External` (`'9'`) and `ViewKind::Agents` (no
  dedicated digit; `v`-cycle only), both read-only (`Row.unit: None`).
  `App::set_external_units`/`set_agent_units`, populated once at
  startup in `crates/tui/src/lib.rs::run()` alongside the existing
  initial report load -- never on the event/render loop. Also added the
  header's scope-coverage clause (`App::set_scope_note`) the chunk
  brief asked for alongside the External view -- see the scoped-down
  decision below.

## Decisions that needed to be made explicitly

**`AgentUnit` carries its own `tool_home`, not a shared parameter.**
My first draft of `actions::propose_agents` took a single `tool_home:
&Path` parameter applied to every matched unit, which is only correct
because exactly one tool (Claude Code) is implemented this chunk. I
changed course before finishing: I added `tool_home: PathBuf` directly
to `AgentUnit` (set per-unit in `discover_and_measure`, which already
has `home` in scope per detector) so a future second adapter's units
mixed into one selection each carry their own correct home for
`execute_agent_session_removal`'s fresh re-identification, rather than
requiring `propose_agents`'s caller to somehow know which single home
applies. `Plan.root` (a single path, used only for the pre/post free-
space measurement) is set from the first matched unit's `tool_home` and
documented as informational only -- `agent_meta.tool_home` is what
`execute` actually trusts.

**Occupancy is checked at action time only, never during ordinary
identification.** #92's acceptance text names an "active-session check
... via existing occupancy seam" as part of identification. I read
this literally against the cost guardrail: `occupancy::occupied` shells
out to `lsof`, and running it for every one of (potentially hundreds
of) sessions on an ordinary `report --view agents` would itself be the
"unacceptable scanning cost" #91's own acceptance names, and would
violate the spirit of `tui_nonblocking` for the TUI path. Occupancy is
checked only when proposing/executing an action on a *specific*
selected unit (one or a few lsof calls, not hundreds) -- and, since a
plain `lsof <path>` only reports processes with that *exact* path open
(a directory's own fd, or a specific file), the session-removal case
checks the transcript file itself (exact and reliable) while the
cache/log-directory case checks the category directory (best-effort:
it would miss a process with a file open *inside* the directory
without the directory itself open). Recorded as an honest limitation,
not silently assumed complete.

**`todos/` matching is a heuristic, exactly once, and named as such.**
Every other session-member source (`file-history/<id>/`,
`image-cache/<id>/`, `uploads/<id>/`, the companion `<id>/` directory)
is an exact match on a documented or structurally-implied path. Claude
Code's own documentation does not specify how `todos/` entries are
named per session. Rather than guess a specific pattern with false
confidence, or skip `todos/` entirely, I match by filename *prefix* on
the session id (UUIDv4 -- collision-negligible) and documented this as
a heuristic in three places (module doc comment, `docs/agent-storage.md`,
this note) so a future worker who learns the real convention knows
exactly what to revisit and why.

**`~/.claude.json` is out of scope, not force-fit.** Claude Code keeps
`~/.claude.json` as a *sibling* of `~/.claude/`, not inside it (per
Anthropic's own "Global Configuration" table). Every identity in this
model (`AgentUnit`, `ExternalUnit`) is a canonical path *under* a tool
home; a sibling file does not fit that model. I chose not to invent a
special case for one file living outside the home directory's own
namespace -- documented as an explicit, named gap in three places
(the detector's own doc comment, `docs/agent-storage.md`, the matrix
row's `home_note`) rather than silently uncovered.

**`swamp propose-agents` is a new subcommand, not a `Propose --agent`
mode.** The existing `Propose` CLI command requires a walked report
`root: PathBuf` end to end. Chunk B2's session note recorded the exact
same reasoning for `propose_external` having no CLI wiring ("adding one
means either making `root` optional ... or a separate subcommand ...
left as an explicit, named gap"). I made a different call for agent
units specifically, since #101's whole point (unlike #43's) is a real,
working action path: a small additive subcommand
(`ProposeAgents { paths, json }`) that resolves its own scope
internally, with **no changes** to `Propose`, `Approve`, or `Execute`
(the latter two are already generic over any `plan_id`). This keeps
the existing, well-tested `Propose` command's contract completely
untouched while still shipping a genuinely complete, working CLI path.

**Cross-device Trash bug caught by my own tests, not shipped.** My
first draft of `crates/core/tests/agent_units_actions.rs`'s
action-executing tests called the default `actions::execute`, which
uses the *real* `$HOME/.Trash` unless `SWAMP_TRASH_DIR` is set. One
test's `fs::rename` failed with what strongly looks like a cross-
device-link error when moving fixture content (under the OS temp
directory) into the real `~/.Trash`; the other test's rename
(coincidentally) succeeded and left real content in my own `~/.Trash`.
Caught by re-reading my own test's assertions before considering the
chunk done, not by CI. Fixed by using `execute_with_trash` with a
fixture Trash directory in every test that performs a real move
(`crates/core/tests/agent_units_actions.rs` and
`crates/cli/tests/agent_storage_cli.rs`'s `SWAMP_TRASH_DIR` env var) --
no test in this chunk touches the real developer Trash.

## Adversarial tests written (not just happy path)

- `crates/core/src/agents/mod.rs` (6 tests): category default
  protection is `ProtectedConfig`-only; agent category key strings can
  never collide with `StorageCategory`'s own kebab strings;
  `swamp protect` add/list/remove round-trips and survives reload;
  protection matches by path or ancestor prefix; `folded_bytes` reports
  truncation rather than a silently short total, and handles a plain
  file (not just a directory).
- `crates/core/src/agents/claude_code.rs` (12 tests): a session's
  companion members are all identified and none of a seeded canary
  string leaks into the identification output; a declared `cwd` that
  no longer exists is `Missing`; a `cwd` that exists but has no `.git`
  is `NotAProject`; a malformed/empty transcript is `Unresolved`, never
  a panic; an in-progress transcript with no trailing newline still
  parses; protected-by-default categories are protected and
  unactionable; cache/log categories are actionable and unprotected;
  `history.jsonl` is individually protected despite its `sessions`
  category; unmatched `todos/` entries do not fabricate a session; an
  unknown top-level entry is folded into one residual unit, not
  dropped; 500 synthetic sessions (each with an oversized body, so only
  the bounded first-line read is ever exercised) identify correctly and
  within a measured-cost bound.
- `crates/core/src/agents/matrix.rs` (3 tests): all 13 named tools
  present exactly once; only Claude Code is `Supported`; every row has
  at least one `https://` source and a non-empty `home_note`.
- `crates/core/tests/agent_units_actions.rs` (7 tests): cache removal
  preserves auth/settings/history/unselected-debug; selected session
  removal preserves an unrelated session, the linked project's own
  `.git`, and shared settings; a protected category refuses at
  proposal time; a path matching no unit refuses; a transcript held
  open by the *test process itself* (a real `lsof`-visible occupant --
  not a mock) refuses at both proposal and (would refuse at) execution;
  the seeded canary never appears in the serialized units, plan,
  execute result, or ledger; cache-category action capability is
  reported correctly on the unit.
- `crates/cli/tests/agent_storage_cli.rs` (5 tests, against the real
  built binary): `--view agents` lists a linked session and never
  contains a `"message"` field anywhere in its JSON; `--project`
  narrows to zero when nothing matches; `swamp protect` round-trips
  through the binary; the full `propose-agents -> approve -> execute`
  path moves an unprotected cache to a *fixture* Trash directory
  (`SWAMP_TRASH_DIR`), never the real one; a protected path refuses.
- `crates/tui/tests/frames.rs` (+2 tests): `external_view`/`agents_view`
  snapshot frames at both first-class terminal sizes.

## Measured cost

Single-sample, one developer's machine (same convention as this
codebase's other timing notes: illustrative, not a benchmark suite).
`identification_cost_is_bounded_for_many_sessions` (500 synthetic
sessions, each transcript padded past 200 KB so only the bounded
first-line read is ever exercised, not the whole file):

```
[measured] identify() over 500 synthetic sessions took 34.255ms
[measured] identify() over 500 synthetic sessions took 29.610ms
```

Consistent with "reads directory names + bounded metadata, never whole
transcripts": 500 oversized transcripts identify in well under 50ms,
not seconds. The test's own threshold (10s) is deliberately generous
(CI-machine headroom), not the expected number; this note records the
actual observed number so a future regression is visible against real
data, not just the loose CI bound.

## Follow-ups for later workers

- **#93-#99**: the remaining twelve named tools. `crates/core/src/agents/matrix.rs`
  has sourced home-path notes for all of them; `claude_code.rs` is the
  pattern to copy (a `locations::Detector` for the home, an
  `agents::<tool>::identify` module, registered in
  `agents::identify_for_tool`).
  `crate::agents::claude_code::CLAUDE_CODE_TOOL_ID`/`identify` is
  called directly (not through a trait object) from
  `actions::execute_agent_session_removal`; a second tool needs a
  `match` arm added there too (or a small trait, if a third tool makes
  the match awkward -- two tools does not yet justify one).
- **TUI mark/confirm/execute for agent-storage units**: both new views
  are read-only this chunk (`Row.unit: None`). Wiring Space/Backspace
  to `actions::propose_agents`/`execute` (occupancy/reference recheck,
  session-removal warnings, Trash envelope) is a real feature, not a
  rendering tweak -- same judgment chunk B2 recorded for why it left
  the External view unimplemented, applied consistently here.
- **Coverage line, scoped down and implemented.** The chunk brief
  explicitly asked for "the coverage clause in the header" alongside
  the External view, so I added `App::set_scope_note`/`ui.rs`'s new
  header clause -- but scoped to what is honestly available without
  taking on #50's full "make the TUI's own report multi-root" job: it
  reflects `scope::RootStatus` for the TUI's one explicit root
  (resolved fresh, no walk needed), not `coverage::RegionStatus` (the
  actual per-root walk outcome `report_scope`'s `scope_coverage` field
  carries, which does not exist for the TUI's single-root
  `report_full_mode`/`report_with_dirs` path). The common case (one
  present root) shows no clause, matching today's real usage; a future
  #50 worker making the TUI multi-root should read
  `App::set_scope_note` and switch its input to a real
  `coverage::RegionStatus` list rather than re-deriving the mechanism.
- **`todos/` naming convention**: if a future worker learns Claude
  Code's actual `todos/` filename convention (from an upstream source,
  never from a real `~/.claude`), replace the filename-prefix heuristic
  in `claude_code.rs` with an exact match and update
  `docs/agent-storage.md`'s note accordingly.
- **`~/.claude.json`**: if a future worker wants to model this sibling
  file, it needs either a relaxed identity contract (a path *beside*,
  not under, the tool home) or a distinct small mechanism -- not a
  forced fit into the existing `(tool_id, category, relative_path
  under home)` model.
- **Plugins/marketplace removal**: only `plugins/.trash`/`skills/.trash`
  are actionable this chunk. Native marketplace-aware removal (e.g. a
  `claude plugin` command, if one exists, verified for scoped
  per-plugin removal) is the preferred mechanism for the rest, not a
  guessed recursive delete of `plugins/`.
