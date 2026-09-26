---
name: swamp
description: Investigate disk usage across a developer's Git projects with the swamp CLI -- what grew, which project/worktree it belongs to, build vs dependency vs Docker breakdown -- and explain what removing something would cost. Swamp reports; the human removes (in its TUI, or by hand) -- this tool never deletes anything. Use this whenever asked about disk space, what's using storage, what grew recently, stale build artifacts, or cleaning up a dev machine, when a `swamp` binary or `~/.local/share/swamp` store is present or mentioned.
---

# swamp: disk growth, by project

Swamp watches Git checkouts, linked worktrees, build output, dependency
trees, and Docker objects, and answers "what grew, where, and what would
it cost to remove it" -- never "what's safe to delete". It is a bounded
evidence tool, not a forensic oracle: absence of evidence for use is not
evidence of disuse.

**Swamp reports; the human removes.** The CLI and this skill are
inspection-oriented: `swamp report` reads stored facts; `swamp observe`
scans and writes observations, history, and enrichment under `SWAMP_DIR`.
`swamp inspect-cargo` performs bounded read-only filesystem inspection.
`swamp protect add/remove` writes a human keep-list; configuration and
scheduling commands can also write state. None of these inspection commands
deletes scanned data. Removal is performed by a human in
swamp's TUI (Space marks, Backspace shows current facts, Enter moves to
the Trash) or a human running a shell command themselves. There is no
`propose`/`approve`/`execute`/`grant` command any more; do not invent
one.

## Inspect first, always

Start with inspection. Observation writes the report store, not cleanup:

```sh
swamp scope --json                                      # what's in scope, and why -- check this first
swamp observe <root> --since 24h                         # scan and persist an observation
swamp report <root> --view grown --json                  # what grew, plus coverage
swamp report <root> --view projects --json             # ranked project list
swamp report <root> --view worktrees --json             # branch/idle/PR/merge facts
swamp report <root> --view agents --json                # agent storage identified in this same scope
```

`swamp observe` scans and refreshes the stored report; `swamp report`
is a pure read of whatever the last `observe` wrote
and does not recursively walk roots or inspect artifacts. It checks
candidate-root presence and the comparison namespace while resolving the
stored report. On a scope never observed, `report` prints
`no observation yet for <scope>; run swamp observe` (`--json`:
`{"error":"no_observation",...}`) and exits 2 -- run `observe` and
re-run `report`, never assume a scan happened implicitly.

`<root>` is a directory tree to scan (a `~/src`-style parent of
several checkouts, or one checkout) and is optional: omit it and
`report`/`observe`/`ui`/`schedule` resolve swamp's configured effective
scope instead (built-in roots, detected tool locations like Cargo/
rustup/Homebrew, and `config.toml`'s `[scan]` additions/exclusions).
`swamp scope --json` shows exactly what that resolves to, with
provenance for every root -- run it before trusting an implicit root.
For an ad-hoc multi-root scope, pass the same explicit roots to both commands:

```sh
swamp observe /path/to/main /path/to/checkout --since 24h
swamp report /path/to/main /path/to/checkout --view projects --json --limit 10
```

Omitting roots selects configured scope, not the last ad-hoc scope.
JSON report results and structured errors go to stdout; diagnostics and
clap argument errors go to stderr. Check both exit status and the JSON
body; a nonzero exit does not imply empty stdout. Full schemas, pagination and
error/exit-code contract, and `swamp scope`'s own schema: see
`references/commands-and-json.md`. Narrowing what you see with
`--project`/`--filter`: see `references/filters.md`. What "since"/
"history"/partial coverage/scope actually mean before you trust a
growth number or an implicit root: see
`references/coverage-and-history.md`.

## Explaining what removal would cost -- never proposing to do it

There is no plan, no approval, no execution and no grant left in
swamp: gather evidence and explain the consequence of removing
something (rebuild cost, redownload, lost session/checkpoint history,
lost emulator data, unpushed commits, no remote to restore from) in
plain words -- never a verdict like "safe to delete" or "unused", and
never framed as something you or swamp could do. If a human wants
something gone, tell them exactly how: open the TUI (Space the row,
Backspace to see the current facts, Enter to move it to the Trash), or
the precise path to remove by hand. Never claim to run, or offer to
run, a cleanup command -- none exists.

Each row/unit carries an `evidence` array (activity, consumer,
current-use, recovery, reclaimability facts with source and freshness)
-- read `references/evidence.md` before summarizing what keeping or
removing something would actually mean. What went where and how to get
it back after a human used the TUI: `references/cleanup-and-recovery.md`.

Before explaining build-container contents or removal, read
`references/build-artifacts.md` for accounting bases, timestamp sources,
consequences, and limits on inferred identity and rebuildability.

## Writes and removal

`swamp report` does not perform cleanup. Observation and administrative
commands write their own state; the TUI has a separate removal flow.
Do not describe the whole executable as read-only or unable to delete. See
`references/trust-model.md` for the full statement of what swamp is now
and what changed.

## Reference index

Load a reference only when the task needs it -- this file alone is
enough for read-only investigation.

| Reference | Load it for | Measured size (`wc -c`) |
|---|---|---|
| `references/commands-and-json.md` | Full command/flag/JSON-schema reference including `swamp scope`, the historical MCP-tool-to-CLI-command mapping, exit codes | 11.4 KB |
| `references/cleanup-and-recovery.md` | Where a TUI Trash move went (ledger, envelope/`restore.json`) and how to restore it -- no CLI command deletes anything | 5.1 KB |
| `references/trust-model.md` | Report reads, observation/state writes, and the separate TUI removal flow | 4.6 KB |
| `references/coverage-and-history.md` | `since`/history-window resolution, partial/unknown coverage fields, reconciliation, scope/coverage-change notes, what a growth number does and doesn't prove | 6.8 KB |
| `references/filters.md` | The filter expression grammar (`kind:`, `growth >`, `idle >`, `merge-complete`, `pr:`, ...) | 2.8 KB |
| `references/agent-storage.md` | Coding-agent-tool storage (Claude Code sessions/caches/logs/protected config): categories, project linkage, `swamp protect` | 3.4 KB |
| `references/build-artifacts.md` | What is inside a build container (Cargo, Node, Gradle, Maven): role families, accounting basis, timestamp source, removal consequences, shared stores, and what is never inferred | 3.6 KB |
| `references/evidence.md` | The activity/consumer/current-use/recovery/reclaimability evidence contract: what each fact's `status`/`source`/`freshness` actually establishes, and what it does not | 5.2 KB |

Load references independently as needed. Size estimates above are
historical; use `wc -c` for current byte counts.
