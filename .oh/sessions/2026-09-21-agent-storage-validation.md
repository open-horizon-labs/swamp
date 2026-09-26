# Agent-tool storage: #100/#101 completion and #102 validation (chunk F)

## Aim

Audit #100/#101's acceptance against the implementation left by the
prior four chunks (Claude Code #92; Codex/Oh My Pi/OpenCode #93-#95;
the remaining nine named tools #96-#99), fix every gap found, unify the
CLI's `propose` entry point across agent-storage/external/filesystem
units, and deliver #102's independent validation suite and benchmarks.
Worked in `integration/full-scope`, per
`.oh/handoffs/2026-09-21-claude-full-scope.md` and the chunk brief that
pointed here (`CHUNK_F.md`). Privacy is a hard rule throughout: every
fixture in every new test is synthetic; no real tool home was ever
read.

## Gaps found and fixed

- **Project tree had no agent-storage linkage at all.** Neither the
  CLI's `--project <name>` text drill nor its `--json` envelope (when
  `--view agents` was not also passed) showed a project's linked agent
  storage; the TUI's Tree view had no equivalent either. Fixed by
  adding `crate::tree::ProjectTree::agent_rows` (`agent_rows_for_project`),
  threaded through `render_project_tree_with_agents` (CLI text) and
  `model::tree_rows_with_agents` (TUI), and by computing `agent_units`
  in the CLI whenever `project.is_some()`, not only `view ==
  Some(View::Agents)`, adding an `agent_storage: {units, total_bytes}`
  key to the project-scoped JSON envelope.
- **`propose-agents` was a separate subcommand, `propose --external`
  did not exist at the CLI level.** Unified into one `propose` command:
  `root` is now `Option<PathBuf>`; without one, `--path` routes to the
  agent-storage proposer, then the external-unit proposer, refusing by
  name if neither matches; `--external` forces the latter explicitly.
  `propose-agents` is now a thin, deprecated alias delegating into the
  same `propose_unified` function.
- **The unified route's agent-unit discovery had to fix the exact gap
  chunk E's own follow-up named**: the old `propose-agents --path` fast
  path found Aider's per-repo units only by walking upward from the
  requested paths themselves, so a path whose worktree root was not
  itself part of the request never surfaced an Aider unit.
  `discover_agent_units_for_propose` now runs the same real report walk
  `report --view agents` uses (every known project worktree, not just
  ones implied by the request) before resolving agent units.
- **No parent/child overlap guard for agent-storage plans.** `propose`'s
  filesystem route already refuses overlapping Cargo-group selections;
  `propose_agents` had no equivalent. Added the same discipline
  (`actions::propose_agents`'s new overlap check). Writing the
  table-driven refusal-matrix test that exercises this immediately
  caught a **real, latent duplicate-identification bug** in an existing
  test fixture: `agent_units_actions_new_adapters.rs`'s `only_detector`
  helper predates the nine later tools and never disabled `"pi"`, so an
  Oh My Pi fixture using `PI_CODING_AGENT_DIR` (a disclosed collision
  risk both tools' own docs document) was *also* independently
  identified by the Pi adapter, producing two `AgentUnit`s at the exact
  same path under two different tool ids. Fixed the test helper to
  disable every named tool detector except the one under test; the new
  overlap check is correct behavior, not overly strict.
- **No recovery manifest for a partially-failed session removal.**
  `execute_agent_session_removal` moved members in a loop with no
  durable record of partial progress beyond an error string. Added
  `restore.json`, written into the Trash envelope before any member
  moves and rewritten after each successful one, plus a
  `PartialAgentRemoval` error type so `execute`'s own outcome still
  names the envelope and the bytes that really moved on a partial
  failure, rather than only a bare error with no traceable location.

## Decisions

- **The refusal-matrix test operates on synthetic `AgentUnit` literals
  for the cross-cutting checks (protected category, human-protect flag,
  unsupported action, database-like path, active session,
  whole-home/whole-projects-dir path), across all 14 named tool ids.**
  `agent_refusal`/`propose_agents` are generic over these fields --
  they do not branch on `tool_id` except for re-identification at
  execute time -- so testing the shared mechanism once per tool id at
  this level is the right altitude, not a shortcut. Two scenarios that
  *do* need a real adapter (parent/child overlap's Codex pairing, plan
  scope drift) go through real `identify()` output instead.
- **Plan scope drift is tested for Claude Code only, named honestly.**
  A membership *set* change (a new member path appearing) is only
  observable for an adapter whose session can have more than one member
  path; Codex's own session unit is deliberately exactly one file (see
  its own dedicated test), so a set-membership-drift scenario does not
  apply to it the same way. The generic re-identification dispatch
  itself (`execute_agent_session_removal`'s `match meta.tool_id`) is
  exercised for every other tool by that tool's own end-to-end
  session-removal test elsewhere in this repo.
- **The #102 validation suite does not re-derive every per-adapter
  test already in this repo.** `crates/core/src/agents/*.rs`'s own
  module tests, `crates/core/tests/agent_units_actions*.rs`, and
  `crates/cli/tests/agent_storage_cli.rs` already cover a great deal of
  #102's acceptance per-adapter. This chunk's new files
  (`agent_refusal_matrix.rs`, `agent_storage_validation.rs` in both
  `core` and `tui`) focus specifically on the cross-cutting properties
  those files do not already prove as one independent check: custom-root
  redirection (not just "also works"), an integrated canary sweep
  spanning render text/JSON/plan/execute/ledger *and*, for the first
  time, real TUI frames driven by real identification output (every
  existing TUI frame test for the Agents view uses a hand-built
  `AgentUnit` literal, which by construction cannot leak real content),
  nested-accounting agreement between the flat view and the project
  tree, incremental/unchanged growth history, and stable history after
  relinking.
- **Relinking test controls for byte-count confounding.** To isolate
  "attribution change never fabricates a growth delta" from "a
  different-length path changes the file's own byte count," the test
  asserts both project paths are the same string length before
  swapping the declared `cwd`, so any observed growth would have to be
  attribution logic misbehaving, not an incidental size change.

## Benchmarks (this session's dev machine; illustrative, not a
benchmark suite -- same convention as this codebase's other timing
notes)

`crates/core/tests/agent_storage_validation.rs::benchmark_unchanged_refresh_one_session_append_and_blob_growth`,
300 synthetic Claude Code sessions, each padded past 200 KB so only a
bounded per-session read is ever exercised:

```
[measured] agent_storage_validation benchmark: initial 300-session scan 57.5ms,
unchanged refresh 25.8ms, one-session append 31.5ms
```

Consistent with "reads directory names + one bounded header line per
session, never the ~200 KB padding body": all three passes complete in
well under 100 ms, not seconds. An unchanged refresh and a one-session
append cost about the same as the initial scan -- this adapter
re-scans its tool home fully on each call rather than incrementally
diffing it against a prior pass, unlike the main folded-directory walk
(which does use event invalidation). That is an honest, not-yet-
optimized fact about this feature's current cost model, not a
regression: #91's own guardrail is "identification reads directory
names and bounded metadata, never full transcripts," which this number
demonstrates, not "identification is incremental," which it does not
yet do. A future worker adding true incremental agent-storage discovery
(mtime-gated re-identification, or reusing the event bus) should treat
this note as the baseline to beat, not assume the current cost is
already bounded by anything other than "sessions are small to stat."

For reference, `crates/core/src/agents/*.rs`'s own per-adapter
`identification_cost_is_bounded_for_many_sessions` tests (from prior
chunks) measured comparable per-adapter numbers at 500 sessions; this
session's own number is a fresh, independent measurement at 300
sessions on the same machine, not a re-report of an old number.

## Verification

`cargo fmt --all --check`, `cargo test --workspace --locked`, `cargo
clippy --workspace --all-targets --locked -- -D warnings`, `cargo run
--locked -p swamp-source-audit`, and `scripts/check.sh` were all run
against this chunk's final state on `integration/full-scope` in
`/Users/muness1/src/open-horizon-labs/swamp-tui-build-decisions` -- see
the worker's final report for exact command output and commit hashes.

## Follow-ups for later workers

- **Human review is still required** before #90 can close -- see
  `docs/agent-storage.md`'s new "Human review still needed" section for
  the complete list (retention-consequence wording, unresolved format
  assumptions carried verbatim from `FOLLOWUPS.md`, and the fact that
  no real installed tool has ever been checked against these adapters,
  by the hard privacy rule). This chunk did not fabricate that review;
  it names exactly what remains.
- **Agent-storage identification is not yet incremental** the way the
  main folded-directory walk is (event invalidation, folded aggregates
  reused across unchanged containers). It is *bounded* (never reads
  full transcripts), which is what #91's guardrail actually requires,
  but a large real `~/.claude` (thousands of sessions) would still pay
  a full re-scan on every `report --view agents`/`propose` call. Worth
  revisiting if a real installation's session count makes this
  noticeable -- not fabricated as already solved here.
- **Linux paths** for Cursor/Windsurf/Cline/Roo Code (and Linux
  verification of every other adapter) remain the independent Linux
  track's job (#77-#89).
