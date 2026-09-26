# R19: an unchanged `observe` on the owner's machine (2026-09-24)

Owner-run. Scratch `SWAMP_DIR` with the real `config.toml` (73 detector
roots incl. Homebrew off, `~/src` walked, three CoreSimulator runtimes
installed, Codex and OrbStack running). Release binary. All numbers are
`/usr/bin/time -p` wall seconds of a second pass ≥ 4 s after the first.

| step | unchanged `observe` | why |
| --- | --- | --- |
| stack/26 before R19 (`6a7c4d8`) | **147 s** (sys 77 s; 525k dirs listed, 2.0M files statted) | see below |
| + sealed read-only volumes reused from their statfs stamp | 143 s (43k / 229k) | `/Library/Developer/CoreSimulator/Volumes`: 3 sealed APFS runtime volumes, 482k dirs / 1.77M files, re-walked every pass (no FSEvents window covers another mount) |
| + `annotate_artifact_ecosystems` memoised per parent, shadow rows skipped | **9.8 s** | 138 s: 4,577 nested-artifact shadow rows, each `read_dir`-ing a `target/debug/deps`-sized parent, every pass |
| + per-unit folded cache files, concurrent per-device FSEvents groups, stamps stored for incomplete sealed folds | 6.2 s | `folded.parquet` was read whole per unit (73×); the first sealed walk is incomplete (root-only dirs) and was never stamped |
| + `FSEventsGetLastEventIdForDeviceBeforeTime` once per device (was once per root: 56 × 30 ms), stable volume stamp (device, size, root inode/mtime -- not the APFS container's floating free space), reused-but-incomplete units re-stamped | **3.0 s** (4 consecutive passes: 3.97 / 3.00 / 3.04 / 3.05) | the last two made every other pass walk the sealed volumes again |
| `swamp report` | **0.65 s** (target ≤ 1 s) | R20 derivation; 3,000-artifact synthetic store: 20 ms |

Remaining ~3 s: unit-root FSEvents replay 0.9 s, project root 1.0 s
(`~/src` incremental: a `target/` that changed since cargo built between
passes), external units 1.6 s (`~/Library/Caches` 1.1 s and `~/.codex`
0.5 s genuinely change every few seconds while Codex runs -- their
whole tree is re-folded; partial re-fold from the event window is the
next lever), agents 0.7 s, commit + tables 0.5 s. The ≤ 2 s target holds
for a machine where nothing under the roots is being written; on this
one, two roots are always being written.

A pre-existing product bug surfaced by the trace: the
`/Library/Developer/CoreSimulator/Volumes` unit rendered **0 B "could not
be read"** on every pass (a sealed runtime volume has root-only corners,
so its fold is never "complete", so the unit was "protected" with no
prior complete value to show) while the adapter listed 45.6 GB beneath
it. An incomplete fold is now shown as a flagged lower bound when there
is no complete measurement, or when it exceeds the last complete figure;
it is still never a growth input (`external_units.rs::
an_unreadable_interior_reports_a_flagged_lower_bound_not_zero`).

Aim-review items closed here: P0 Claude session `cwd` (first record
that carries one; test with `queue-operation` first lines), P1 agents
ranking (largest first), P1 future-mtime label (1 h tolerance), P1
"no declared consumers" contradiction, P1 "/" unowned row, P1 coverage
note noise, P1 Debug labels in `swamp scope`. Not done: the multi-root
header still prints the first root (a `Report` has no roots list; the
coverage block above it has them), and the per-row accounting-basis
note (P2).
