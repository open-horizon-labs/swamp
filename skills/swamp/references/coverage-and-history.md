# Coverage, history, and what a growth number proves

Swamp keeps current-state + reverse-delta history per volume under
`$SWAMP_DIR`. A growth number is only ever a comparison between two
real observations -- never a projection, and never a fact conjured from
a single snapshot.

## `since` resolution

`observe` resolves `since` in this order: the
explicit `--since` you passed, else `config.toml`'s configured
`since`, else the hard-coded default (`24h`). `report --json`'s
envelope always echoes the *effective* value back as `"since"` --
never assume your raw argument is what was actually used; read the
field. `report` has no `--since` flag and reads the stored comparison.

## History span vs. asked window

`--view grown --json`'s `coverage.history` block:

```json
{
  "history_secs": 0, "asked_window_secs": 3600, "effective_window_secs": 0,
  "note": "asked for 3600s of growth but the store holds 0s of observations; growth is reported over 0s"
}
```

- `history_secs`: how much history this store actually holds for this
  root's volume (`null` if there is none at all).
- `asked_window_secs`: your `--since`, parsed.
- `effective_window_secs`: the window growth was *actually* computed
  over -- the shorter of the two.
- `note`: set whenever `asked_window_secs > history_secs` (you asked
  for more than the store can honor) or when there is no history yet
  ("no observations yet: growth cannot be reported"). Always check this
  before quoting a growth figure to a human as if it covered the window
  they expect.

History ends at the report's stored `observed_at`, not the current wall clock.
Reading the same observation later does not create more history. Multi-root
reports include per-root spans and use the shortest common measured span;
a root with unavailable history makes common history unknown.

A brand-new store's very first observation has `history_secs: 0` --
this is correct, not a bug: there is nothing yet to diff against. A
"no growth" result under these conditions describes an absence of
comparison, not an absence of change.

## Coverage changes are not storage changes

Re-observing an unchanged filesystem, restarting swamp, enriching
GitHub/Docker facts after the fact, or simply letting time pass never
generates a byte-history delta or a tombstone on its own. If a number
changes between two calls with nothing on disk actually different,
that is a defect to investigate, not an expected refresh artifact.

This also covers *scope* changes: adding a root to `config.toml`'s
`[scan]` table, a detector newly resolving a location, or excluding a
path are changes in what swamp *looks at*, never a change in what
exists on disk. `observe` persists the resolved scope under `$SWAMP_DIR`
and prints a one-line note on stderr
when it differs from the last one:

```
coverage changed since last observation: +root /Users/you/.cargo (detector cargo-home), -root /Users/you/old-project (excluded)
```

`swamp scope --json` shows the full resolved scope on demand (roots,
statuses, reasons, the detector catalog, and its version) without
needing to diff two observations yourself -- see
`commands-and-json.md`'s `swamp scope` section. A root omitted from a
`report`/`observe`/`ui`/`schedule` call resolves this same scope, one
shared code path for every command; never assume an agent's or
another command's idea of "the roots" without checking `swamp scope`.

## Multi-root observation and per-root coverage

With no explicit roots, `observe` observes the whole configured scope;
`report` reads its stored observation without scanning. For an ad-hoc
multi-root scope, supply the same roots to both commands:

```sh
swamp observe /path/to/main /path/to/checkout --since 24h
swamp report /path/to/main /path/to/checkout --view grown --json
```

A rootless report selects configured scope, not the last ad-hoc scope.
`report --json` sums
every present root's bytes into one report (order-independent, each
root counted exactly once) and adds a `scope_coverage` array whenever
any root is not simply, cleanly observed. Each entry:

```json
{"path": "/Users/you/old-project", "status": "missing", "walked_total": 0, "projects": 0, "mode": ""}
{"path": "/Users/you/locked", "status": "inaccessible", "reason": "permission denied", "walked_total": 0, "projects": 0, "mode": ""}
```

Five statuses: `complete`, `partial` (walked, but part of it could not
be read this pass -- its rows are left untouched, not tombstoned),
`excluded`, `missing`, `inaccessible` (with a `reason` string on the
latter two/`partial`). None of these is ever reported as bytes going to
zero or a tombstone: a `missing`/`inaccessible` root's previous history
(if any) is untouched, and a root that only lost read access -- the
path still exists, it just could not be listed -- never counts as
deletion, and regaining access never counts as regrowth. Treat any
non-`complete` row as "not fully known this pass", never as "gone".

## Reconciliation and unknowns

`--view reconciliation --json`: `{attributed, unowned, walked_total,
du_total, docker_attributed, docker_unowned}`. `du_total` is `null`
unless `observe --verify-du` (slow, runs a real `du -skPx`) was used.
`report --verify-du` only displays the stored verification total.
`walked_total` not matching `attributed + unowned` closely is itself
useful evidence, not a failure to hide -- it usually means permission
denials or an in-progress walk.

`--view unowned --json` rows carry an explicit `reason`
(`outside-any-checkout`, `owned-by-nothing`, `inconclusive-evidence`,
`no-containing-repo`, `shared-cache`, `permission-denied`,
`docker-no-join`) -- never attribute an unowned row to a project by
name similarity yourself; that's exactly what `--view docker`'s
explicit `"unowned, name-alike"` labelling exists to prevent you from
doing silently.

## External/shared storage units

Storage with no containing project (the Cargo registry, rustup
toolchains, a Homebrew prefix, a language-version-manager data dir, a
model cache, ...) is a first-class **external unit** (`report --view
external`/`--json`), identity `(detector, category, canonical path)`,
independent of any project or worktree. It carries its own
size/growth/regrowth history under the same coverage rules above, and
zero or more declared `consumers` -- adding or removing a consumer
never duplicates the unit or resets its history. External units are
**never folded into `reconciliation`** (not summed into
`walked_total`/`attributed`/`unowned`): the `--view external` total is
a separate, additive figure, not a double count of anything above it.

A detector-resolved location that falls *inside* an ordinary scan root
(e.g. Homebrew's `~/Library/Caches/Homebrew` inside the built-in
`~/Library/Caches` default, or Cargo home's own `registry`/`git`
subdirectories inside its own base directory) is pruned from that
root's walk rather than counted twice: `swamp scope --json`'s
`external_pruned_subtrees` names exactly which subtree was pruned from
which root and which detector separately measures it, and
`report --json`'s top-level `notes` array carries a matching one-line
entry for the same reason. The same "count it once" rule applies
*among* external units themselves: a location nested inside another
detector-resolved location (mise's `installs`/`downloads` inside its
own data dir, Hugging Face's `hub` cache inside `HF_HOME`) is excluded
from that outer location's own measurement, so summing every unit in
`--view external` never double-counts a nested one.
No command deletes one. In the TUI a person can mark a unit or a listed
folder for the reviewed Trash move (the confirm says what swamp does not
know); an agent must never act on one itself -- relay the facts, and the
manager's own command where it has one.

## Filesystem vs. Docker accounting

Filesystem and Docker sizes are separate accounting domains
(`reconciliation.docker_attributed`/`docker_unowned` vs. the
filesystem totals) and should not be summed to predict how much
physical disk space an action will actually reclaim -- hardlinks,
shared image layers, and Docker's own storage driver mean the
relationship is not additive.
