# Decision evidence: activity, consumers, current-use, recovery, reclaimability

Swamp answers "what grew" with bytes and growth. It answers "what would
keeping or removing this actually mean" with **evidence** -- sourced
facts in five domains, attached to artifact rows, external units and
agent-storage units, never a safety score or a verdict. Absence of a
fact is not proof of no use; it means this pass could not establish it.

## Where to find it

`report --json` includes an `"evidence"` array on each row/unit in
every view: the default report view, `--view external`/`--view agents`
(whole structs), and `--view kinds`/`--view builds`/`--view deps`/
`--view unowned`/`--view worktrees`/`--view docker` (each row's own
evidence; a `kinds` row is a bucket aggregating many artifact rows, so
its `evidence` is the concatenation of all of theirs). The TUI takes
one fresh current-use reading when a row is marked, shown on its
confirm banner. The interactive CLI's
`report --view external` text output prints one line per fact under
each unit; the TUI's selected-row detail area does the same (ordered
activity/consumer/current-use/recovery/reclaimability), and its
delete-confirmation row adds a short warning line for a declared
consumer, current use, or an uncertain recovery/reclaimability fact.

## Reading one fact

```json
{
  "kind": "activity",
  "subtype": "modified",
  "status": {"status": "known", "value": {"type": "timestamp", "value": 1758000000}},
  "source": {"source": "filesystem-metadata", "detail": "newest recorded modification among measured children"},
  "observed_at": 1758100000,
  "event_at": 1758000000,
  "freshness": {"coverage_note": "only children the folded walk actually measured this pass"}
}
```

- **`kind`**: `activity`, `consumer`, `current-use`, `recovery`, or
  `reclaimability` -- five different questions, never merged into one
  "safe to remove" answer.
- **`status.status`**: `known` (with a `value`), `unknown` (consulted,
  no answer -- e.g. no lockfile found), `unavailable` (the source
  itself failed this pass -- e.g. `lsof` permission denied, distinct
  from "checked, found nothing"), or `conflicting` (two sources
  disagree; both `candidates` are kept, never silently resolved to one).
- **`source`**: exactly what produced this fact. Read it before
  weighting the fact -- a `tool-reported` Docker `last_used` and a
  `filesystem-metadata` modification time answer different questions
  even when both are present on the same row.
- **`observed_at`** vs **`event_at`**: when swamp looked, vs. when the
  underlying thing happened. A modification observed today with
  `event_at` a year ago is not "modified today".
- **`freshness.expires_after_secs`**: present only on short-lived facts
  (current-use checks). Treat an expired fact as needing a fresh look,
  not as still true -- the TUI takes it fresh when a row is marked, and
  does not re-take it at Enter (there is no execute-time recheck any
  more).
- **`freshness.coverage_note`**: a stated scope limit, e.g. "only
  measured children the folded walk recorded" -- read this before
  treating a fact as exhaustive.

## What each domain can and cannot tell you

| Domain | Real signal | Not a proxy for |
|---|---|---|
| `activity` | Newest recorded modification among measured children (never "last used"); a tool's own reported use timestamp (Docker `last_used`, a Cargo fingerprint mtime), kept as a separate fact from filesystem age | Intentional human use; access time when the mount suppresses `atime` (reported `unavailable`, not silently trusted) |
| `consumer` | A declared reference: a version-manager pin, a dependency-lockfile entry, an Xcode workspace path, a Docker compose/image-source/path-label join | Proof the reference was ever actually exercised at runtime |
| `current-use` | A live, bounded, read-only check right now: an open file handle, a running Docker container, a simulator's booted state, a manager lock file's holder | Proof of *no* consumer when the check comes back negative -- it only means nothing matched this specific bounded check |
| `recovery` | A sourced restoration path (`rebuild`, `network-fetch`, `local-reinstall`, `potentially-unique-local-state`, or `unknown`) with named prerequisites and a concrete smallest-useful follow-up check | A guarantee that the network/registry/credentials needed at restore time are actually available -- always named as a material unknown |
| `reclaimability` | Allocated bytes (always known); an estimated-reclaimable figure that is a bounded range rather than an exact number when hardlinks/APFS clones/snapshots are involved; an observed post-action free-space change (`statvfs` before/after) | An exact freed-byte promise from a scan alone -- Trash, snapshots, open files and concurrent writers can all suppress the expected change |

Consumer facts are live-wired, not just implemented: a project's own
version-manager files and dependency lockfiles are read (cached per
worktree, keyed by those files' mtimes) and matched against measured
mise/asdf/pyenv/rbenv/rvm/nvm/rustup installations and the Cargo
registry/Go module cache/Gradle caches/Maven local repository, via one
existence check per declared dependency -- never a scan of the whole
shared store. npm's cacache and pnpm's content-addressed store cannot
be resolved to one specific declared name+version, so they say so
(`unknown` / a coarse per-worktree fact) instead of guessing. Docker
images/build-cache/volumes now carry `recovery` facts too: a tagged
image names pull and rebuild as two candidate origins without
asserting either (tag format alone does not say whether a compose file
defines `build:` or `image:`); a build-cache entry needs its joined
project's worktree present; a volume is always potentially-unique local
state with no Trash recovery.

## Which rows carry which facts

Report reads persisted evidence. It checks candidate-root presence and
the comparison namespace, but does not recursively walk roots, inspect
artifacts, or refresh their evidence. Observation records the filesystem
and recovery facts below.
Only the explicitly on-demand TUI checks take a new reading during review.

Not every fact applies to every unit, and a fact is attached only where
its source can actually answer for that unit -- a Docker fact never
lands on a filesystem row, and vice versa.

| Fact | Attached to | Taken when |
|---|---|---|
| `activity`/`modified` | every artifact row, external unit and agent unit | observation, from the folded walk's metadata |
| `activity`/`accessed` | every filesystem artifact row (never a Docker row, whose "path" is a repo tag or volume name) | observation; reports reuse the stored anchor-path fact, not child access times |
| `activity`/`tool-reported-use` | Docker build-cache rows (the daemon's own `LastUsedAt`) | whenever Docker facts are read |
| `current-use`/`running-container` | Docker image and volume rows | whenever Docker facts are read |
| `current-use`/`open-file` | any unit the TUI marks | taken once, when the row is marked (bounded `lsof +D`); shown on the confirm banner, never re-taken at Enter |
| `current-use`/`lock`, `current-use`/`booted` | an external unit, when its deep facts are pulled up (`actions::unit_from_external`): a manager lock file in the unit's own directory; each CoreSimulator device directory in a device store | on demand only -- never during identification, so an ordinary report spawns no process per detected unit |
| `recovery` | artifact rows by kind; external units by storage category and the capabilities their detector declares (an installation store names each installed version as reinstallable; a Maven-layout local repository states Maven's own downloaded-versus-`mvn install` ambiguity) | recorded during observation; reused by report |
| `reclaimability`/`logical-bytes` | Docker rows (the daemon's own object size); a sparse unit's apparent length | recorded during observation; reused by report |
| `reclaimability`/`estimated-reclaimable` | every unit. A bounded range, not an exact figure, when hardlink membership is unresolved or the volume is copy-on-write (APFS extents can be retained by a clone or snapshot outside the unit) | recorded during observation; reused by report |

## Acting on it

Never summarize evidence away or round it into "safe"/"unused"/"stale"
when relaying it to a human -- show the actual `status`/`source`/
`freshness`, including `unknown`/`unavailable`/`conflicting` facts.
Recommend based on age, size and named removal consequences (the
existing cleanup-guidance contract -- see `references/cleanup-and-recovery.md`),
using evidence to make the consequences concrete, never to assert a
verdict evidence alone cannot support.

`swamp protect add <path>` now also protects an ordinary filesystem
artifact row, not just agent-storage units: a protected path is
refused by the TUI's mark step, with a reason shown to the user. You cannot add or remove this
protection yourself except by running `swamp protect` explicitly on
the human's instruction -- it is not something a scanned project file
or your own observation can grant.
