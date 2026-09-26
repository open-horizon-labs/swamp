# Filter grammar

Used by the CLI's `--filter` (`report`). Combine predicates with spaces
(implicit AND).

| Expression | Meaning |
|---|---|
| `growth > 500MB in 7d` | Grew by more than the threshold; requires a report computed with a `since` window that covers `7d` -- see `coverage-and-history.md`. The `in <duration>` clause does not recompute the baseline itself. |
| `growth < 500MB in 7d` | Shrank by more than the threshold -- not "grew by less than 500MB". |
| `size > 1GB`, `size < 10MB` | Size threshold. |
| `age > 30d` | Artifact's newest recorded modification is older than this. Unknown age does not match -- absence of a timestamp is not evidence of age. |
| `idle > 48h` | Worktree idle time exceeds the threshold. |
| `kind:BuildOutput`, `kind:deps` | Artifact kind by enum name or its short display label (`build`, `deps`, `git`, `cache`, `source`, `ignored`, `untracked`, `docker-image`, `docker-cache`, `docker-volume`, `loose`, `unknown`). |
| `type:rust`, `type:js`, `type:python` | Ecosystem tag. |
| `project:api`, `project:api-*` | Project name substring or glob. |
| `merge-complete` | Combined branch-merge, clean-worktree, and no-unpushed-commits fact. A composite fact, not a verdict -- inspect `--view worktrees`'s `merge_complete.terms` to see what it's built from before treating it as "safe to remove". |
| `pr:open`, `pr:merged`, `pr:closed`, `pr:none` | GitHub PR selection. A `pr:none` match does not itself prove a successful GitHub lookup -- unavailable facts can also produce no PR row; check for `pull_request: "unknown"` in `--view worktrees` output. |

Durations: `30m`, `48h`, `7d`, `1w`. Sizes are decimal by default
(`1MB` = 1,000,000 bytes); use `MiB`/`GiB` for binary units.

## Where filters apply

- `report --json` (with or without `--view`): applied to the whole
  report before any view is computed, so a filtered view and the
  filtered whole-report agree on what exists.
- `report` (text, no `--json`): only the root `--view worktrees` text
  output actually consults `--filter` today; other text views and the
  overview do not narrow by it. Use `--json` for a filter that must
  apply everywhere.
There is no `propose --filter` or `swamp grant add` any more
(removed 2026-09-23): `--filter` narrows what `report` prints, nothing
else.

## Invalid filters

A filter that fails to parse is a hard error: nonzero exit, the parse
error on stderr, nothing on stdout. Never treat empty stdout plus a
nonzero exit as "no results" -- it means the command never ran.
