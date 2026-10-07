# Platforms

Swamp supports two targets:

| | Target triple | Validated on | Status |
|---|---|---|---|
| macOS | `aarch64-apple-darwin` | macOS arm64, in CI on every push | Builds, tests, releases |
| Linux | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86_64, in CI on every push; the archive also smoke-tested on the newest hosted Ubuntu | Builds, tests, releases (glibc; see [Release archives](#release-archives-and-what-they-require)) |

No other target is supported, and none is planned here. Windows is not a target. ARM Linux is not a target. Nothing in this work makes either one closer; adding one means doing this exercise again for that kernel.

**CPU baseline: generic x86_64.** No `target-cpu=native`, ever, in a release profile. A binary built for the machine that built it is not a binary anyone else can run, and a measurement produced by a build nobody can reproduce is not evidence.

**No root, no assumed init system.** Everything swamp does on Linux runs as an ordinary user. It does not require `systemd`, a session bus, a container runtime, or membership in any group. Where a capability genuinely needs one of those, swamp refuses and names what is missing (see [Capabilities](#capabilities)) rather than escalating or guessing.

**glibc, not musl.** `x86_64-unknown-linux-gnu` links glibc. `scripts/platform-isolation.sh linkage` asserts in CI that the binary's runtime dependencies stay within the loader, `libc`, `libm` and `libgcc_s` — so a new dependency that drags in a system library is a CI failure rather than a support question after the fact.

## Capabilities

This table is generated from `crates/core/src/platform/CAPABILITIES`, and `crates/core/tests/platform_matrix_matches_docs.rs` fails if the two disagree. A capability cannot be claimed here without being claimed in the code, or removed from the code without being removed here.

| Capability | macOS | Linux | Notes |
|---|---|---|---|
| `walk-and-accounting` | supported | supported | Allocated bytes from st_blocks*512, hardlink dedup by (dev, ino), same-filesystem boundary by st_dev, symlinks never followed. Shared POSIX code. |
| `free-space` | supported | supported | A syscall through libc, replacing df output parsing whose columns differ between the two: statfs(2) on macOS, whose statvfs has 32-bit block counts, and statvfs(3) on Linux. |
| `history-replay` | supported | unavailable | macOS replays the fseventsd log from a stored event id. Linux has no persisted kernel change history; an observation there walks fully and says so, unless a running collector can vouch for the gap. |
| `live-watch` | supported | supported | Not used by the TUI since v0.7.5. macOS can open an FSEvents stream. Linux registers unprivileged inotify watches per directory and names every loss (queue overflow, watch limit, permissions, unmount); a loss makes the next refresh a full walk. |
| `background-collection` | unavailable | supported | Linux: opt-in swamp collect keeps a bounded change list that later observations reuse only while it runs, in the same boot, with coverage intact. macOS needs none: FSEvents keeps the history. |
| `scheduled-observation` | supported | supported | macOS installs an opt-in per-user LaunchAgent. Linux installs systemd --user units (timer, optional collector); where no user manager is reachable it refuses and writes nothing, and it never enables lingering. |
| `trash` | supported | supported | macOS renames into ~/.Trash. Linux follows the freedesktop Trash spec (home trash, or the mount's own .Trash-$uid) with a .trashinfo record; a rename or a refusal, never a copy or a permanent fallback. |
| `occupancy` | supported | supported | macOS runs a bounded lsof; Linux reads procfs (fds, cwd, exe, maps) of this user's processes. Anything that cannot be read is Unknown -- never 'nothing is open': a Trash move names it as a warning on the confirm, and tool-managed removal (no Trash) names it as a warning and re-checks it at `Y`. |
| `atime-reliability` | supported | supported | macOS reads statfs mount flags; Linux reads /proc/mounts. Either failing is Undetermined, not 'atime is fine'. |
| `release-artifact` | supported | supported | Separate archives with checksums per target (binary, README, skill), each built and tested on its own runner; a release publishes only after both targets and a newer-Ubuntu test pass. |

"Unavailable" and "planned" both mean the same thing at runtime: swamp refuses and says why. The difference is whether an issue exists that would change the answer.

### The one that is not a missing feature

`history-replay` is the only row that says *unavailable* rather than *planned*, and the distinction is the point of this whole design.

macOS's `fseventsd` writes a per-volume change log to disk. A stored event id is a cursor into it, and a replay from that cursor accounts for everything that happened **while swamp was not running**. That lets Swamp reuse unchanged measurements across downtime when the event stream still covers the interval; it is not a fixed latency guarantee.

Linux has no equivalent. inotify reports what happens while a watch is open, keeps no history, and tells you with `IN_Q_OVERFLOW` when it dropped even some of that. An inotify watch descriptor is a handle on a running watch — it is not a cursor, and storing one where an event id belongs would turn "swamp was not watching" into "nothing changed". That is a false measurement, and the growth store's whole value is that it does not contain any.

So a Linux observation walks fully and reports `mode=full reason=no_persisted_change_history` -- unless something was watching the whole time. The live watch ([#81](https://github.com/open-horizon-labs/swamp/issues/81); no longer used by the TUI since v0.7.5) and the opt-in collector ([#82](https://github.com/open-horizon-labs/swamp/issues/82), `swamp collect`) narrow the gap to the time since their watch opened (see [Live watching and continuity](#live-watching-and-continuity-on-linux)). They do not remove it: a period with no watcher is a gap, and no amount of implementation work will change that.

## Reuse assessment

Issue [#79](https://github.com/open-horizon-labs/swamp/issues/79) requires this before any bespoke adapter: for each candidate, what it supports, its licence, its dependency and build-script footprint, its maintenance, how it gates by target, and whether it preserves swamp's accounting, scope, history, error visibility and action protections.

Versions and platform statements below were checked against crates.io, docs.rs and the upstream repositories on **2026-09-22**.

### `trash` 5.2.9 — **adopted as the independent reader; not used for the move ([#85](https://github.com/open-horizon-labs/swamp/issues/85))**

| | |
|---|---|
| Licence | MIT. MSRV 1.85.0. |
| Platforms | Windows, macOS, and freedesktop-compliant environments. Implements v1.0 of the freedesktop Trash specification. |
| Dependencies (Linux) | `log`, `libc`, `once_cell`, `scopeguard`, `urlencoding`, `chrono`. **No D-Bus, no C build script.** |
| In swamp | A **Linux-only dev-dependency**, pinned `=5.2.9`. Ships in nothing. |

Part 1 recommended adopting it for the move. Reading the 5.2.9 source to implement #85 changed that, for four reasons, each a rule this project will not break:

1. **It copies across devices.** `move_items_no_replace` tries `rename`, and on `EXDEV` falls back to `copy_dir_all` followed by `remove_dir_all` of the source. For a build directory that is a whole-tree copy that expands sparse files and splits hardlinks -- it can need more space than the disk has -- and it is not atomic.
2. **It falls back to the home trash across devices.** When a per-volume trash is not writable (`PermissionDenied`), `delete_all_canonicalized` moves the item to the home trash instead, which on another filesystem is the same copy.
3. **It does not say where an item went.** `delete` returns `()`. The ledger records a recovery location for every removal; it could not, without re-listing every trash on the machine and guessing by name and second.
4. **It honours a relative `$XDG_DATA_HOME`**, which the base-directory specification says to ignore.

So the move is swamp's own, in `crates/core/src/fs_gate/destroy.rs` (`trash_move`/`Envelope::open`, proof-and-authorization-gated like the macOS path): the home trash (`$XDG_DATA_HOME/Trash`, or `~/.local/share/Trash`) laid out as `files/<name>` plus a `.trashinfo` record in `info/<name>.trashinfo` (original path, deletion time), written best-effort after a successful move rather than reserved with `O_EXCL` beforehand. **A rename or a refusal** -- no copy, no permanent fallback; the destination is refused outright if a same-named entry already exists (a plain existence check immediately before the rename, not an atomic `renameat2(RENAME_NOREPLACE)`: a narrower guarantee than the original design called for, noted here rather than overclaimed). A trash root on another filesystem than the store's `SWAMP_TRASH_DIR` override is refused (`fs_gate::destroy::Envelope::open`'s `same_device_as` check); the per-mount `$topdir/.Trash-$uid` fallback the freedesktop spec allows for that case is not implemented.

The crate is still used, for what it is good at: `crates/core/tests/linux_trash.rs` lists and restores every item swamp trashes through `trash::os_limited::{list, restore_all}`. That is the evidence that a file manager following the same specification can find swamp's items, read their original path and deletion time, and put them back. Those tests hold a lock around the crate's calls, since it reads `$XDG_DATA_HOME` from the process environment.

### `notify` 8.2.0 — **not adopted; #81 uses inotify directly**

| | |
|---|---|
| Licence | CC0-1.0 (siblings are MIT/Apache-2.0). MSRV 1.77. Latest overall is 9.0.0-rc.5, a prerelease. |
| Backends | inotify on Linux, FSEvents on macOS by default (kqueue behind `macos_kqueue`), ReadDirectoryChangesW on Windows, polling everywhere. |

The documented limitations are the reason for the recommendation, not against the crate. Its README says the Linux backend "is documented to not be a 100% reliable source" under load, and points at `fs.inotify.max_user_watches` / `max_user_instances`. What it does **not** document anywhere is `IN_Q_OVERFLOW` — there is no named API for surfacing a queue overflow to the caller.

That single omission is disqualifying for swamp's purposes, because overflow is the case that matters: it is the moment the kernel says "I lost events", and it is precisely when a watcher must stop claiming coverage and force a full walk. A library that cannot tell swamp overflow happened would let swamp report "nothing changed" about a period where changes were dropped. `PollWatcher` is not an answer either — a full rescan per interval is the cost swamp exists to avoid.

`notify` also cannot replay history, which nothing can on Linux (see above). So the value it would add over `inotify` directly is cross-platform abstraction swamp does not need: macOS already has a hand-written FSEvents backend that does exactly what the replay design requires.

**Decision: `inotify` directly, through `libc` -- no new dependency -- with `IN_Q_OVERFLOW` surfaced as a named coverage loss.** Implemented in [#81](https://github.com/open-horizon-labs/swamp/issues/81) (`crates/core/src/live_watch.rs`).

### `walkdir` 2.5.0 — **already a transitive dependency; kept as the reference walker, not the production one**

| | |
|---|---|
| Licence | Unlicense OR MIT. Last release 2024-03-01; stable and maintained. |
| Dependencies | `same-file` (plus `winapi-util` on Windows only). On Unix that is the whole footprint. |
| Cost to adopt | **Zero new dependencies** — it is already in swamp's normal dependency graph through `gix`. |

It gets the important defaults right: `follow_links` is off, `same_file_system` exists, and errors are yielded as items rather than swallowed. It is single-threaded, and `DirEntry::metadata()` costs an extra `stat` on Unix (`file_type()` is the free one).

It is used in the repository today as the **reference implementation** in `crates/core/tests/walk_library_comparison.rs`, which is where the evaluation lives rather than in prose:

- `a_generic_walker_needs_swamps_semantics_to_reach_swamps_number` sums `metadata.len()` over a fixture with a hardlink pair and a 32 MiB sparse file, the way a generic size pass naturally would, and gets a figure **more than ten times** swamp's. Add allocated bytes, `(dev, ino)` dedup and a same-filesystem boundary on top and the two agree exactly.

That is the finding. `walkdir` is not wrong; it is a walker, and the thing swamp needs is a measurement. Whatever library sat underneath would still have to be wrapped in all three rules, so the reuse saves the traversal loop and nothing else.

Measured, both walkers over the same generated tree, same build profile (debug), warm cache:

| Shape | Files | swamp | walkdir + swamp's rules | Ratio |
|---|---|---|---|---|
| 16 dirs × 2,000 files — macOS arm64, APFS | 32,000 | 37.7 ms | 80.0 ms | 2.1× |
| 512 dirs × 40 files — macOS arm64, APFS | 20,480 | 24.5 ms | 58.7 ms | 2.4× |
| 16 dirs × 2,000 files — Ubuntu 24.04 x86_64, ext4 | 32,000 | 47.9 ms | 92.0 ms | 1.9× |
| 512 dirs × 40 files — Ubuntu 24.04 x86_64, ext4 | 20,480 | 38.4 ms | 64.5 ms | 1.7× |

One CI run each, 2026-09-22, on shared runners: the absolute times are worth little and the ratio is the point. Every run prints its own (`WALK BENCH os=... ratio=...`) from the **Traversal, accounting and volume identity** step, so the current numbers are always in the job log rather than only in this file.

**Decision: keep swamp's walker.** It is 1.7–2.4× faster than the alternative *with the same semantics bolted on*, the bounded parallel pool is what makes the difference on the many-directory shape, and swapping it would trade that for no reduction in the code that actually has to exist.

### `jwalk` 0.9.0 — **rejected**

Archived upstream. The `Byron/jwalk` repository has `archived: true`, and release 0.9.0 (2026-08-05) exists to carry the notice: *"This crate is no longer maintained or supported."* The release before it was 0.8.1, in December 2022.

Technically it is the closest thing to what swamp already has — a rayon-backed parallel walk that preloads metadata, with per-directory parallelism ("It wont help when reading a single directory with many files", which is the shape a `target/` directory often has). Under maintenance it would have been the serious candidate.

**Decision: rejected on maintenance.** Depending on a crate whose author has publicly stopped supporting it, to replace working code, is a trade with nothing on the upside. No benchmark was run against it in-repo, deliberately: the number could not change the decision, and running it would have meant adding an unmaintained crate to the lockfile to produce it.

### `clean-dev-dirs` 2.8.2 — **detector conventions only, not the scanner**

| | |
|---|---|
| Licence | Apache-2.0 OR MIT (per its `Cargo.toml`; the GitHub API reports only Apache-2.0). |
| Library API | Yes — a real `[lib]` exporting `Scanner`, `Cleaner`, `Project`, `ProjectType`, `ScanOptions` and more. Not binary-only. |
| Dependencies | 17 direct, including `clap`, `colored`, `indicatif`, `inquire`, `rayon`, `trash`, `walkdir`, `jwalk` 0.8.1. |

Its CLI is more careful than the issue's phrasing assumed: it prompts before deleting, and it moves to trash by default (`use_trash: !permanent`, with a test pinning that), with `--permanent` as the opt-in. So "never run its destructive CLI as a scan" remains right, but it is not a reckless tool.

Its **scanner** is where reuse fails, in three independent ways. `src/utils/size.rs` sums `metadata.len()` — apparent size, not allocated blocks, so every sparse file and every filesystem with compression reports wrong. There is no inode tracking, so a hardlinked file counts once per name. And `same_file_system` is not set, so a size can walk across a mount boundary. Each of those is a property swamp's accounting is built on, and none is configurable from the outside.

Adopting it would also mean pulling a CLI's presentation stack — `clap`, `colored`, `indicatif`, `inquire` — into a library dependency, plus `jwalk`, which is archived.

**Decision: do not reuse the scanner.** What is worth reusing is its **ecosystem detection conventions** — which directory names belong to which toolchain — and swamp's `ecosystem.rs` already cites this project for exactly that. Keeping one catalog with attribution, rather than a second conflicting one, is the reuse that applies here. Its licence notice belongs with that citation.

### Summary

| Candidate | Decision | Because |
|---|---|---|
| `trash` 5.2.9 | Adopt as the test-only reader (#85); do not adopt for the move | Its `delete` copies and deletes across devices and does not return where an item went; its listing and restore are the independent check that swamp's records are readable. |
| `notify` 8.2.0 | Do not adopt; `inotify` directly (#81) | No documented way to surface `IN_Q_OVERFLOW`, which is the one event a coverage claim depends on. |
| `walkdir` 2.5.0 | Keep as the test reference | Correct but generic; swamp's walker is 1.7–2.4× faster once the same semantics are applied to both. |
| `jwalk` 0.9.0 | Reject | Upstream archived and explicitly unmaintained. |
| `clean-dev-dirs` 2.8.2 | Reuse its conventions, not its scanner | Apparent size, no hardlink dedup, no filesystem boundary; brings a CLI's dependency stack. |

## Where each platform's code lives

Shared portable code stays shared. Target gating is for genuinely different kernels, not for filing code by operating system.

| Concern | Shared | macOS-only | Linux-only |
|---|---|---|---|
| Contracts | `platform/mod.rs` (`Os`, `ContinuitySource`, `ContinuityCursor`, `Scheduling`, `CAPABILITIES`) | | |
| Traversal and accounting | `walk.rs`, `attribution.rs` — POSIX `st_blocks`, `st_dev`, `st_ino` | | |
| Free space | `platform/fs_space.rs` contract | `statfs` (64-bit block counts) | `statvfs` (64-bit block counts) |
| Change observation | `fs_events.rs` types and refusals; `live_watch.rs` state machine | `fs_events::macos` (CoreServices) | `live_watch::inotify`; `continuity.rs` collector |
| Scheduling | `schedule.rs` interval parsing, status; unit rendering in `systemd_user.rs` | `launchctl`, plist | `systemctl --user` (`systemd_user::RealSystemctl`) |
| Trash | `fs_gate::destroy` (`trash_move`, `Envelope`) | rename into `~/.Trash` | freedesktop `files/`+`info/` layout, `.trashinfo`, plain rename (no `renameat2`) |
| Occupancy | `occupancy.rs` (tri-state, one scan for all anchors) | bounded `lsof` | procfs |
| atime reliability | `activity.rs` contract | `statfs` mount flags | `/proc/mounts` |
| Default scan roots | `locations/builtin.rs` | `~/src`, `~/Library/Developer`, `~/Library/Caches`, `$XDG_CACHE_HOME` | `~/src`, `$XDG_CACHE_HOME` |
| Data and log directories | `platform::data_dir`, `schedule::log_dir` | `~/.local/share/swamp`, `~/Library/Logs/swamp` | `$XDG_DATA_HOME/swamp`, `$XDG_STATE_HOME/swamp` |

Cargo enforces the dependency half: Apple framework crates (`core-foundation`, `core-foundation-sys`, `fsevent-sys`) sit under `[target.'cfg(target_os = "macos")'.dependencies]` in `crates/core/Cargo.toml`, and `scripts/platform-isolation.sh deps` asserts in CI that they are present in the macOS graph and absent from the Linux one. `scripts/platform-isolation.sh linkage` asserts the same thing one level lower, against the built binary's linkage and symbol table.

## Paths

`SWAMP_DIR`, `SWAMP_LOG_DIR` and `SWAMP_TRASH_DIR` override everything below, on both platforms.

| | macOS | Linux |
|---|---|---|
| Growth store | `~/.local/share/swamp` | `$XDG_DATA_HOME/swamp`, default `~/.local/share/swamp` |
| Scheduled-run log | `~/Library/Logs/swamp` | `$XDG_STATE_HOME/swamp`, default `~/.local/state/swamp` |
| Trash | `~/.Trash` | `$XDG_DATA_HOME/Trash` (`files/` + `info/`), or the mount's `.Trash/$uid` / `.Trash-$uid` |
| Scheduling | `~/Library/LaunchAgents` | `$XDG_CONFIG_HOME/systemd/user` (default `~/.config/systemd/user`); `SWAMP_SYSTEMD_UNIT_DIR` overrides |
| Collector checkpoint | not applicable | `<growth store>/continuity/<root id>.parquet` (+ `<root id>_entries.parquet`, `.lock`, `.dirty.lock`, `.sync`) |

The macOS growth store path is deliberately unchanged, including its XDG-shaped spelling: existing installs keep their history where it is, and this work moves no user data.

Two rules the XDG base directory specification states, followed here:

- **A relative value is invalid and ignored.** `XDG_CACHE_HOME=cache` does not produce a scan root relative to whatever directory swamp was run from; it falls back to `~/.cache`.
- **State is not data.** Logs go under `$XDG_STATE_HOME`, which is what that category is for; the growth store is data.

And one rule swamp adds: **no current-directory fallback.** If `HOME` is unset and `SWAMP_DIR` is not given, resolution fails with a message rather than writing a growth store into the working directory — where the next run from a different directory would not find it and every project would look like it had vanished.

### Why Linux's default roots are what they are

`~/src` on both: a habit, not an OS convention.

`~/Library/Developer` and `~/Library/Caches` are macOS's. `~/Library/Developer` is Xcode's, and no Linux directory holds "the SDK and simulator storage of the platform toolchain"; proposing `/usr/lib` or a distribution's package cache would mean walking system-owned storage a user cannot act on without root, which swamp never asks for. `~/Library/Caches` has an equivalent in `$XDG_CACHE_HOME` (default `~/.cache`). Swamp includes the XDG cache root on both platforms: cross-platform developer tools also use it on macOS, alongside native caches.

Tests keep macOS Library paths out of Linux defaults and verify that macOS retains native caches alongside the XDG cache root.

Homebrew is detected on both, with the prefixes each platform actually uses: `/opt/homebrew` and `/usr/local` on macOS, `/home/linuxbrew/.linuxbrew` on Linux. `/usr/local` is not proposed on Linux — there it is a distribution-owned directory Homebrew does not claim.

### Detectors that do not apply here

A detector that does not apply to the running platform (Xcode's, CoreSimulator's and the other macOS-only ones on Linux) is **reported, not omitted**. Silence would be indistinguishable from a detector that ran and found nothing, or one that failed. `swamp scope` lists them under their own heading, *not applicable on this platform*, with the platforms each one does apply to, and `swamp scope --json` carries the same list as `not_applicable_detectors`. Every registered detector is in exactly one of the applicable and not-applicable lists, and a macOS scope must list no macOS detector as inapplicable (`scope::tests`, both directions).

## Live watching and continuity on Linux

**The watcher (#81).** `crates/core/src/live_watch.rs` holds one claim and names every way it stops being true:

> Since `opened_at`, every change under the root is in the dirty set.

inotify is not recursive, so the watcher adds one watch per directory -- on the root's filesystem only, never through a symlink, never under swamp's own store or a scope exclusion -- and **registers each directory before listing it**, so a subdirectory created during the listing is reported by its parent. Everything reported during registration is kept as dirty, and `opened_at` is the second registration *finished*, rounded up. A change during bootstrap is therefore either in the dirty set or before `opened_at`; there is no third place for it.

A directory created or moved in is registered with its whole subtree, all of it dirty. A rename pair inside the root keeps its watches (inotify watches inodes) under the new path; a directory moved out of the root has its watches dropped. The dirty set is exactly the directory an entry changed in (and the entry itself when it is a directory) -- not its parent as well, which would turn a write inside a build directory into a re-walk of the worktree around it.

The claim ends, with a named reason, on: `IN_Q_OVERFLOW` (`watch_queue_overflow`), running out of watches (`watch_limit_reached`, naming `fs.inotify.max_user_watches`), a directory that cannot be watched or listed (`watch_permission_gap`), `IN_UNMOUNT` (`watched_filesystem_unmounted`), a watch the kernel removed unasked (`watch_removed`), the root itself moved (`root_mismatch`), and more than 512 dirty directories (`too_many_changes`). A lost claim is **never** an empty change set: the next observation walks the root fully and names the loss. An overflow, an unmount or a removed watch are recoverable -- a fresh epoch opens and the observation after the full walk can be incremental again. A watch limit or a permission gap is not: `swamp collect` stops vouching for that root and says why.

**In the TUI** (before v0.7.5) each live batch fed the same incremental pipeline FSEvents batches do. Since v0.7.5 `swamp ui` opens no filesystem watch and never scans on file events; the pipeline above serves `swamp observe`, `swamp collect` and the scheduled run.

**The collector (#82).** Between processes -- a scheduled `observe`, a one-shot `report` -- nothing survives on Linux unless something kept watching. `swamp collect` is that something: opt-in, user-started (directly, or as a user service through `swamp schedule --collector`), foreground, stopped by SIGINT/SIGTERM. It keeps a bounded checkpoint per root under the growth store (`continuity/<root id>.json`): root identity (canonical path, `st_dev`, root inode), the boot id, the epoch, the coverage, the dirty directories with sequence numbers, and its exclusions. An observation reuses the list -- walking only those directories -- when **all** of these hold, and otherwise walks fully naming the first that fails:

| Condition | Refusal when it does not hold |
|---|---|
| a collector for this root is running (it holds an `flock` for its whole life, which the kernel releases however it dies) | `no_persisted_change_history` (never one) / `collector_stopped` |
| same boot, same root identity | `collector_stopped` / `root_mismatch` |
| its exclusions hide nothing this observation walks | `scope_changed` |
| it confirms it has read every queued event (a sync request written into its control directory, on the *same* inotify instance as the root, so it is queued behind every earlier change) within 2 s | `collector_unresponsive` |
| coverage held since its epoch opened | the loss |
| the stored observation is inside the epoch | `live_watch_gap`, or the loss that ended the previous epoch |

The list is **consumed only after the observation's history and replay cursor are written**, under a lock the collector's own read-modify-write also takes, and only up to the sequence number the plan covered: an entry dirtied again during the walk survives. A crash before the history write, or between it and the consumption, leaves the entries, and the next observation re-walks them -- redundant, never wrong. An observation lock per root, held from reading the previous state to committing the next, keeps a TUI refresh, a CLI run and a scheduled run from interleaving (so one cannot overwrite the other's rows with an older walk).

A fresh store's first observation records the classification rules and its second anchors the cursor, as on macOS; from the third on, a collector that has been running can vouch.

## Scheduling on Linux

`swamp schedule --every 1h [--collector] [roots]` writes swamp-owned units into `~/.config/systemd/user/` (every one begins with a marker line; a unit of the same name without it is never replaced or removed) and enables them through `systemctl --user`:

- `swamp-observe.timer` → `swamp-observe.service`: a oneshot `swamp observe` with the binary's absolute path, systemd-quoted arguments (`%` and `$` doubled, control characters refused), `Nice=10`, idle I/O, three restarts an hour at most, output in the user journal (`journalctl --user -u swamp-observe.service`) as well as swamp's own `observe.log`. systemd never runs two instances of one oneshot at once, and swamp's observation lock serializes it against a manual run.
- with `--collector`, `swamp-collect.service`: the collector as a user service, restarted on failure (five times an hour at most).

The timer alone keeps nothing between runs: **without the collector every scheduled run walks fully**, and `swamp schedule` (status) says so. With it, a run walks only what changed while the collector was running.

Nothing runs as root, and **lingering is never enabled**: without it a user manager runs only while you have a session, so both units stop at logout and start again at the next login. Status reports `Linger=yes/no` and what `loginctl enable-linger` would change -- your decision. Where no user manager is reachable (no `$XDG_RUNTIME_DIR`, `systemctl --user` does not answer: a container, WSL without systemd, `su`, cron) `install` refuses before writing anything and says to use cron or your own timer instead. A start that fails is rolled back. `swamp schedule --off` stops, disables and removes swamp's units only. launchd is compiled into the macOS build only and systemd into the Linux build only.

## Trash on Linux

Every supported recoverable TUI action -- a confirmed path, Cargo group, or agent-storage unit -- moves through one backend, `fs_gate::destroy` (the [reuse assessment](#reuse-assessment) says why it is swamp's own and not `trash::delete`). On Linux the item goes to the freedesktop Trash a file manager shows: `$XDG_DATA_HOME/Trash/files/<name>` with `info/<name>.trashinfo` holding its original path and deletion time, or, on another mount, that mount's `.Trash/$uid` or `.Trash-$uid`. It is a rename or a refusal -- never a copy, never a permanent deletion -- and the ledger records both the location and the `.trashinfo`. Cargo groups and agent sessions go in as one *envelope* per group, with swamp's `restore.json` inside naming each member's original path; the envelope's `.trashinfo` points beside the members' original directory, so a file manager's Restore puts the envelope back there and `restore.json` says where each member goes. `SWAMP_TRASH_DIR` is a plain directory on both platforms, not the system Trash, and nothing claims otherwise. Moving to Trash frees no space until the Trash is emptied.

## Occupancy on Linux

Linux has no `lsof` requirement: `fs_gate::procfs::probe` reads `/proc` directly, unprivileged, once for every anchor of an action. For each process it may inspect it compares `cwd`, `root`, `exe`, every fd and every mapped file with the selection (a deleted-but-open file still counts).

**Which processes it may inspect is the kernel's rule, not a list.** `ptrace_may_access` lets an unprivileged process read another's fds only when every uid and gid match, the target holds no capability the reader lacks, and the target is dumpable. Processes outside that -- another user's, one with more privilege (a setuid program, a capability), one the kernel marks non-dumpable (its procfs entries turn root-owned) -- are outside the question, as they are for `lsof` without root; the evidence's coverage note says so. Measured on the Ubuntu 24.04 runner, this is what makes cleanup possible at all: the user manager `systemd --user` holds `CAP_WAKE_ALARM`, and its `(sd-pam)` PAM holder is non-dumpable; neither is readable by the user they run as.

Everything else it cannot answer is **`Unknown`, shown as such on the confirm and never read as free**: a process with exactly this user's credentials whose entries are still this user's and yet cannot be read (an LSM denial), a procfs that belongs to another PID namespace (`/proc/self` is not this process), an unreadable `/proc`, and the 10 s bound. A process that exits mid-scan is skipped. A nested PID namespace that mounts its own `/proc` cannot be told apart from the host from inside it; that limit is recorded, not claimed as covered.

**This scope is a decision, not a fact of the code**, and is listed for the owner in the session note: the literal reading -- every unreadable same-user process is `Unknown` -- would leave every Linux action carrying an Unknown open-file warning on any machine with a systemd user session, which is every standard Ubuntu login.

## Release archives and what they require

`release.yml` builds each shipping binary once on its target runner. Publication requires full checks on both macOS and Ubuntu 24.04, smoke tests of both packaged binaries, and a smoke test of the same Linux archive on the newest hosted Ubuntu (#88).

The full checks run the workspace tests, source audits, compile-fail cases, mutation sweep, and cost test. They use the normal test profile; we do not rebuild the whole test suite with shipping LTO settings. Optimized-code coverage comes from exercising the packaged binary, not from recompiling every test harness. This does not provide full unit-test coverage under release optimization. The newer-Ubuntu job validates archive compatibility without installing Rust or rebuilding the workspace.

| Asset | Contents |
|---|---|
| `swamp-<version>-x86_64-unknown-linux-gnu.tar.gz` (+ `.sha256`) | `swamp`, `README.md`, `skills/swamp/` |
| `swamp-x86_64-unknown-linux-gnu.tar.gz` (+ `.sha256`) | the same, under the name `releases/latest/download/` resolves |
| `swamp-<version>-aarch64-apple-darwin.tar.gz`, `swamp-aarch64-apple-darwin.tar.gz` (+ `.sha256`) | the macOS equivalents |

No MCP server artifact exists for either target (#104). The same `scripts/package-release.sh` and `scripts/release-smoke.sh` run in `ci.yml` on every push, so the archive CI smoke-tests is the archive a release publishes. The smoke test verifies the checksum, extracts, and runs the *extracted* binary: `--version`, `--help`, the packaged skill, an explicit-root `report --json`, `observe` twice into a scratch store and a `report --json` of that history, and TUI startup and quit under a pseudo-terminal.

Measured on the Ubuntu 24.04 runner (CI run 35789903497, 2026-09-22):

| | |
|---|---|
| Runtime libraries (`ldd`) | `linux-vdso.so.1`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`, `/lib64/ld-linux-x86-64.so.2` -- nothing else, asserted by `scripts/platform-isolation.sh linkage` |
| Newest glibc symbol version required (`objdump -T`) | **`GLIBC_2.39`** -- so the archive needs glibc 2.39 or newer: Ubuntu 24.04 and later. Older distributions (Ubuntu 22.04's glibc 2.35, Debian 12's 2.36) will not load it; build from source there |
| CPU | generic x86-64 (no `target-cpu=native`, `RUSTFLAGS` empty in both workflows) |
| Kernel features used | inotify, plain `rename(2)` (check-then-rename, not `renameat2(RENAME_NOREPLACE)`), procfs; tested on Linux 6.17 (Azure) |
| Not required | root, `lsof`, D-Bus, a desktop session; `systemd --user` only for `swamp schedule` |

The minimum glibc is a property of the build image, not a promise: it is re-measured by every CI run's smoke step, and would move if the baseline image moved. No musl or static build, no ARM Linux archive, and no Windows artifact exist or are implied.

## Linux validation

`scripts/linux-validation.sh` runs in CI (job *Linux x86_64 release archive, smoke and validation*) against the **extracted release archive**, a generated workload and scratch state, and writes `linux-validation.txt` to the job's artifact. From run 35789903497:

| | |
|---|---|
| Machine | GitHub `ubuntu-24.04` runner: Ubuntu 24.04.5 LTS, Linux 6.17.0-1022-azure, AMD EPYC 7763 × 4, 16 GB, ext4; `max_user_watches` 655360, `max_queued_events` 16384 |
| Workload | 21 checkouts (20 Cargo projects, 1 node), ~1,480 directories, ~8,500 files, 36.8 MB allocated |
| Install to first report | 468 ms (checksum, extract, `report --json`) |
| Initial full scan | 311 / 279 / 281 ms (3 runs) |
| Unchanged, collector running | 196 / 198 / 196 ms, `mode=incremental changed_dirs=0` (3 runs) |
| One-subtree mutation | 225 / 224 / 225 ms, `mode=incremental changed_dirs=2-3` (3 runs); `walked_total` equal to a reference full walk into a fresh store |
| Collector killed (SIGKILL), change made while down | `mode=full reason=collector_stopped` |
| New collector epoch | `mode=full reason=live_watch_gap` |
| Real inotify queue overflow (collector SIGSTOPped through 10k creates) | `mode=full reason=watch_queue_overflow`, then `mode=incremental` on the next run |
| Watch memory | 1,481 watches, ~1.5 MiB kernel estimate (1,080 B/watch), collector RSS 8.3 MB |
| On-disk state | growth store 110 KB; collector checkpoint 623 B |
| Reviewed cleanup (historical CLI, now removed; current cleanup is TUI-only) | `propose -> approve -> execute` moved `node_modules` to `$XDG_DATA_HOME/Trash/files/node_modules` with a `.trashinfo` |
| systemd user lifecycle | the runner exposes a user manager (lingering on): `schedule --every 1h --collector` installed both units, the collector ran, one scheduled observation ran (`Result=success`, incremental), `--off` removed every swamp unit |

Wall times are one shared runner's, and are recorded rather than asserted. The **regression thresholds are asserted on work, not time**, in `crates/core/tests/live_watch_cost_bounds.rs`, on both platforms: an unchanged observation under a live epoch lists **0** directories (bound: ≤ 4, and under 2% of the full walk); a change inside a build output lists that artifact's own directories (measured: 1, of 714 for the full walk); a change in one worktree's source lists that worktree, not its 300-directory sibling (measured: 19; bound: under a quarter of the full walk). Linux is not expected to match macOS latency -- it has no persisted history to replay -- and nothing here claims it does; what is claimed is that, with a collector running, repeat observation is proportional to what changed.

The same script and the smoke test run on the newest hosted Ubuntu against the 24.04-built archive (job *Linux x86_64 archive on the newest hosted Ubuntu*).

**The user's own Linux machine is unverified.** Everything above is GitHub's runner. To validate a real host: check out the repository, download the release archive and its `.sha256` into one directory, and run `REPS=3 scripts/linux-validation.sh <archive> <version> <out-dir>`. It uses scratch `HOME`/`SWAMP_DIR`/Trash only; its systemd step installs swamp's units into your real user unit directory and removes them again, so skip it (unset `XDG_RUNTIME_DIR`) if you would rather it did not.

## How the platform invariants are enforced

Two kinds of check, because each misses what the other catches.

**Runtime tests on both runners** hold the behaviour: `install_without_a_user_manager_refuses_and_writes_nothing` asserts the unit, agents and log directories are *empty* after a Linux refusal with no user manager, and `systemd_user::tests` drive the whole systemd lifecycle through an injected backend; `a_kernel_without_persisted_history_says_so_rather_than_unsupported` pins the Linux refusal; `neither_platforms_conventions_leak_into_the_other` pins the defaults; `platform_matrix_matches_docs` pins the table above.

**A source audit**, `platform_capabilities_gate_their_backends`, holds the shape future code must keep, and is written in the derived-set form the audit re-review asked for rather than as file or function-name lists:

- capability queries are *derived*: any definition in the workspace whose return type is a capability enum;
- every answer to one must reach control flow (`let _ = scheduling();` fails);
- the scheduling feature is derived from the CLI's own `Schedule` dispatch arm, and every path from it to a filesystem write or process spawn — through helpers, renamed entry points and aliased imports — passes an honoured capability check first. Writes are an inverted set: every `std::fs` call except a short list of reads;
- which refusal a platform without replay gives is decided in exactly one function, the one that reads `ContinuitySource`.

Call edges resolve to *definitions*, not names. The first version keyed them by name and so let `work_counters::install` (called by every subcommand) hide `schedule::install` from the rule entirely; ten fixtures in `crates/source-audit/tests/mutations/platform_capabilities_gate_their_backends/` now pin that and the other bypasses. What the audit cannot see — trait-object dispatch, function pointers, a writing helper genuinely shared with another subcommand — is listed in `.oh/guardrails/platform-capabilities-are-refused-not-approximated.md`, and is what the runtime tests are for.

## The volume ledger off macOS

`swamp observe --volume` needs `statfs`, a directory listing and `lstat`, all
on both platforms. What is macOS only is asked, not assumed: `diskutil` and
`tmutil` are spawned only on macOS, so on Linux there are no system-volume,
purgeable or snapshot rows and no note about their absence (there is nothing to
ask). The container is the filesystem under `/`; other filesystems are found in
`/proc/self/mounts`, and one on its own storage is a mounted view, a network
share has no size, and pseudo filesystems (`proc`, `sysfs`, `tmpfs`, ...) are not
listed. Linux folders one level under `/home`, `/var`, `/usr` and `/opt` are
listed the way `~/`, `/Library` and `/private` are on macOS.

## Volume identity, and its limit

The growth store is keyed by `root_scoped_volume_id`: a hash of `(st_dev, canonical path)`. The canonical path means two spellings of one root — including a symlinked one — share a history instead of silently starting a second, empty one. The device means two roots that happen to share a path string on different filesystems do not.

**`st_dev` is not stable across reboots for every Linux mount.** Device numbers are assigned as the kernel enumerates devices; a mount whose minor number changes gets a different id, and swamp starts a fresh history for that root rather than continuing the old one.

That is the safe direction, and it is a choice rather than an oversight. The alternative — keying on the canonical path alone — would join two genuinely different filesystems mounted at the same path into one history, and report the difference between them as growth. A lost baseline is visible to the user ("no history yet"); fabricated growth is not. `volume_identity_depends_on_the_device_and_a_changed_device_starts_a_new_history` pins the behaviour so it cannot be "fixed" without someone deciding to.

## Filesystems

Measured behaviour is asserted on whatever the CI runner mounts — APFS on macOS, ext4 on the Ubuntu runner — by `crates/core/tests/traversal_accounting_and_volume_identity.rs`, which checks allocated bytes, hardlink dedup, sparse files and symlink loops against the filesystem's own `stat` data and against `du`.

Known limits, not tested because no CI runner provides them:

- **Overlay filesystems** (Docker's container layers, `overlayfs` generally) report the *upper* layer's allocation for a file the container modified and the lower layer's for one it did not. A total is therefore about the merged view, not about reclaimable space in either layer.
- **Delayed allocation** (ext4, XFS, btrfs and overlayfs over them): a file with dirty data reports a token `st_blocks` until writeback (measured on a Docker overlayfs: 512 bytes for 5 KB and 20 KB files). Recent files in that state are counted as reported and flagged pending (a fresh sparse file looks identical, so no length-based figure is substituted); `scripts/linux-validation.sh` and the traversal tests call `sync` before every measurement so incremental and full walks are compared on settled data.
- **Network filesystems** (NFS, SMB, sshfs) may report `st_blocks` that does not correspond to local allocation at all, and `statvfs` figures that describe the server. `cargo_cleanup.rs` already refuses to act on a filesystem it does not recognise as local; measurement still reports what the filesystem says, labelled as such.
- **Btrfs, ZFS, XFS, bcachefs and overlayfs** can share extents (reflinks, snapshots, a lower layer) or compress below the file, so a sum of `st_blocks` can exceed the space that removing those files would return. On those filesystems (read from `statfs`'s `f_type`) each row's reclaimable figure is reported as an **upper bound** naming the filesystem and the mechanism -- the same class of overstatement APFS clones cause on macOS, handled the same way (`reclaimability::shared_extent_bound`). ext4 and tmpfs share nothing and keep the exact figure. Totals remain allocated bytes, as everywhere.

## Running the platform checks yourself

```bash
# Both halves of "only this platform's backends are in this build"
./scripts/platform-isolation.sh deps x86_64-unknown-linux-gnu
cargo build -p swamp && ./scripts/platform-isolation.sh linkage target/debug/swamp

# Accounting and volume identity against the filesystem and du
cargo test -p swamp-core --test traversal_accounting_and_volume_identity -- --nocapture

# The walker benchmark (prints; asserts only that both walks agree)
cargo test -p swamp-core --test walk_library_comparison -- --ignored --nocapture
```

Cross-*checking* for Linux from a Mac needs a C cross-compiler for `zstd-sys`. `cargo check --target x86_64-unknown-linux-gnu` works with `zig cc` standing in:

```bash
printf '#!/bin/sh\nexec zig cc -target x86_64-linux-gnu "$@"\n'   # minus --target=, which zig rejects
CC_x86_64_unknown_linux_gnu=/path/to/that/wrapper \
  cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
```

That is a compile check and nothing more. It does not link, it does not run, and it is not acceptance for anything — which is why CI runs natively.
