# 2026-09-22 — Linux part 2: watching, continuity, scheduling, Trash, occupancy, packaging (stack/18)

Branch `stack/18-linux-watch-schedule-trash-packaging`, based on
`stack/15-linux-platform-isolation-and-ci`. Input: `CHUNK_K2.md` —
#81, #82, #83, #85, #86, #88, #89 under outcome #77 / epic #78. Part 1
is `.oh/sessions/2026-09-21-linux-platform-isolation.md`.

---

# 1. Three things the code made me change my mind about

**The `trash` crate does the one thing we forbid.** Part 1 recommended
adopting `trash` 5.2.9 for the move, on the strength of its spec-correct
path resolution. Reading `freedesktop.rs` to implement #85:
`move_items_no_replace` falls back to `copy_dir_all` + `remove_dir_all`
on `EXDEV`; a per-volume trash that is not writable falls back to the
home trash across devices (the same copy); `delete` returns `()`, so
the ledger could not say where an item went; and a relative
`$XDG_DATA_HOME` is honoured. So the mover is swamp's own (a rename or
a refusal; `.trashinfo` written `O_EXCL` first; `renameat2(NOREPLACE)`),
and the crate became the **independent reader** in the Linux tests:
every item swamp trashes is listed and restored through
`trash::os_limited`. That is better evidence than adopting it would
have been — a record another spec implementation cannot read is a Trash
nobody can restore from. The AC allows "narrowly required adaptation or
rejection with evidence"; the brief said "adopt". I did the former and
say so here and in the report.

**The literal occupancy rule refuses everything.** "Permission
restrictions are Unknown" read literally made every Linux action refuse
on the CI runner: the runner's `systemd --user` (pid ~1030) and its
`(sd-pam)` are unreadable by the user they run as. A diagnostic step in
`ci.yml` recorded why: `systemd --user` holds `CapPrm 0x800000000`
(`CAP_WAKE_ALARM`), and `(sd-pam)` has identical uids/gids and no
capabilities but is non-dumpable (it changed credentials from root
without an `exec`). That is every Ubuntu login. The rule I implemented
is the kernel's own `ptrace_may_access` for `PTRACE_MODE_READ`: every
uid/gid equal, no capability the reader lacks, dumpable (read from the
ownership of `/proc/<pid>/fd`, which turns root-owned when a process is
non-dumpable). Outside that rule a process is outside the question — the
same boundary `lsof` has without root, which is what Linux used before
and what macOS still uses. A same-credential process whose entries are
still ours and still unreadable (an LSM denial) stays `Unknown`. **This
is an owner decision**: it relaxes the literal AC, deliberately, and the
cost of not relaxing it is that Linux cleanup cannot run on a standard
system at all.

**Writing an audit found a fail-open bug in code it was not written
for.** `occupancy_gaps_are_unknown_never_free` rejected the pre-existing
macOS `lsof` probe: it read its captured stdout with `let _ =
f.read_to_string(&mut s)`, so a capture that could not be read back was
"lsof printed nothing" — `Free`. Fixed in the same commit.

---

# 2. The designs, briefly (the docs have them in full)

- **#81** `live_watch.rs`: a pure state machine over a `Kernel` trait
  (unit-tested on both platforms with a fake kernel) and a raw inotify
  kernel via `libc` (no new dependency). Watch before list; bootstrap
  events kept; epoch = registration *finished*, rounded up. Named
  losses: overflow, watch limit, permission gap, unmount, removed
  watch, root moved, dirty bound (512). **inotify marks exactly the
  directory an entry changed in**, not its parent: the FSEvents-style
  "path and parent" made a write inside `node_modules` re-walk the whole
  worktree (measured: 40 listings instead of 1).
- **#82** `continuity.rs` + `swamp collect`: bounded checkpoint per root
  under `continuity/`; liveness by `flock`; a sync request on the same
  inotify instance; consumption only after the history commit, by epoch
  and sequence; an observation lock per root from reading the baseline
  to committing. Plan table and every refusal in `continuity::tests`.
- **#83** `systemd_user.rs`: units, quoting, marker ownership, rollback,
  linger reporting; all through an injected `Systemctl`. launchd
  compiled on macOS only, systemd on Linux only.
- **#85** `platform/trash.rs`: `Target::{Directory, Freedesktop}`,
  `move_item`, `Envelope`. Every recoverable sink goes through it
  (`trash_backend_owns_every_move`). macOS: the same rename into
  `~/.Trash` under the same names as before.
- **#86** procfs as above; one scan for all anchors of an action.
- **#88/#89** `scripts/package-release.sh`, `release-smoke.sh`,
  `linux-validation.sh`, used by both `ci.yml` and `release.yml`.

---

# 3. Audits

Two new, derived-set form, on the `platform_audits` workspace model:

- `trash_backend_owns_every_move` — sinks derived as callers of anything
  returning `OccupancyState`; no rename of existing data outside
  `platform::trash` in their reach (a write-then-rename publish is
  allowed); inside the backend `std::fs` is inverted (no copy,
  `remove_dir_all`, `write`...), a move's result is honoured, and a
  moved source is never removed. 7 fixtures (6 reject incl. alias and
  discarded-result, 1 accept).
- `occupancy_gaps_are_unknown_never_free` — probes derived by return
  type plus exact callees; std's error-discarding adapters, `if let Ok`
  without `else`, absent-outcome `Err` arms without a "gone" guard,
  `let Ok .. else { Free }`, discarded reads, reads inside macros. 9
  fixtures (8 reject incl. alias and discarded, 1 accept).

`execution_sinks_recheck_live_state` changed in two ways: backend moves
are audited *at their callers* (a call into a backend definition that
moves is itself destructive there), and a discarded recheck no longer
earns credit through the name-keyed walk into its own body — which it
did once `member_occupancy` grew helpers, and which the existing corpus
fixture `01-discarded-protection` caught.

---

# 4. Measured

From CI run 35789903497 (Ubuntu 24.04 runner, ext4; full table in
docs/platform.md, "Linux validation"): install-to-first-report 468 ms;
initial full scan ~280-310 ms; unchanged under a running collector
~197 ms with `changed_dirs=0`; one-subtree mutation ~225 ms, equal to a
reference full walk; collector killed -> `collector_stopped`; new epoch
-> `live_watch_gap`; a real queue overflow -> `watch_queue_overflow`,
incremental again on the next run; 1,481 watches ~1.5 MiB kernel, 8.3 MB
collector RSS; checkpoint 623 B. Minimum glibc 2.39 (`objdump -T`).
Work bounds as tests (`live_watch_cost_bounds.rs`, both platforms):
unchanged 0 listings, build-output change 1, one-worktree source change
19, full walk 714.

---

# 5. What is not done or not verified

- The **user's actual Linux host is unverified**. Everything above ran
  on GitHub's Ubuntu runners. To check a real host: install the archive
  (docs/usage.md), then run `scripts/linux-validation.sh
  <archive> <version> <out-dir>` from a checkout — it uses scratch state
  only, and prints what it measured. Worth checking there in particular:
  whether any process of the user's is withheld *without* being
  non-dumpable or privileged (that would refuse cleanup, correctly), and
  `fs.inotify.max_user_watches` against the size of `~/src`.
- `release.yml` has not run: it triggers on a tag or a manual dispatch,
  neither of which this chunk may do. The same scripts ran in `ci.yml`.
- The reviewer cost test `two_unchanged_full_observations_cost_report`
  still fails on both jobs, as on the base; not edited, gated or
  ignored.
- A nested PID namespace with its own `/proc` cannot be told apart from
  the host; recorded as a limit.
- **A pre-existing, platform-neutral incremental gap** found by the
  collector's equivalence test: in a root with *no* discovered checkout
  (every byte unowned), an incremental observation with changed
  directories does not re-measure them -- `walked_total` came out 24576
  against a full walk's 36864 with a canned FSEvents plan on macOS too
  (`growth::apply_incremental`'s changes outside every worktree go to
  shallow discovery, not to the unowned totals). Not fixed here (growth
  internals, both platforms); the tests use real checkouts, where the
  incremental result equals a full walk. Follow-up for the owner.
- Btrfs/ZFS/XFS/overlay bounds are labelled from `statfs`; no runner has
  those filesystems, so only the pure mapping is tested.
