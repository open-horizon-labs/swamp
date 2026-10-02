# Agent-tool storage

Coding-agent tools (Claude Code, Codex and its desktop app, Oh My Pi,
OpenCode, Gemini CLI, Pi, Aider, GitHub Copilot CLI, Cursor, Windsurf,
Cline, Roo Code, Continue -- every tool in the required matrix, see
`docs/agent-storage.md`) keep session transcripts, caches, logs,
checkpoints and configuration under their own home directory (Aider's
per-repo files live inside each project checkout instead). Swamp
identifies this separately from ordinary project storage, links
sessions to the swamp project they belong to where evidence supports
it, and offers a narrow, supported cleanup path.

**Never read or repeat a session's actual content.** Swamp itself never
puts prompt/response/attachment/credential text into its output; you
must not either, even if you happen to see a path that looks
interesting. Talk about counts, bytes, ages, categories and project
links -- never contents.

## Inspect

These commands use configured scope; observe that same scope first.
For an ad-hoc scope, pass the same explicit roots to observation and
both reports instead. Agent units outside that scope are not included.

```sh
swamp observe                                      # refresh configured scope
swamp report --view agents --json                    # every identified unit
swamp report --view agents --project my-repo --json  # narrowed to one project's linked units
```

Each unit has a `category` (`sessions`, `caches`, `logs`, `checkpoints`,
`attachments`, `plugins`, `protected-config`, `unclassified`,
`managed-worktrees`), a `bytes`/`growth_bytes` pair, a `project_link`
(`linked`/`unresolved`/`missing`/`not-a-project`/`moved`/`remote`/
`shared`/`not-applicable`, never a basename guess; a `linked` state
carries `source`: `declared` when the tool's own metadata named the
path, `inferred` when -- Claude Code only -- no `cwd` was recorded and
the session's `projects/<slug>` folder name re-encoded exactly one
known worktree, recomputed every pass), and `protected`
(true for credentials/config/skills/automation by default, or anything
a human added with `swamp protect`).

Supported cleanup rules cover caches/logs (a category directory the tool can regenerate) and sessions (the transcript with linked recovery material). Session removal discards unique resume/rewind/checkpoint history, never the linked project’s files. Config kept by default, plugins outside their own `.trash` staging area, attachments and unclassified paths can be selected individually; their review says what the tool loses. `A` skips kept-by-default units and those without a cleanup rule, while reviewing eligible rows across the full current list, including off-screen rows.

A project's linked agent storage is also visible from the project tree
itself, not only `--view agents`: `swamp report --project <name>` (text
or `--json`, no `--view` needed) shows a collapsed "Agent storage
(linked)" summary alongside the project's worktrees.

## Human keep/protect intent

```sh
swamp protect list --json
swamp protect add <path>       # survives refresh; blocks the TUI from marking anything under it
swamp protect remove <path>
```

## Removing agent-storage units: TUI only

There is no CLI command that removes an agent-storage unit. Point the human to Agent storage in the TUI: `2`, then `v` three times. Space marks the unit; Backspace opens review, naming the linked project and history lost. Enter confirms after the primary summary has been displayed. `l` opens the optional path/member inventory; Esc returns to the summary, or cancels from there. See [terminal controls](https://github.com/open-horizon-labs/swamp/blob/main/docs/usage.md#terminal-controls).

A session removal that partially fails (some members moved, then a
later one could not be) leaves a `restore.json` manifest inside its
Trash envelope naming exactly which member moved where.

If a unit cannot be marked, the TUI's footer names the exact reason
(`protected by you: ...` for the person's own `swamp protect` mark, or a
path that is gone) -- relay it verbatim, never as "unsafe" or "can't be
deleted". The other facts (kept by default, no rule for the category, a
database-like file) are warnings in review, not refusals.

## Full reference

`docs/agent-storage.md` (in the repo, not this skill) has the complete
category table, project-linkage state table, the required 14-row tool
matrix with a per-tool support level and the upstream source each was
verified against, and every documented gap (Oh My Pi's shared-blob GC
and OpenCode's snapshot/`storage/part` actions are both deliberately out
of scope; Gemini CLI's project id is one-way). Twelve of the fourteen
rows are `supported`; **Cursor and Windsurf are `unverified`** -- their
units are identified and measured, but no supported cleanup rule is offered and project
linkage is reported `unresolved`, because no primary source confirms the
layout those adapters model.
