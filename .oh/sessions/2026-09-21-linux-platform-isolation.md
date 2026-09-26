# 2026-09-22 — what a build can honestly promise (stack/15)

Branch `stack/15-linux-platform-isolation-and-ci`, based on
`stack/13-event-gated-reuse` (`9653cd7`). Input: `CHUNK_K1.md` —
issues #87 (native CI), #79 (platform isolation and the reuse
assessment), #84 (Linux-native locations), #80 (traversal, accounting
and volume identity), under the Linux outcome #77 planned in
`.oh/sessions/2026-09-19-linux-native-platform-support.md`.

---

# 1. What was actually broken

The 2026-09-19 planning note said the walker was "already mostly
portable Unix code" and that FSEvents, launchd, Trash and releases
needed deliberate handling. That was right about the walker and
understated everything else, because the failures were not *missing*
code — they were code that ran and lied.

`cargo check --target x86_64-unknown-linux-gnu` passed on the base
branch before I changed anything. Compiling was never the problem. A
Linux build did the macOS thing:

- `swamp schedule --every 1h` wrote a LaunchAgent plist into a
  `~/Library/LaunchAgents` that no daemon reads, called a `launchctl`
  that does not exist, and printed "Scheduled observation every 1h".
- Free space came from `df -k`'s fourth whitespace-separated field.
  That is macOS's "Available" column. GNU coreutils prints a different
  header and wraps a long device name onto a second line, so the same
  read can return the capacity percentage.
- The trash directory was `~/.Trash`, which no Linux desktop looks in.
- `BuiltinDefaultsDetector` proposed **nothing** on Linux, so a default
  scope had no roots of its own and fell through to whatever absolute
  paths the detectors found. On a GitHub runner that meant `NVM_DIR`
  and `ANDROID_SDK_ROOT`.
- A missing `HOME` put the growth store in the current directory.

Each of those reports success. That is the pattern the whole chunk is
about, and it is why `platform/mod.rs` exists.

---

# 2. The contract, as data

`crates/core/src/platform/` holds the capability table, the
change-observation source and its cursor, scheduling, the trash
location strategy and free space. Data, not `cfg` attributes, for the
reason `locations::Platform` already is: a test on either machine can
ask what the other platform promises. Only `Os::current` is
`cfg`-gated, and only the *backends* compile conditionally.

The decision worth recording is the **cursor type split**:

```rust
ContinuityCursor::FsEventsEventId { event_id, device }   // covers all prior time
ContinuityCursor::LiveWatchEpoch  { opened_at, device }  // covers only t >= opened_at
```

`covers_since` returns `true` unconditionally for the first and
`opened_at <= since` for the second. An inotify watch descriptor is
deliberately not constructible into either: a descriptor identifies a
watch, and says nothing about when it began. Storing one where an event
id belongs is how "swamp was not watching" becomes "nothing changed",
which is the single falsehood the growth store must never contain.

Consequently `RefreshRefusal` gained `NoPersistedChangeHistory`
alongside `UnsupportedPlatform`. The distinction is not pedantry. "No
backend yet" invites someone to write one. "This kernel keeps no
history" is a fact about Linux that #81's watcher narrows and cannot
remove. Which one a build gives is decided in exactly one place,
`fs_events::platform_refusal`, from the continuity source — and the new
audit fails a second copy of that decision.

---

# 3. Reuse: the assessment came first, and it changed two answers

`docs/platform.md` has it in full, with versions and licences checked
live on 2026-09-22. What I want recorded here is where the assessment
*changed* what I would otherwise have done.

**`notify` 8.2.0 — rejected on one omission.** I expected to adopt it.
Its docs and README describe the inotify watch limits and say the Linux
backend "is documented to not be a 100% reliable source". What they
never mention, anywhere, is `IN_Q_OVERFLOW` — there is no named API for
surfacing a queue overflow to the caller. That is the one event that
matters: it is the kernel saying "I lost events", and it is exactly
when a watcher must stop claiming coverage. A library that cannot tell
swamp overflow happened would let swamp vouch for a period where
changes were dropped. #81 should use `inotify` directly. Recorded, not
implemented — the watcher is #81's.

**`jwalk` 0.9.0 — rejected on maintenance, not on merit.** It is the
closest thing to what swamp already has, and under maintenance it would
have been the serious candidate. The repository is archived, and 0.9.0
exists to carry the notice "This crate is no longer maintained or
supported". I deliberately did **not** benchmark it: the number could
not change the decision, and producing it would have meant adding an
unmaintained crate to the lockfile.

**`walkdir` — kept, as the test reference.** It is already in the
normal dependency graph through `gix`, so using it costs no new
dependency. The finding is in
`walk_library_comparison.rs::a_generic_walker_needs_swamps_semantics_to_reach_swamps_number`:
a naive `metadata.len()` sum over a fixture with a hardlink pair and a
32 MiB sparse file reports **more than ten times** swamp's figure; add
allocated bytes, `(dev, ino)` dedup and a same-filesystem boundary and
the two agree exactly. A walker is not a measurement. Whatever library
sat underneath would still need all three rules, so reuse would save
the traversal loop and nothing else.

Measured, same profile for both, warm cache, one CI run each on
2026-09-22:

| Shape | Files | swamp | walkdir + swamp's rules | Ratio |
|---|---|---|---|---|
| 16 dirs × 2,000 files — macOS arm64, APFS | 32,000 | 37.7 ms | 80.0 ms | 2.1× |
| 512 dirs × 40 files — macOS arm64, APFS | 20,480 | 24.5 ms | 58.7 ms | 2.4× |
| 16 dirs × 2,000 files — Ubuntu 24.04, ext4 | 32,000 | 47.9 ms | 92.0 ms | 1.9× |
| 512 dirs × 40 files — Ubuntu 24.04, ext4 | 20,480 | 38.4 ms | 64.5 ms | 1.7× |

Shared runners, so the absolute times are worth little and the ratio is
the point. The margin is narrower on Linux than on macOS -- consistent
with `getdents64` being cheaper than APFS's directory reads, which is
where a single-threaded walker loses most -- and it is still a margin,
in the direction that says the bounded parallel pool is doing work. Every
CI run prints its own figures, so these do not have to be trusted or
maintained.

**`trash` 5.2.9 — adopt in #85.** Its source does the spec's cross-device
rules properly (sticky-bit check, symlink rejection, `.Trash-$uid`
fallback), its errors are typed, and nothing in it falls back to
permanent deletion. One caveat recorded as a constraint on future code:
on Linux it calls non-thread-safe `getmntent`/`getmntinfo`, and its own
docs say a crate calling those from other threads should not use it.
Swamp does not — `activity.rs` reads `/proc/mounts` as a file — so the
caveat is satisfiable today.

**`clean-dev-dirs` 2.8.2 — conventions, not scanner.** It does publish a
real library with a `Scanner`, which I had assumed it did not. The
scanner is still unusable: `src/utils/size.rs` sums `metadata.len()`,
tracks no inodes, and does not set `same_file_system`. Its CLI is also
more careful than the issue implied — it prompts and trashes by default.
`ecosystem.rs` already cites the project for its conventions; that
citation is the reuse that applies.

---

# 4. Traversal: no change was needed, and that is the finding

`walk.rs` already used `st_blocks * 512`, `(dev, ino)` dedup, an
`st_dev` boundary and `symlink_metadata` throughout. I changed none of
it. What did not exist was evidence, so
`traversal_accounting_and_volume_identity.rs` builds one fixture with a
hardlink pair, a 64 MiB sparse file, a two-link symlink loop, a symlink
to the root, a symlink to an interior directory and nested directories,
and checks the total against the filesystem's own `stat` data **and**
against `du -skPx`. `du -s --apparent-size` is used the other way
round, to prove the sparse file is sparse — if the two agreed the
fixture would test nothing. Where `du` is absent the test prints a skip
with the reason rather than passing quietly.

**Volume identity has a limit and it is now an assertion.** The id is a
hash of `(st_dev, canonical path)`. `st_dev` is not stable across
reboots for every Linux mount, so a device-minor change starts a fresh
history for that root. That is the safe direction, and it is a choice:
keying on the path alone would join two genuinely different filesystems
mounted at one path into a single history and report the difference as
growth. A lost baseline is visible to the user; fabricated growth is
not. `volume_identity_depends_on_the_device_and_a_changed_device_starts_a_new_history`
pins it so it cannot be "fixed" without someone deciding to.

**Free space did change, and not the way the brief predicted.** The
brief said to use `statvfs` via `libc`. On Darwin that is wrong:
`fsblkcnt_t` is `c_uint`, so POSIX `statvfs`'s block counts are 32-bit
and a volume above 2^32 blocks — an ordinary 16 TB external disk —
overflows or returns `EOVERFLOW`. macOS uses native `statfs` (64-bit
counts, unit `f_bsize`); Linux uses `statvfs` (64-bit counts, unit
`f_frsize`). Both are cross-checked against `df -k` by
`the_figure_agrees_with_df`, which reads the figure by position among
the numbers rather than by field index — a wrapped GNU `df` row being
the bug that motivated the change.

---

# 5. CI, and the two tests a second OS falsified

`.github/workflows/ci.yml` landed first, deliberately, so every later
push ran natively on both. Ubuntu 24.04 is pinned rather than
`ubuntu-latest`; the macOS runner selection mirrors `release.yml`.
Steps are ordered cheapest-and-most-specific first, because the
workspace suite takes minutes and one failing test hides every later
step — the platform-isolation evidence this workflow exists for now
runs before it. `--no-fail-fast`, because bringing a second platform up,
the useful signal is every failure rather than the first binary that
has one.

Two halves of the isolation check, both in `scripts/platform-isolation.sh`:
the resolved `cargo tree -e normal` graph must contain every crate its
target owns and none another target owns, and the built binary's own
`ldd`/`otool -L`/`nm` output must agree. The Linux binary's runtime
dependencies are asserted to stay within the loader, `libc`, `libm` and
`libgcc_s`, so a dependency that drags in a system library is a CI
failure rather than a support question afterwards.

Two tests were environment-dependent in ways only a second OS exposed,
and both were wrong before I touched them:

- `crates/cli/tests/observe.rs` disabled three detectors *by name* —
  the three that fire on a Mac — while its own doc comment claimed it
  disabled every non-builtin one. On a Linux runner nvm
  (`NVM_DIR=/home/runner/.nvm`) and Android
  (`ANDROID_SDK_ROOT=/usr/local/lib/android/sdk`) fired instead, from
  absolute paths no fixture `HOME` redirects. The test walked 2.6 GB of
  the runner's real SDK for 143 seconds and then failed an assertion
  that had nothing to do with any of it. It now enumerates the registry.
- `external_units.rs` asserted that a deny-list leaves `core-simulator`
  **authorized**. `authorized_roots` drops a candidate whose path is
  absent, so that held only on a machine with CoreSimulator installed.
  It now asserts what the scope *considered*, which is the property it
  was always about.

Also fixed: sixteen clippy lints and three dead-code warnings that
appear only under the toolchain CI installs. CI's `stable` is 1.98.1;
this machine's is 1.92.0, nine months behind. I installed 1.98.1 as a
named toolchain (without changing the default) so both targets could be
linted locally before pushing. **This will recur**: `-D warnings` against
a floating `stable` fails on a day nobody changed the code. Whether to
pin CI's toolchain is a call for the owner, and is in the follow-ups.

---

# 6. Audits

`fsevents_before_full_walk` and `scheduled_refresh_launchagent` keep
their ids — renaming them would churn the corpus directories and the
guardrail frontmatter for no gain — and their *statements* are now
platform-neutral in the guardrail docs. Their *code* I left alone:
re-review 3 lists both among the 43 slips, and rewriting the audit
layer is `stack/17-audit-rules-redesign`'s job, running in parallel.
Editing the same rules on two branches would be a merge conflict dressed
up as progress.

The new audit, `platform_capabilities_gate_their_backends`, was written
after re-review 3 and to its standard (`review/REVIEW-STACK-3.md` §1:
derived sets, never hand-written file or name lists):

1. **Capability queries are derived** — any definition in
   `crates/{core,cli,tui}/src` whose return type is a capability enum.
2. **Asking is not refusing** — every call that may reach one must be
   honoured (§17 item 2).
3. **The guard comes first, transitively.** The scheduling feature is
   derived from the CLI's own `Command::Schedule` arm (minus what any
   other subcommand reaches); every path from it to a write passes an
   exact, honoured capability check first. Helpers called *before* the
   check are walked. Writes are an inverted set (all of `std::fs` but
   the reads, plus `File::create`, `OpenOptions::new`, `Command::new`).
4. **The platform refusal is decided once**, by the one function that
   reads `ContinuitySource`; the variants it builds are derived, not
   listed.

**The finding worth keeping.** The version the previous worker left
uncommitted keyed the call graph by function *name*, with a comment
that collapsing namesakes over-approximates and so fails closed. It
fails **open** wherever the graph is subtracted: `work_counters::install`
is called by every subcommand, so the name `install` counted as shared,
and `schedule::install` — the function the rule exists for — was never
walked. Five of its nine corpus fixtures (no check, check after the
write, alias, helper, `OpenOptions`) were **accepted**. Keying by
definition (file + name), resolving call paths through the module
layout, and using only *exact* edges on the subtracted side fixed all
five; fixture 10 pins the namesake case directly. Over-approximation is
only fail-closed on the side of a set you add to, never on the side you
subtract.

Two smaller things: the workspace is parsed once per audit run instead
of five times (16 s → 5 s for this audit in a debug build), and the
old text checks for `CAPABILITIES` and `#[cfg(target_os)]` in
`platform/mod.rs` were dropped rather than ported — they were
existence checks on one hand-named file, and both properties already
have stronger evidence (`scripts/platform-isolation.sh` on each built
binary; `platform_matrix_matches_docs` on the table).

Ten rejection fixtures, all rejected, including alias/rename (03) and
discarded-result (04). The capability table is checked against
`docs/platform.md` in **both** directions by `platform_matrix_matches_docs`.

Also fixed on the way: the `free-space` capability note still said
`statvfs` on both platforms after macOS had moved to `statfs`; code and
doc now both say which syscall each uses. And the env-mutating unit
tests in `platform` and `schedule` each had their own mutex, which
serialises each module against itself and not against the other — one
unsets `HOME`, the other reads it. They now share `crate::TEST_ENV_LOCK`.

---

# 7. The pre-existing failure I did not touch

`reviewer_cost_measurement_stack2::two_unchanged_full_observations_cost_report`
fails on the base commit `9653cd7`, on macOS, locally and on the
self-hosted runner, with `dirs_listed=71` where it asserts `0`. It is
documented in section 2 of `.oh/sessions/2026-09-22-event-gated-reuse.md`
("which I may not edit, still fails two assertions, and after this
change it fails them *harder*"), with the same numbers.

I did not fix it, and I did not silence it. It is stack/13's deliberate,
raised consequence and the completion it needs — per-unit-root FSEvents
cursors — is that chunk's recorded follow-up.

It matters for this chunk in one additional way. As written, that test
can never pass on Linux: its reuse gate requires event coverage, and a
Linux build has none. Whoever implements the per-unit-root cursors
should give it a Linux expectation at the same time — under
`ContinuitySource::LiveWatchEpochOnly` the honest expectation is the
named full-walk fallback, not zero listings.

---

**Status at the end of this chunk (2026-09-22).** It is the *only*
failing test on either CI job: run 35766818429 failed on both jobs, and
on each the sole failure was this test (`dirs_listed` 71, expected 0);
every other step — fmt, both isolation checks, clippy, source audit,
the #80 traversal step, `check.sh`'s greps — passed. Locally on macOS
the workspace suite is 1,161 passed, 1 failed, the same one. It also
still fails on `stack/14-detector-root-cursors`, whose note attributes
it to the preserved 3 s `TooSoon` floor and says it was left failing for
re-review 3 on purpose. So CI on this branch cannot be green on either
OS without someone deciding what that auditor-written test should assert
— on macOS, and separately on Linux, where no continuity source exists
at all. That is a decision for the owner, not a fix for this chunk; I
did not edit, gate or `#[ignore]` it.

---

# 8. Follow-ups, recorded not done

- **#81 (Linux watcher).** Use `inotify` directly, not `notify`;
  surface `IN_Q_OVERFLOW` as a named coverage failure. The cursor shape
  is already there: `ContinuityCursor::LiveWatchEpoch`, whose
  `covers_since` is `false` for anything before the watch opened.
- **#82/#83.** Untouched here beyond the contracts they will hang off.
- **#85 (Trash).** Path resolution is done, spec-correct and tested
  (`platform/trash.rs`). What remains is the `.trashinfo` record, the
  cross-device move, and restore. Adopt `trash` 5.2.9; adopting it
  transfers none of swamp's safeguards, which still have to run at the
  sink first.
- **#86 (systemd --user).** `Scheduling::Unavailable` already names it.
  An implementation must refuse explicitly where the user manager is
  unavailable rather than assuming systemd.
- **#88 (Linux release).** CI builds and tests Linux and publishes
  nothing. `release.yml` is untouched.
- **#89.** Untouched.
- **Toolchain pinning.** `-D warnings` against a floating `stable` will
  break again. Either pin CI's toolchain or accept periodic lint
  commits; this is a decision, not a task.
- **`Platform::current()` on a third OS.** `locations::Platform::current`
  answers `Linux` for any non-macOS target, so a hypothetical BSD build
  would get Linux's conventions. `platform::Os::current` refuses to
  compile there instead. The two should agree; making `locations`
  refuse as well is a small change I left alone because it touches a
  type several chunks are using.
- **Btrfs/ZFS overstatement.** On a deduplicating or compressing
  filesystem a sum of `st_blocks` exceeds what removing those files
  returns. macOS has `reclaimability::apfs_clone_or_snapshot_bound` for
  the same class of problem; Linux has no equivalent, and a total there
  is an upper bound that is not labelled as one. Documented in
  `docs/platform.md`, not implemented.
