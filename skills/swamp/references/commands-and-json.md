# Commands and JSON contract

Every command in this reference is noninteractive: it reads (and, for
`observe`, writes) the growth store under `$SWAMP_DIR`
(default `~/.local/share/swamp`), prints exactly one JSON document to
stdout when `--json` is given, and never prompts. Diagnostics, progress
lines, and clap argument errors go to stderr. Structured JSON errors go
to stdout even with a nonzero exit; inspect both the status and body.

`swamp observe` scans and persists report facts (`du`, `gh`, and Docker
inspection may be involved): `swamp observe [--full] &&
swamp report ...` is the shape every recipe below assumes. `swamp
report` reads whatever the last `observe` wrote. It checks candidate-root
presence and the comparison namespace, but does not recursively walk
roots, inspect artifacts, or spawn subprocesses. On a scope
that has never been observed it prints `no observation yet for <scope>;
run swamp observe` (`--json`: `{"error":"no_observation","scope":...}`)
and exits 2, rather than scanning to produce one. `inspect-cargo` is a
separate bounded filesystem inspection; it does not persist an observation.

This table replaces the MCP server that shipped through v0.6.x
(`crates/mcp`, removed in favor of this CLI + skill). Every MCP tool
below maps onto CLI flags that reuse the exact same core logic
(`swamp_core::agent_json`), so results are identical in content, not
just similar in spirit.

| Former MCP tool | CLI equivalent |
|---|---|
| `report` (root, since, project, view, filter, dirs) | `swamp observe <root> [--since S] && swamp report <root> --json [--view V] [--project P] [--filter F] [--dirs]` |
| `what_grew` (root, since) | `swamp observe <root> --since <S> && swamp report <root> --view grown --json` |
| `list_projects` (root, since) | `swamp observe <root> [--since S] && swamp report <root> --view projects --json` |
| `list_worktrees` (root, since, filter) | `swamp observe <root> && swamp report <root> --view worktrees --json [--filter F]` |
| `docker_objects` (root, unowned_only, project) | `swamp observe <root> && swamp report <root> --view docker --json [--unowned-only] [--project P]` |

`since` now belongs to `observe`: growth/regrowth figures are fixed at
the observation that computed them, from whichever window that pass
used (its own `--since`, else `config.toml`'s `since`, else the
hard-coded default). `report` has no `--since` of its own any more.

There is no `propose`/`execute`/`plans`/`grant` command any more (removed
2026-09-23, "swamp reports; the human removes"). `report` reads the store;
`observe` writes observations/history/enrichment; `protect add|remove`
writes a keep-list. Configuration and scheduling commands can also write
state. Removal is a separate TUI operation, or a human's own shell command.
See `trust-model.md`.

`<root>` is now optional on `report`/`observe`/`ui`/`schedule`: omit it
and the command resolves swamp's configured effective scope (built-in
defaults, detected tool locations, and `config.toml`'s `[scan]` table)
instead of explicit paths. For an ad-hoc multi-root scope, repeat the same
explicit roots on `report` that you supplied to `observe`:

```sh
swamp observe /path/to/main /path/to/checkout --since 24h
swamp report /path/to/main /path/to/checkout --view projects --json --limit 10
```

Omitting roots selects configured scope, not the last ad-hoc scope.
`swamp scope --json` (below) is the one
place every one of those commands' root resolution is inspectable.

## `swamp scope [<root>...] --json`

No MCP predecessor -- new in the scope/detector-registry work (#41/#44).
Prints the effective scan scope: every root swamp would use for this
invocation (or, given explicit roots, what those resolve to -- config
`exclude` still applies), each with its filesystem status
(`present`/`missing`/`unreadable`/`skipped-as-nested`/`excluded`) and
every reason it is in scope, plus the full detector catalog (including
`disabled`/`not-present`/`unresolved-with-reason` entries never
promoted to a root) and the catalog version. `disabled_detectors` is
every detector not running this pass; `default_off_detectors` (stack/26)
is the subset of those off because the detector itself defaults to off
(a system-wide install tree -- currently only the full `homebrew`
detector; `homebrew-devtools` and `homebrew-other` are on by default and
split Homebrew into developer tooling and one remainder) rather than
because your config named it -- `[scan] enabled_detectors = ["homebrew"]`
turns it back on. Source roots are declared, never inferred: `swamp config
add-root <path>` / `remove-root <path>` edit `[scan] include`, and `swamp
scope`, `swamp config show` and `swamp report` list each declared root as
present (with stored bytes), missing or unreadable:

```json
{
  "catalog_version": "2026-09-30.1",
  "generated_at": 1758470400,
  "defaults_enabled": true,
  "disabled_detectors": ["homebrew"],
  "default_off_detectors": ["homebrew"],
  "configured_include": [],
  "configured_exclude": [],
  "explicit": false,
  "roots": [
    {
      "path": "/Users/you/src",
      "reasons": [{"source": "detector", "detector_id": "builtin-defaults", "category": "unclassified", "provenance": {"BuiltinConvention": null}}],
      "status": {"state": "present"}
    }
  ],
  "detectors": [
    {"detector_id": "cargo-home", "name": "Cargo home", "locations": [{"detector_id": "cargo-home", "path": "/Users/you/.cargo", "category": "installation", "provenance": "builtin-convention", "status": {"state": "resolved"}, "note": "cargo home: bin/, config.toml, credentials"}]}
  ],
  "pruned_subtrees": [],
  "external_pruned_subtrees": []
}
```

`detectors` covers the full catalog (#45-#49): language version
managers (mise, asdf, pyenv, uv, Conda, rbenv, RVM, ruby-install, nvm,
rustup), shared dependency/build caches (Cargo home, npm, pnpm,
Gradle, Maven, Go, pip), Apple/Android tooling (Xcode, CoreSimulator,
Android SDK), and model/VM stores (Homebrew, Hugging Face, Ollama,
Docker Desktop's sparse backing file, OrbStack) -- see
`docs/locations.md` for the full table of every detector, its
locations, overrides, categories, and documented limits (e.g. pnpm's
per-volume stores, Maven's undecidable downloaded-vs-local split).
`external_pruned_subtrees` names a detector-resolved location that
folded into one of `roots` as a nested subtree and was pruned from
that root's walk because it is separately measured as its own external
unit (`--view external`) -- the mechanism that keeps a location's
bytes counted exactly once instead of twice (see
`coverage-and-history.md`'s "External/shared storage units").

An effective scope with no roots at all (`defaults = false`, no
`include`, every detector disabled) is not an empty `roots: []` --
`report`/`observe`/`scope` all refuse to run with a nonzero exit and an
explicit stderr message, never a silent fallback to the current
directory. See [coverage-and-history.md](coverage-and-history.md) for
the coverage-change notes `report`/`observe` print when the resolved
scope differs from the last observation.

## `swamp report [<root>...] --json`

Always applies `--filter` (if given) to the whole report before
computing any view -- a filtered view and a filtered whole-report
agree on what rows exist. `--project` scopes to one project (matched by
name or `owner/repo` display name). Without `--view`, prints the full
report structure with `since`/`index_refreshed`/`total`/`truncated`
added and the top-level `projects` array bounded by `--limit`/
`--offset`. With `--project`, the envelope also gains `agent_storage:
{units, total_bytes}` -- this project's own linked agent-storage units
(see `agent-storage.md`), the same linkage `--view agents --project
NAME` reports, present here too so a project-scoped query never has to
also pass `--view agents` to see it.

The full report (no `--view`) also carries a `headline` object, the same
numbers as the first line of the text report: `developer_bytes`, `locations`,
`categories` (bytes and counts that add up to `developer_bytes`), `not_counted`
(the remainder unit and mounted images), `mixed_owners`, `percent_of_used` (null
when there is no honest percent: no ledger, an unreadable, newer or future-dated
one, a previous scope, one command-line root, or developer storage above used),
`disk` (`state` and, when measured, the ledger's parts, the spot audit and
`accounted_check`), `flags`, `measured_at` (times, not ages) and `line`. `--view
reclaim --json` adds `headline` and `headline_relation` (`holds`: the Reclaim
totals add up to `developer_bytes`). Percent is of the container's used bytes,
rounded down to one decimal.

### `--view <name> --json`

Every view returns the envelope:

```json
{
  "view": "worktrees",
  "project": null,
  "result": [ /* view-specific: array or object */ ],
  "observed_at": 1234567890,
  "since": "24h",
  "index_refreshed": false,
  "total": 12,
  "truncated": false
}
```

With no explicit root, the envelope also gains `scope_coverage` (an
array) whenever any root in the configured scope is not cleanly
`complete` this pass -- see `coverage-and-history.md`'s "Multi-root
observation and per-root coverage". An explicit single root never
carries this key: nothing about its scope is ambiguous.

`total`/`truncated` are present whenever `result` is an array; they
describe the array's *unbounded* length and whether `--limit`/
`--offset` cut anything off this page -- never assume a page is the
whole answer without checking `truncated`. `since` is the window the
*last observation* actually used (its own `--since`, else
`config.toml`'s, else the 1h/24h/7d default) -- `report` takes no
`--since` of its own, so this is always echoing back what `observe`
computed with, not a per-`report`-call argument. `index_refreshed` is
always `false`: `report` is a pure read (R12) and never persists a new
observation itself.

Views:

| `--view` | `result` shape | Notes |
|---|---|---|
| `worktrees` (default) | array: `{project, path, branch, idle_secs, merge_complete: {verdict, terms}\|null, pull_request, remove_command}` | `remove_command` is text for a human to run, never executed by swamp. `merge_complete` is a composite fact with its terms, never a verdict. |
| `projects` | array: `{name, display_name, project_id, bytes, growth_bytes, checkout_count, worktree_count, remote}` | Ranked growth desc, then bytes desc. JSON only (no text render). |
| `grown` | object: `{grown: [...], unowned_by_reason: {...}, permission_denied_count}` | See below; JSON only. |
| `builds` | array: `{project, kind, path, bytes, growth_bytes}` | `BuildOutput` + `Cache` kinds. |
| `deps` | array: same shape | `DependencyTree` kind only. |
| `docker` | array: `{project, object, kind, bytes, shared_bytes, created_at, shared_with, containers, dangling, note, unowned}` | Add `--unowned-only` to restrict to objects with no join evidence. `project` is `null` unless `--project` is given, in which case unowned rows are labelled `"<name> (unowned, name-alike)"` -- never silently attributed. |
| `kinds` | array: `{kind, bytes, count}` | |
| `types` | object keyed by ecosystem | |
| `unowned` | array: `{path_or_object, bytes, reason, shared_bytes, docker_kind, note}` | |
| `reconciliation` | object: `{attributed, unowned, walked_total, du_total, docker_attributed, docker_unowned}` | |
| `rust` | array of nested Cargo artifacts | Inspection only; not project-scoped by `--project` yet. |
| `external` | object: `{units: [{detector_id, detector_name, category, provenance, path, bytes, growth_bytes, regrowth_count, consumers, note, bytes_counted_elsewhere, overlap_count, last_used: {at, source, atime?}, children: [{kind, name, bytes, measure, mtime_max, entries, not_measured, last_used}]}], total_bytes}` | Storage with no containing project (Cargo registry, rustup, Homebrew, ...). `total_bytes` is separate from `reconciliation` above -- never sum the two. No command removes one; a person can mark it in the TUI for the reviewed Trash move. Removing one means the manager that owns it fetches or rebuilds it again next time. `last_used` is a fact with its `source` (`tool_native:<name>`, `file_atime` or `none`; `at` is null for no record), never a verdict; `children` are the depth-2 rows, whose `bytes` (null exactly when `measure` is `not_measured`) sum with the remainder and adjustment rows to the walk's total for the unit; `bytes_counted_elsewhere` is what worktrees inside the unit hold that is counted under their projects. |
| `disk` | object: `{measured, measured_at, age_secs, complete, container: {total, used, free, data_volume_used, statfs_at}, accounted: {bytes, locations}, everything_else: {bytes, folders, top: [row]}, system_volumes: {bytes, volumes: [row]}, purgeable: row\|null, snapshots: row\|null, not_measured: {count, names, not_yet_measured, not_yet_measured_names, estimate_bytes, estimate_name}, residual: {name, bytes, percent_of_used, bookkeeping_balanced, residual_flag, unexplained_bytes, within_one_percent}, audit: {folders: [{path, ledger_bytes, audited_bytes, difference, percent, outside_tolerance}], skipped, max_difference_percent, audit_flag}, mounted_views: [row], external_volumes: [row], notes, rows: [row], rows_total, rows_truncated}` where `row` is `{path, category, allocated_bytes\|null, overlap_bytes, entries, unreadable, measured_at, age_secs, method, exactness, note}` | Where the whole disk went, from the stored volume ledger (`swamp observe --volume` writes it). A pure read: no root needed, no observation needed, never a walk or a program run. With no ledger: `{measured: false, note}`, exit 0. `allocated_bytes` is null exactly when `exactness` is `not_measured`. `residual.bytes` is signed: negative when shared (cloned) extents were counted once per file. `mounted_views` and `purgeable` are never added. `rows` is the 50 largest by default (`--all` for every row); the totals always cover every row. `estimate_bytes` is null unless something is unreadable or not yet measured; `bookkeeping_balanced` is arithmetic (the parts with the estimate add up to `container.used` within 1%), not evidence about the walk; `audit_flag` is the check that can fail (an independent re-measurement of up to five folders disagreed beyond max(1%, 4 MiB)). The full `report --json` carries the same object as `disk`. |

`--view grown` additionally has a top-level `coverage` block:

```json
"coverage": {
  "walked_total": 123, "du_total": null, "unowned_total": 0, "attributed_total": 123,
  "observed_at": 1234567890, "since": "24h", "index_refreshed": false,
  "history": {
    "history_secs": 0, "asked_window_secs": 3600, "effective_window_secs": 0,
    "note": "asked for 3600s of growth but the store holds 0s of observations; growth is reported over 0s"
  }
}
```

`history.note` is set whenever the asked window exceeds what the store
actually holds, or when the store holds no history at all ("no
observations yet: growth cannot be reported") -- a growth number
without checking this is a number that may describe a shorter window
than you asked for. See `coverage-and-history.md`.

`--view projects`/`--view grown` are JSON-only: in text mode (no
`--json`) they exit nonzero with a stderr message instead of silently
falling back to a different view.

`--view agents` and `--view external` support both text and JSON output.
The Rust text view defaults to the largest 30 units per container;
`--all` shows every unit. JSON pagination uses `--limit`/`--offset`.

### Pagination

Build/dependency `interior.units` arrays are separately bounded to 30 units
per container by default. Use `--unit-limit N --unit-offset N` to page them
without changing the outer container page. Each interior has `units_total`,
`units_truncated`, and `units_offset`; its family summaries still cover the
complete container. For example:

```sh
swamp report <root> --view builds --json --limit 1 --unit-limit 10 --unit-offset 10
```

Cargo's flat `--view rust --json` uses ordinary `--limit`/`--offset` instead.

`--limit N` / `--offset N` bound the array in `result` (or the
top-level `projects` array with no `--view`). Both are only consulted
with `--json`. A truncated page still reports the true `total`; never
treat a `--limit`-bounded call as an exhaustive inventory.

## There is no propose/execute/plans/grant command

Removed 2026-09-23 ("swamp reports; the human removes"): there is no
plan, no grant, no `swamp propose`/`execute`/`plans`/`grant`. A row's
facts (bytes, growth, recovery cost, warnings) are exactly what
`report`'s views already show; nothing produces a separate "plan"
object, and nothing authorizes or executes anything. The only thing
that moves a path to the Trash is a human in the TUI (Space marks,
Backspace shows current facts, Enter moves it) or at a shell. See
`cleanup-and-recovery.md` for where a TUI move went and how to restore
it, and `trust-model.md` for the full statement of what changed.

## Exit codes and error semantics

- `0`: the command ran and, in JSON mode, printed exactly one JSON
  document to stdout.
- `report --json` with no observation: exit `2`, with a structured
  `{"error":"no_observation","scope":...}` document on stdout.
  Text mode prints the missing-observation diagnostic on stderr.
- Clap argument errors (for example an unknown flag): exit `2`, usage
  and error text on stderr, no JSON result on stdout. `--json` does not
  turn command-line parsing failures into JSON.
- Other failures: inspect stderr and any structured stdout error;
  do not assume every nonzero status has an empty stdout stream.
- `inspect-cargo --json` can exit `0` with `coverage.supported: false`
  for an unavailable profile, or `coverage.complete: false` when a
  budget is exhausted. Check coverage and limits before using its totals.

Capture stdout and stderr separately. Parse a nonempty JSON stdout body
even on failure, and check its `error` field as well as the exit status.
There are no CLI plan/grant/execute outcomes to interpret.
