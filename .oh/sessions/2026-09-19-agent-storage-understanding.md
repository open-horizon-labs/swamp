# Agent-tool storage understanding and selective cleanup

## Aim

Explain agent-generated storage growth and let users retain/remove identified units without losing retained work or private state. Independent AS0 strategy under W0/outcome #40, delivered by epic #90. Planning only.

## Problem Space

User requested ~/.claude, ~/.codex, ~/.omp and OpenCode modeling plus cleanup, then confirmed Oh My Pi and instructed all major tools, starting with Claude Code/Codex/Oh My Pi. Full named matrix: those priority three, OpenCode, Gemini CLI, Pi, Aider, GitHub Copilot CLI, Cursor, Windsurf, Cline, Roo Code and Continue. This is explicit required scope, not universal completeness; more tools can enter the catalog without guessing formats.

Tool homes mix caches with unique sessions, attachments, checkpoints, configuration and credentials. Metadata sources/layouts vary by tool/version. Shared blobs, worktrees, editor profiles, remote hosts and active databases make generic directory deletion unsafe. Collect bounded identity metadata, not conversation/secret contents; no hooks, state migrations or recovery side effects during observation. Local planning inspection only checked named-root presence and a CODEX_HOME setting; no session/auth contents were read.

## Solution Space

Selected: independent domain adapters with shared nested measurement/history and explicit action boundaries; identify first, expose retention consequences, then supported reviewed cleanup. Reuse existing compatible contracts/libraries and native scoped operations. Reject whole-home cache labels, age-as-obsolete, partial-reference blob GC, executing cleaner defaults or application hooks, and silent candidates-only treatment of the required tool set.

Coordinate #43/#53/#64/#74/#85 but do not require those full workstreams or duplicate stores. MAC/Linux path/provider differences stay behind platform seams. No migration support or automatic retention. A lack of precise action capability can legitimately leave a version/category inspection-only; silently omitting a required adapter cannot.

## Selected S&T

AS0 selected, parent W0: understand agent-generated storage and selectively retain/remove it. AS1 selected, parent AS0: versioned discovery, classification and trustworthy history; necessary to distinguish generated storage from unique work, plausible through bounded read-only adapters. AS2 selected, parent AS0: inspect relationships/use/retention and precise cleanup; necessary for informed decisions and exact scope, plausible through explicit evidence and existing action protections. AS3 selected, parent AS0: privacy/integrity/performance/usefulness validation; necessary because rows alone do not establish safety/value, plausible through fixtures, measurements and human review.

GAS = AS1 + AS2 + AS3, all required and selected. Owners unassigned; review trigger for all: unsupported schema, private-content leakage, false history, corrupt references, unsafe action or unacceptable cost. Existing W1/W1b/W2a/W2b/W3 contributions are retained through shared contracts; this is a distinct independently deliverable strategy, not part of build cleanup #74.

## Risk retirement

Planned adversarial checks must reject all-home-is-cache, old/archived-is-unused, guessed project ownership, nested/shared double counting, metadata-as-growth, prompt/secret leakage, individual database-sidecar deletion and shared-blob GC from partial coverage. Use custom roots, multiple profiles, append-in-progress, active process/database, missing metadata, shared blob/worktree and unknown-version fixtures. Protect credentials/config/skills/automation definitions by default; unique history removal requires explicit selection and warnings. Human usefulness and retention tolerance remain reviewed separately from test correctness.

## Plan

**Outcome:** [#40](https://github.com/open-horizon-labs/swamp/issues/40)
**Epic:** [#90](https://github.com/open-horizon-labs/swamp/issues/90)
**Updated:** 2026-09-19

| S&T Step | Disposition | Issue/Epic | Parent Step | Depends On |
|---|---|---|---|---|
| AS0 | selected | [#90](https://github.com/open-horizon-labs/swamp/issues/90) | W0 | its own children |
| AS1 | selected | [#91: Discover agent-tool storage and model nested units with privacy-preserving history](https://github.com/open-horizon-labs/swamp/issues/91) | AS0 | none |
| AS1 | selected | [#92: Identify Claude Code sessions, recovery data, caches and managed project storage](https://github.com/open-horizon-labs/swamp/issues/92) | AS0 | #91 |
| AS1 | selected | [#93: Identify Codex session, worktree, cache and state storage without exposing private content](https://github.com/open-horizon-labs/swamp/issues/93) | AS0 | #91 |
| AS1 | selected | [#94: Identify Oh My Pi storage and shared session-blob relationships](https://github.com/open-horizon-labs/swamp/issues/94) | AS0 | #91 |
| AS1 | selected | [#95: Identify OpenCode data, cache, sessions and snapshot storage with version-aware boundaries](https://github.com/open-horizon-labs/swamp/issues/95) | AS0 | #91 |
| AS1 | selected | [#96: Model Gemini CLI, Pi and Aider storage with tool-specific retention boundaries](https://github.com/open-horizon-labs/swamp/issues/96) | AS0 | #91 |
| AS1 | selected | [#97: Model GitHub Copilot CLI storage and separate caches from user session state](https://github.com/open-horizon-labs/swamp/issues/97) | AS0 | #91 |
| AS1 | selected | [#98: Model Cursor and Windsurf agent storage separately from editor configuration and workspace data](https://github.com/open-horizon-labs/swamp/issues/98) | AS0 | #91 |
| AS1 | selected | [#99: Model Cline, Roo Code and Continue storage across editor profiles and extension hosts](https://github.com/open-horizon-labs/swamp/issues/99) | AS0 | #91 |
| AS2 | selected | [#100: Expose agent-storage size, growth, project links and retention consequences in CLI/TUI/MCP](https://github.com/open-horizon-labs/swamp/issues/100) | AS0 | #92, #93, #94, #95, #96, #97, #98, #99 |
| AS2 | selected | [#101: Plan and execute precise agent-storage cleanup with session and database integrity protections](https://github.com/open-horizon-labs/swamp/issues/101) | AS0 | #100 |
| AS3 | selected | [#102: Validate agent-storage modeling, privacy and selective cleanup across the named tools](https://github.com/open-horizon-labs/swamp/issues/102) | AS0 | #101 |

## Handoff

### Required project linkage (user clarification)

Project linkage is required across named adapters, not an optional display field: resolve session/workspace metadata to checkout/worktree and canonical swamp project identity, then link checkpoints/attachments/shared blobs through explicit session references. Support project-to-agent-storage and agent-unit-to-project navigation, filtering and size/growth inspection. Prioritize Claude Code, Codex and Oh My Pi while retaining the whole named tool matrix.

Keep direct versus derived relationships, source/freshness, shared consumers and unresolved/moved/deleted/remote-host references visible. Do not infer ownership from basename, force global credentials/config into a project, read prompt/secret content for attribution, double-count worktrees/shared blobs, rewrite old byte history on relinking, or cascade deletion from project association. Tests cover alternate clones, external worktrees, same-name unrelated projects, shared attachments, multi-project sessions and partial evidence. Current CLI/skill direction is governed by standalone #103/#104.

Updated existing epic #90 and tasks #91–#100/#102 rather than adding duplicate linkage work. All existing privacy, active-use, authorization and recovery protections remain.

Start shared contract #91, prioritize #92/#93/#94, then remaining named adapters. Inspection may progress on completed adapters; full completion of #100/#90 requires the agreed matrix, not just the first three. #101 is planned cleanup implementation, not authorization to execute cleanup now. #102 provides independent validation. Split oversized grouped-tool issues while retaining each adapter and acceptance coverage.

Primary evidence: https://code.claude.com/docs/en/settings ; https://github.com/openai/codex ; https://opencode.ai/docs/troubleshooting/ ; https://github.com/can1357/oh-my-pi/blob/main/docs/session.md and docs/settings.md. Verify supported installed versions and source formats during implementation. No private data is included in issues/fixtures. No application code, dependencies, services or real storage changed during planning.

## Reconciliation, 2026-09-21

#103/#104 landed: `crates/mcp` is removed. Where the #100 table entry and
elsewhere in this session say "CLI/TUI/MCP", read that as "CLI (interactive
and `--json`), TUI, and the `skills/swamp/` agent skill" -- no MCP server to
build or expose this epic's adapters through. Every domain requirement above
(project linkage, direct/derived relationships, unresolved/moved/deleted
references, no basename-inferred ownership, no double-counting, no
history-rewrite on relink) is unchanged; only the transport changed.

## Reconciliation, 2026-09-25

Codex linkage (#93) was not delivering: on the owner's machine all 3,450
Codex session units were `unresolved`. Cause: the rollout's first
`session_meta` record is longer than the adapter's 8 KiB read bound
(current Codex writes `payload.base_instructions.text`, the project's
instructions file, into that record -- 22 KB median, 48 KB max on a
structural probe of the 100 most recent rollouts), and the parser
required the whole line to parse. The `cwd` was at 220-334 bytes in
every one of them. Fixed by bounded early extraction of exactly the
supported `session_meta` `payload.cwd` (or `payload.meta.cwd`): the
record is streamed one byte at a time and the read stops at the `cwd`'s
closing quote, so nothing past it is fetched from the file (an earlier
same-day cut read the 8 KiB prefix and parsed only the field out of it,
which the owner correctly rejected -- the contract is about what is
read); the 8 KiB ceiling is unchanged, and the >8 KiB canary test
asserts the counted bytes end exactly at the closing quote. Corroboration candidates
recorded, none adopted: `payload.git.{branch,commit_hash,
repository_url}` (92/100 records, 18-48 KB in -- beyond the bound),
`payload.forked_from_id` (9/100; a session id, not a project),
`turn_context.cwd` per turn (9/100 within 64 KB; beyond the first
line). Claude Code linkage gained folder-name inference the same day
(`2026-09-25-agent-folder-inference.md`): 121 -> 2 unresolved, labelled
`inferred`, re-derived every pass. Evidence in
`.oh/sessions/2026-09-25-codex-early-cwd.md`.
