# Usage

For the product overview, start with the [README](../README.md). For which
platform can do what, and where swamp keeps its files on each, see the
[platform guide](platform.md). The one real difference between the two:
macOS keeps a change history swamp replays, so an observation there is
able to reuse unchanged measurements when event coverage is valid; Linux keeps none, so an observation
walks fully unless a live watch (the opt-in `swamp collect`)
has been running since the last one -- and swamp says which it did.

## Installing a release

On Apple silicon macOS, install with [Homebrew](https://brew.sh). The formula installs the `swamp` binary, the `skills/swamp/` agent skill, and verifies the release archive's checksum:

```bash
brew install open-horizon-labs/tap/swamp
swamp --version
```

Update with `brew upgrade swamp`; uninstall with `brew uninstall swamp`.
The tap checks for new releases every 15 minutes; GitHub may delay scheduled updates.

If you installed manually before, run `type -a swamp`. A copy in
`~/.local/bin` may take precedence over Homebrew. Remove that manually installed
copy after verifying `"$(brew --prefix)/bin/swamp" --version`.

Release archives remain available on the [releases page](https://github.com/open-horizon-labs/swamp/releases).
You can also [build from source](../README.md#build-from-source).

### Linux x86_64

There is no package-manager formula for Linux. Download the archive and its
checksum, verify, and install the binary (and, if you use an agent, the
skill):

```bash
curl -fLO https://github.com/open-horizon-labs/swamp/releases/latest/download/swamp-x86_64-unknown-linux-gnu.tar.gz
curl -fLO https://github.com/open-horizon-labs/swamp/releases/latest/download/swamp-x86_64-unknown-linux-gnu.tar.gz.sha256
sha256sum -c swamp-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf swamp-x86_64-unknown-linux-gnu.tar.gz
mkdir -p ~/.local/bin
install -m 755 swamp-x86_64-unknown-linux-gnu/swamp ~/.local/bin/
swamp --version
```

The binary links glibc (not musl, not static) and is built on Ubuntu 24.04
for a generic x86-64 CPU; the [platform guide](platform.md#release-archives-and-what-they-require)
lists the measured minimum glibc and the Ubuntu releases it has been run on.
It needs no root, no daemon and no `lsof`. Update by repeating the download;
there is no migration step. Versioned archives (`swamp-<version>-x86_64-unknown-linux-gnu.tar.gz`)
are attached to each release as well.

## Observations and history

### Unique bytes and fast refresh

Ordinary refreshes update changed containers without revisiting unchanged roots
to resolve hardlinks. Their unique-byte estimates can therefore be stale. The
TUI labels these totals as needing reconciliation; JSON exposes the same fact
as `reconciliation.unique_estimate.needs_reconciliation`.

Run `swamp observe --full` for the configured scope (or pass the same explicit
roots you normally observe), then `swamp report --view reconciliation`.
When using explicit roots, pass the same roots to both commands.
The full observation includes an additional parallel measurement across the
observed project roots, external units, and agent units. It counts a shared
device/inode once, respects exclusions, and does not follow symlinks. This
extra traversal is explicit, never part of an ordinary incremental refresh.

The resulting byte total and reconciliation timestamp are stored in the
existing Parquet run row, alongside bounded container-sharing summaries in the
store. No inode inventory is retained. Later observations
keep that last result with `needs_reconciliation: true`; an incomplete full
observation cannot certify it as current. Before the first reconciliation the
JSON value is `null`, not zero. The estimate covers the observed paths: known
missing roots remain listed in scope coverage and contribute no paths. That
does not delete their stored history. Inaccessible or partially read roots
prevent a fresh reconciliation.

This scope-wide physical-byte estimate excludes Docker and does not change
per-root charges, artifact growth history, or cleanup advice. Do not add it to
the row totals. Row totals use local accounting and may include shared storage;
neither figure promises how much deletion will free.
Uniqueness here means hardlink deduplication by inode, not detection of shared
APFS clone/snapshot extents.

`swamp observe` refreshes report data: it walks the filesystem,
groups projects, computes signals, discovers external/agent-tool
storage, and persists all of it. `swamp report` is a pure read of what
the last `observe` wrote. It checks candidate-root presence and the
comparison namespace, but does not recursively walk roots, inspect
artifacts, or spawn a subprocess. Run `observe` first; `report` on a scope that has
never been observed prints `no observation yet for <scope>; run swamp
observe` (JSON: `{"error":"no_observation", ...}`) and exits 2.

`inspect-cargo` separately performs bounded, read-only inspection of an
existing Cargo profile. It does not run Cargo or persist an observation.

What `report` reads is Parquet, and only facts. Rows that an
observation produced: the project, worktree and artifact rows
(`projects.parquet`, `worktrees.parquet` + `worktree_facts.parquet`,
`artifact_shape.parquet` + `artifact_shape_lists.parquet`, and the
per-volume current-artifact table); external and agent-tool storage
units (`external_units.parquet`/`agent_units.parquet` +
`unit_consumers.parquet`/`agent_unit_members.parquet`, and the sibling
tables `unit_meta.parquet` for an external unit's last-used and overlap and
`unit_children.parquet` for its depth-2 rows); a volume's
unowned rows (`unowned.parquet` + lists/evidence, and
`docker_unowned.parquet` for the Docker objects no project claims);
nested build-artifact units (`nested_artifacts.parquet` + lists/
evidence); every row's decision evidence (`evidence.parquet`); per-root
coverage with each walked root's own totals (`coverage.parquet`); the
run's notes (`notes.parquet`); the run itself (`runs.parquet`: when,
with which growth window, whether the drill-down was asked for, the
live GitHub-enrichment counters, the scheduler status the pass saw);
and the tracking state of a walk's top-level directories
(`<volume>/dir_tracks.parquet`). Everything else in a report is
computed from those when it is read -- the by-type summary, the
reconciliation total, the byte-history series, the per-worktree
directory/large-file drill-down and each artifact's growth all come
from the fact tables and the reverse-delta history at the observation's
own timestamp, so a read is byte-identical to the pass that wrote the
facts and there is no second copy of anything to disagree
(`.oh/guardrails/store-is-facts-report-is-views.md`). There is no JSON
render-cache row anywhere in the store. Every table is rewritten by
`observe`; delete one and the next `observe` recreates it. There is no
migration for a store written before a table existed -- `report`
simply reads what the last `observe` wrote.

```bash
swamp observe ~/src
swamp observe ~/src --since 24h --full
swamp report ~/src
swamp report ~/src --sort size --reverse
swamp schedule --every 15m ~/src   # LaunchAgent on macOS, systemd --user timer on Linux
swamp schedule --every 1h --collector ~/src   # Linux: also keep a live change list
swamp schedule
swamp schedule --off
swamp collect ~/src                # Linux: watch in the foreground until Ctrl-C
swamp collect --status --json ~/src
```

`observe` records data without rendering and enriches worktrees from GitHub (`--no-enrich` skips it; `--enrich` forces a refetch of every worktree now, ignoring the cache rules). Scheduling runs that observation command; it does not delete anything.

`swamp schedule` installs a per-user LaunchAgent on macOS. On Linux it installs `systemd --user` units instead: a timer that runs `swamp observe` on the interval and, with `--collector`, the collector as a user service. Neither needs root; lingering is never enabled for you, so without `loginctl enable-linger` both stop at logout and resume at the next login, and `swamp schedule` (status) says which. Where no systemd user manager is reachable -- a container, WSL without systemd, a shell outside a login session -- the command refuses, says so, and writes nothing; schedule `swamp observe` from cron instead. `--off` stops and removes only the units swamp wrote. `--collector` is refused on macOS, which does not need one.

**Linux: the timer alone walks fully every time.** inotify keeps no history between processes, so each scheduled run reports `mode=full reason=no_persisted_change_history`. `swamp collect` -- run it yourself, or install it with `--collector` -- keeps a bounded change list while it runs, and an `observe`/`report` then walks only what changed. It stops being trusted, and the next run walks fully naming why, when it is not running (`collector_stopped`), after a reboot, when its watch lost events (`watch_queue_overflow`), hit the watch limit (`watch_limit_reached`) or could not read a directory (`watch_permission_gap`), when its exclusions differ from the observation's (`scope_changed`), or when the last observation predates it (`live_watch_gap`). `swamp collect --status [--json]` shows each root's epoch, coverage, dirty directories, inotify watches and their approximate kernel memory. See [Live watching and continuity](platform.md#live-watching-and-continuity-on-linux).

History starts when swamp observes a root. `observe --since` selects the comparison window that pass's growth/regrowth figures use, defaulting to the `since` config value; `report` has no `--since` of its own -- it reads whatever window the last `observe` used. Use seconds, minutes, hours, or days here: `30m`, `24h`, `7d`. The filter language also accepts weeks, but the CLI/config history-duration parser does not; use `7d` rather than `1w` for `since`.

Use `observe --full` to force a full filesystem walk. A normal observation can also fall back to a full walk when event history is insufficient; the report notes explain why.

Observations are stored separately for each canonical scan root. You can switch between a project and its parent directory using the same `SWAMP_DIR`; each root keeps its own history and incremental checkpoint. Overlapping roots are separate views, not totals to add together.

For an ad-hoc multi-root scope, pass the same explicit roots to `report`
as to `observe`:

```bash
swamp observe /path/to/main /path/to/checkout --since 24h
swamp report /path/to/main /path/to/checkout --view projects --json --limit 10
```

Omitting roots selects the configured scope, not the last ad-hoc scope.

## Scope and coverage

`report` and `observe` accept explicit roots. With roots omitted, scope-aware
commands resolve the same **effective scope**, computed by one shared
function so no command can silently disagree with another:

```bash
swamp scope
swamp scope --json
```

The effective scope is built from four sources, in this order, and
every root falls into exactly one of two classes: a **project root**
(ordinary Git/ecosystem discovery, project grouping, and an
unowned-remainder walk) or a **detector location** (measured only as an
external unit -- a folded row plus its adapter's own interior
identification -- and never walked for projects, even when a Git
checkout happens to live inside it). `swamp scope`'s text output shows
the two classes under separate headings; `swamp report`'s per-root
coverage says `detector location (measured as an external unit, not
scanned for projects)` for the second class.

1. **Built-in default roots**, per platform. Only `~/src` is a project
   root. macOS also proposes `~/Library/Developer` and
   `~/Library/Caches`; Linux also proposes `$XDG_CACHE_HOME` (default
   `~/.cache`) -- both are detector locations, not project roots: a
   project checked out inside either one is still discovered (as its
   own external unit's interior, if its adapter identifies one) but
   never becomes a "project" the way something under `~/src` would.
   Neither platform's conventions appear in the other's build; see
   [Platforms](platform.md#why-linuxs-default-roots-are-what-they-are)
   for why Linux has two rather than three.
2. **Detector results.** A built-in catalog of read-only detectors
   proposes locations for developer tools: language version managers
   (mise, asdf, pyenv, uv, Conda, rbenv, RVM, ruby-install, nvm,
   rustup), shared dependency/build caches (Cargo home, npm, pnpm,
   Gradle, Maven, Go, pip), Apple/Android developer tooling (Xcode,
   CoreSimulator, Android SDK), and package/model/VM stores (Homebrew,
   Hugging Face, Ollama, Docker Desktop's host backing file, OrbStack).
   Every one of these is a detector location, never a project root. A
   conventional path is still proposed even when the tool's
   executable is absent, so a leftover cache can still be found. See
   [docs/locations.md](locations.md) for the full table -- every
   detector, its locations, overrides, categories, and documented
   limits (e.g. pnpm's per-volume stores are not enumerated, Maven's
   downloaded-vs-locally-installed split needs per-artifact evidence
   this catalog does not inspect). `swamp scope --json` always lists
   the exact detector IDs and catalog version in use.

   A detector-resolved location that falls *inside* another kept
   project root (a declared root, `~/src`, or another detector's own
   base directory) is pruned from that root's walk and measured
   exactly once, as its own external unit -- see
   [coverage-and-history.md](../skills/swamp/references/coverage-and-history.md)'s
   "External/shared storage units".
3. **Declared roots: `[scan] include`** in `config.toml`: the directories
   you say hold your source code, always in scope. swamp never works
   these out: it does not read your shell history, your editor's recent
   projects, `~/.gitconfig` or Spotlight to guess them. Declare them with:

   ```sh
   swamp config add-root ~/code             # a directory that holds projects
   swamp config add-root /Volumes/data/src --allow-missing   # not mounted yet
   swamp config remove-root ~/code
   swamp config show                        # lists them: present / missing / unreadable, with bytes
   ```

   `add-root` edits `config.toml` in place: your comments, key order and
   other settings stay, the write is atomic (a temp file renamed over the
   old one), and two `add-root` calls at once both land. It refuses a path
   that does not exist (unless `--allow-missing`, which records a root that
   is not mounted yet; it is then reported `missing`, a coverage fact and
   never an error or a deletion), a file, a directory you cannot read, `/`,
   and a directory inside a root you already declared (it names which). A
   root you already declared is a no-op however it is spelled (trailing
   slash, `..`, a symlink, `~`, a relative path). Declaring a directory
   that contains declared roots replaces them and says so. What is stored
   is what you typed (with `~` kept), not the resolved path.

   When your roots change (`add-root`, `remove-root`, or an upgrade that adds a
   detector), `swamp report` and `swamp ui` keep showing the most recent
   observation whose roots overlap, labelled `showing the previous scope`, until
   `R` or the next `swamp observe` builds the new one: nothing is walked to show
   it and no growth is computed across the two scopes. The TUI header carries a
   `N declared roots` clause and the help screen lists them, and
   `swamp report --json` has a `declared_roots` array.

   `swamp scope`, `swamp config show` and `swamp report` print each
   declared root as `present` (with the bytes from the last stored
   observation, or `not measured yet`), `missing`, or `unreadable`, and
   never walk anything to do it.

   **First run.** When nothing has been observed yet and `config.toml` has
   no `[scan]` section, an interactive `swamp ui` or `swamp observe` asks
   `Where is your source code? Press Enter for ~/src` (offering `~/src`
   only if it exists; otherwise just asking for a directory; `~/src` stays a
   built-in default either way), records the answer and carries on. It looks at nothing else on disk to propose a
   candidate. Without a terminal it never waits: it prints
   `swamp config add-root <path>` as the way to declare a root and uses the
   built-in defaults. A skipped answer is remembered, so it is asked once.
4. **`exclude`** and **`disabled_detectors`**: pruned last, and always
   win over every other source -- including an explicit root you pass
   on the command line. Disabling a detector does not hide a path
   reachable through another still-enabled root (e.g. disabling the
   Cargo detector does not hide `~/.cargo` if it happens to live inside
   `~/src`, which is still in scope via the built-in defaults).

`[scan] defaults = false` means **explicit-only scope**: swamp infers
nothing. In scope are your `include` entries, any explicit command
root, and detectors your config actually names -- either through the
`enabled_detectors` allow-list, or (equivalently) as the complement of a
non-empty `disabled_detectors` deny-list. With `defaults = false` and
neither list set, **nothing** is in scope and every command says so.

```toml
[scan]
defaults = false                     # infer nothing
include = ["~/code"]                 # ...except this
enabled_detectors = ["huggingface"]  # ...and this one detector
```

An earlier release read `defaults = false` as "drop the three built-in
default roots, keep inferring from every other detector", which put
paths in scope that nothing in the config had asked for. That reading is
gone.

The two lists are read differently, and it is worth being precise about
which, because "explicit" can mean either:

- **`enabled_detectors` is an allow-list.** When it is non-empty, only
  the detectors it names run. Adding a detector to the catalog later
  changes nothing for you.
- **`disabled_detectors` is a deny-list**, even under
  `defaults = false`: the catalog *minus* the ones you named. So
  `defaults = false` with only `disabled_detectors = ["homebrew"]` runs
  every other detector -- naming what you do not want is itself an
  explicit statement about the rest.

Both readings are pinned by name:
`scope.rs::tests::defaults_false_without_includes_or_enabled_detectors_is_empty`
(neither list set means empty),
`defaults_false_with_only_disabled_detectors_still_runs_the_rest` (the
deny-list reading), and the reviewer's own
`defaults_false_must_mean_explicit_only`. If you want the strict
reading, set `enabled_detectors` rather than relying on
`disabled_detectors`.

**Homebrew is reported in two parts by default.** `/opt/homebrew` (or
`/usr/local`) is a **system-wide install tree**, shared by every account
on the machine, and its Cellar and Caskroom hold GUI applications and
system tools with nothing to do with any project. So swamp counts by
default only what is unambiguously developer tooling: an allowlist of
language toolchains and build tools (`llvm`, `openjdk`, `dotnet`, `zig`,
`go`, `rust`, `node`, `python`, `ruby`, `cmake`, `gradle`, `maven`,
`ninja`, `mise`, `terraform`, ...; `llvm@20` and `llvm@21` are both
`llvm`), the developer casks (`android-platform-tools`,
`android-studio`), and Homebrew's Android command-line tools. Each of
those is a unit of its own. Everything else under the prefix is not
dropped: it is one measured line, `Homebrew (other)`, and the two add up
to the prefix. Ambiguous tools (qemu, ansible, pandoc, duckdb, mlx, GUI
casks) are in `Homebrew (other)` until someone argues them in with
evidence; a `Brewfile` naming one would be that evidence, but reading
`Brewfile`s is not implemented yet. The Android bytes Homebrew installs
are counted once, by the `android` detector's directories, not again in
the Homebrew unit. The allowlist is in
[docs/locations.md](locations.md#homebrew-dev-tooling-allowlist-174).

swamp only reports these. If you decide to remove one, Homebrew
re-obtains it with `brew reinstall <formula>`, a network download.

To see Cellar and Caskroom whole instead of the split (the full
detector, which `swamp scope` reports `disabled (default off)`):

```toml
[scan]
enabled_detectors = ["homebrew"]  # everything else keeps its own default
```

`disabled_detectors = ["homebrew"]` turns off **all** Homebrew reporting, as it did
before the split (the id names the family of three); the member ids
`homebrew-devtools` and `homebrew-other` work on their own.

To drop the remainder line, or Homebrew entirely:

```toml
[scan]
disabled_detectors = ["homebrew-other"]                      # keep the dev tooling only
# disabled_detectors = ["homebrew-devtools", "homebrew-other"]   # no Homebrew at all
```

The same `enabled_detectors` key that means "only these" under
`defaults = false` means "also this one, which defaults off" under
`defaults = true` -- one key, read according to which scan mode it
appears under, rather than a second key for the opposite direction.
Every other catalog detector -- language version managers, shared
build/dependency caches, agent-tool homes -- lives under `$HOME` and
keeps its ordinary opt-out default (`disabled_detectors` turns it off).

Passing an explicit root (`swamp report ~/other-tree`) replaces sources
1-3 entirely for that invocation -- `exclude` still applies. A root that
sits inside another in-scope root is folded into its parent for
measurement (not walked twice); `swamp scope --json` still lists it,
marked `skipped-as-nested`, so you can see exactly why it did not get
its own line. The default `swamp scope` text view hides these
folded-in rows as noise; `swamp scope --verbose` shows every root,
including them.

An effective scope that resolves to nothing at all -- `defaults =
false` with no `include` and no enabled detectors -- is a visible error,
never a silent fallback to the current directory or your home
directory. Malformed `[scan]` config (e.g. `defaults = "yes"` instead
of a bool) is also a visible, nonzero-exit error rather than a silently
broadened scope.

### Keeping things: `swamp protect`

`swamp protect add <path>` records that you want a path kept. It blocks
in **both** directions:

- anything at or beneath a protected path is refused when you try to mark it in the TUI; and
- anything that *contains* a protected path is refused too. Protecting `~/.claude/debug/log.txt` therefore also stops
  `~/.claude/debug/` being marked, because removing the parent would
  destroy exactly what you asked to keep.

The TUI reloads the protect list from disk fresh every time you mark a row, so a protection added mid-session is honoured immediately. If the protect list cannot be read or parsed,
protection state is **unknown**: marking is not blocked, and the confirm says
`could not read your protect list ... your keep marks were not checked`
until the file is repaired or removed -- `swamp protect list`
reports the same error rather than printing an empty list. The file is
written atomically (temp file plus rename), so an interrupted write
cannot turn your keep list into an empty one.

Coverage is not storage: adding a root, excluding one, or a detector
newly resolving a path is a change in what swamp *looks at*, not a
change in what exists on disk. `report`/`observe` persist the resolved
scope (`scope_roots.parquet` and its sibling scope tables under `SWAMP_DIR`) and print a one-line note on
stderr when it changes since the last observation, e.g. `coverage
changed since last observation: +root /Users/you/.cargo (detector
cargo-home), -root /Users/you/old-project (excluded)`. This note never
implies bytes were added or removed -- see
[coverage and history](../skills/swamp/references/coverage-and-history.md).

With no explicit root, `report`, `observe`, and `ui` all now observe
the **whole** effective scope coherently in one call
(`swamp_core::report::report_scope`), not just its first present root:
every present root is walked, every root's growth store is written
(each root still keeps its own physical current+reverse-delta store,
keyed by device and canonical path -- this is one coherent
orchestration, never a merged store, so one root's observation can
never overwrite or corrupt another's), and the resulting `report --json`
carries a per-root `scope_coverage` array whenever any root is not
fully, cleanly observed. Passing an explicit root still replaces the
scope entirely and stays on the single-root path, same as always. A
scheduled run installed with no explicit roots (`swamp schedule --every
30m`, no trailing paths) re-resolves the configured scope on every
fire, so editing `config.toml` takes effect on the next run rather than
only after re-running `schedule --every`.

`swamp ui` with no explicit root opens over this same whole scope, not
just its first present root: project, shared/external, and agent-tool
storage from every present root are all visible together, including a
root that has no Git checkout in it at all. The header shows a short
coverage clause whenever any root's own walk this pass was not cleanly
`complete` (e.g. `2 roots (1 missing)`, `2 roots (1 partial: 2 path(s)
unreadable during this walk)`). The UI opens on the last stored report at
once and scans only when there is no stored report yet; otherwise the
schedule keeps the index current and `R` refreshes on demand (see the
v0.7.5 changelog). Passing an explicit root
(`swamp ui ~/other-tree`) keeps the single-root path unchanged.

### Observation regions

Every root `report`/`observe`/`ui` considers gets one of five outcomes
for that pass, visible in `scope_coverage`/`swamp scope`:

- **complete** -- walked in full; its growth store is authoritative.
- **partial** -- walked, but part of it (a subdirectory, a worktree)
  could not be read this pass; that part's rows are left untouched
  rather than tombstoned.
- **excluded** -- pruned by a config `exclude` entry, either the whole
  root or a subtree inside an otherwise-included root. Not observed,
  never treated as deleted.
- **missing** -- the root does not currently exist. Its previously
  observed rows (if any) keep their history untouched.
- **inaccessible** -- the root exists but could not be read at all
  (e.g. `chmod 000`, or access lost between scope resolution and the
  walk). No walk is attempted; the store is completely untouched and no
  FSEvents cursor advances for it.

A root that only *lost read access* (still exists, cannot be listed) is
distinguished from one that is genuinely gone: losing and regaining
access to an unchanged tree never fabricates a deletion or a later
regrowth. Removing a root from scope, or adding an exclusion, is
likewise a coverage change, never a storage change -- see
[coverage and history](../skills/swamp/references/coverage-and-history.md).

The TUI header shows a short coverage clause when its own root is not
simply present (e.g. `2 roots (1 missing)`), dropped first among
trailing clauses on a narrow terminal like every other low-priority
header fact -- never silently hidden. This reflects the *resolved*
scope for the one root `swamp ui` was given (`scope::RootStatus`, no
walk required), not the richer per-root walk outcome above
(`coverage::RegionStatus`) that `report --json`'s `scope_coverage`
shows; making the TUI's own report itself multi-root is tracked
separately (#50).

## External and shared storage

### Which projects share these bytes?

`swamp observe --full` records hardlink-sharing groups between folded containers.
Select an artifact or worktree in the TUI to see its sharing evidence, or use
`swamp report --view reconciliation` (`--json` for the stored groups).
For example, a package-manager store and two projects can share one 400 MB
group: that is 400 MB counted once, not 400 MB for each pair.

These facts carry their reconciliation time. Normal refresh retains them with
“needs reconciliation”; it does not rescan other projects to rediscover peers.
Links outside scanned coverage remain unresolved, and very large summaries
explicitly report omissions. Absence of a listed peer is not proof of exclusivity.

Sharing is not a deletion prohibition or an ownership claim. Cleanup estimates
are calculated for the selection: deleting one link may free no space, while
deleting all its links can free the inode's storage. APFS clones and snapshots
can still affect actual savings. Sharing groups do not change growth history.

Storage with no containing project -- the Cargo registry, rustup
toolchains, a Homebrew prefix, and future detector-resolved locations
(model stores, package caches, ...) -- is measured as a first-class
**external unit**, independent of any project or worktree:

```bash
swamp report --view external
swamp report --view external --json
```

A unit that is a machine-wide **build store** -- a Maven repository, a
Gradle home's `caches`/`wrapper/dists`/`daemon`/`native`, npm and pnpm
stores, Go's module, download and build caches, pip and uv caches (and
uv's interpreters and tools), DerivedData, Xcode archives and device
support, CoreSimulator devices/runtimes/caches, Android SDK packages and
AVDs -- also prints its identified interior underneath, in the same
family rows `--view builds` uses ("inside (identification only -- no
cleanup is offered here)"); `--json` carries it as `interiors`, keyed by
the unit's path. The join is by declared capability, so a custom
`GOMODCACHE`, `GRADLE_USER_HOME` or DerivedData location is identified
like the default. Interior units keep their own size/growth/regrowth
history; an unchanged store is replayed with no listing and no read. See
[docs/build-artifacts.md](build-artifacts.md#machine-wide-stores-in-the-live-report).

Each unit's identity is `(detector, category, canonical path)`; size,
growth and regrowth history live in the same current+reverse-delta
growth store as everything else (a new key family, not a second
store), so the same retention/coverage guarantees apply. Categories
(`installation`, `downloads`, `cache`, `local-state`, `environments`,
`build-output`, `models`, `unclassified`) come from the detector
registry that also resolves scope (#44) -- an entire manager home is
never collapsed into "cache", and a unit is measured whether or not any
project currently references it (removing the last consumer never
deletes the unit or its history).

No command removes an external unit: registry/detector output
identifies shared storage (a package manager's cache, a toolchain
install), it never deletes it. In the TUI each unit and each listed
folder is a real path and can be marked for the reviewed Trash move
(see "Cleanup and recovery"); removing one means the manager that owns
it fetches or rebuilds it again the next time it is needed, and the
confirm says so. The
`--view external` total is deliberately kept separate from
`reconciliation` above it: external units are never folded into
`walked_total`/`attributed`/`unowned`, so there is nothing to
double-count, but the two bases (a walked root vs. a detector-resolved
location) are different enough that summing them would be misleading.

Sizes are allocated bytes, exactly as the filesystem reports them. On
filesystems that assign blocks late (ext4, XFS, overlayfs over them) a file
written in the last two minutes may report almost nothing until it is
flushed; such files are counted as reported and listed as
`pending_allocation_files` in the work counters, with
`pending_allocation_bytes` as the most they can still add, so a rising total
right after a big write is explained rather than surprising. A file that
cannot be read mid-walk is not measured; the rest of its directory still is.

The TUI has a dedicated External view (Tools section: `2`, then `v`): the same
one-row-per-unit facts as `--view external`; each unit and each listed
folder can be marked for the reviewed Trash move.

### Last run or opened

Every external unit shows one fact about use, with where it came from:

```text
Last run or opened: Jul 8 (file access time)
Last run or opened: Sep 6 (Xcode DerivedData record)
Last run or opened: no record
```

In JSON it is `last_used`: `at` (epoch seconds, `null` for no record),
`source` (`tool_native:<name>`, `file_atime` or `none`) and, beside a
tool-native value, `atime` -- the key files' access time, kept so the two
can be compared. The label is a fact about one file or one record. It is
never "unused" and never "since": a unit whose row says Jul 8 was last
opened that day as far as the record shows, which is not a statement that
nothing needs it. Dates are UTC.

**Precedence, highest first.**

1. A **tool-native record**: the tool's own database or metadata, written
   for this purpose. A backup, an antivirus scan or an indexer cannot move
   it.
2. The **file access time of the unit's key files** -- the regular files
   directly inside a `bin` directory of the unit. Never a directory's own
   access time (a listing moves it), never a symlink's, never a file swamp
   opened.
3. **No record.** Never a date derived from a modification time.

When both exist and disagree the tool-native value is shown and the access
time stays in the JSON. A unit that declares no source shows no record.

| Unit kind | Source | Read from | Checked on a real machine |
|---|---|---|---|
| rustup toolchains | file access time | `toolchains/<toolchain>/bin/*` | yes: a toolchain's `rustc` access time moved when it ran, its `bin` directory's did not |
| mise installs | file access time | `installs/<tool>/<version>/bin/*` | yes |
| pyenv versions | file access time | `versions/<version>/bin/*` | yes |
| Homebrew Cellar | file access time | `Cellar/<formula>/<version>/bin/*` | layout yes; the detector is off by default, so the default report does not exercise it |
| Android SDK packages | file access time | `<package>/<version>/bin/*` (`cmdline-tools`, `cmake`); a package with no `bin` (system images, platforms, emulator, platform-tools, build-tools) shows no record | yes |
| ESP-IDF tools | file access time | `tools/<tool>/<version>/<tool>/bin/*` | yes |
| Cargo registry cache and sources, git databases and checkouts | tool-native: `~/.cargo/.global-cache`, shown as "last used by cargo" | the newest `timestamp` in the table for that subtree (`registry_crate`, `registry_src`, `git_db`, `git_checkout`), opened with SQLite's immutable read-only mode: no file created beside cargo's database (no `-shm`/`-wal`), no lock taken; a read torn by a checkpoint is "unavailable", and a change still in a WAL is not seen | yes: the database and its values were read on this machine (SQLite `user_version` 7) |
| Xcode DerivedData | tool-native: each project folder's `info.plist` `LastAccessedDate` | the existing bounded `plutil` read; a project shows its own, the unit the newest | yes |
| CoreSimulator devices | not implemented | `device.plist` has no last-booted key on this machine (keys seen: `deviceType`, `isDeleted`, `isEphemeral`, `name`, `runtime`, `runtimePolicy`, `state`, `UDID`; no device was booted) | unverified |
| Docker and OrbStack | build-cache entries keep the daemon's own `last_used` (an existing fact); images and volumes report none | the daemon | existing |
| Hugging Face hub repos | file access time | the largest weight blob of the revision shown (the `model-stores` adapter; neither huggingface_hub nor transformers records use), with swamp's own header read set aside | yes: Sep 29 for the one model on this machine |
| Ollama models | file access time | the model layer blob (`application/vnd.ollama.image.model`); Ollama records no use | yes: Aug 30 for `qwen3:0.6b` |
| npm `_cacache`, Gradle | not implemented | | unverified |
| everything else | no record | | |

Access time is a weak signal and the docs say so where it is used:

- **Backup tools, antivirus and indexers touch it.** A unit can show a
  recent date because something scanned it: a later access date may be an
  indexer or a backup reading the file, not you. It is therefore an upper
  bound on how recently the unit was used, never proof of use.
- **`--version` counts.** So does any read; a shell completion that runs a
  tool counts as a run.
- **Mounts.** On a `noatime` mount nothing updates it; on `relatime` (Linux
  default) it updates when the previous value is older than the file's
  modification or a day. APFS behaves the same way for this purpose.
- **Aliases are not double counted.** Symlinks (mise's `latest`, Homebrew's
  `bin` links) are skipped, so an alias never lends its target's time to
  another unit.
- **A date in the future is not shown.** A tool-native record dated after now
  (a tracker written in milliseconds reads as the year 58,000, a copied plist
  as 2099) is set aside as `no record (ignored: date in the future)`. A scan
  that hits its listing limit says `no record (probe limit reached)`, not a
  bare `no record`.
- **It is read on every observation, not replayed.** Reading a file raises
  no filesystem event, so an unchanged unit that is replayed without a walk
  would report an access time that is only as new as its last walk. Swamp
  lists the declared `bin` directories and takes one `lstat` per key file
  each observation instead: bounded (a unit that would need more than 4,096
  listings shows no record), counted in the observation's work counters,
  and it opens nothing (justification: `docs/architecture.md`). `swamp report` never does this: it reads the stored
  value.

### What is inside a big root

A large `unclassified` root (`~/Library/Caches` was one 36.8 GB row) and
every unit that declares a last-use source list their immediate child
folders, largest first, with size, modification time and last-used:

```text
36.9GB  unclassified  /Users/me/Library/Caches
    inside, largest first (rows add up to the total the walk measured):
           19.5GB  hiphi-endpoints  modified 1d ago  Last run or opened: no record
           ...
          281.7MB  remainder: 145 other entries (the other folders, and files directly inside); 9 folders not measured
           -3.7MB  adjustment: hardlinked files are counted once in this unit's total
```

- **Bounded.** The top 15 rows (`DRILLDOWN_TOP_N`) and one remainder row; an
  unclassified root is listed only at or above 1 GiB
  (`DRILLDOWN_MIN_BYTES`). Both are constants in `drilldown.rs`.
- **Exact.** The rows sum to the total the same walk measured, in `--json`
  too (`children`). The remainder is the walk's total minus the listed
  rows, so it holds the other folders and the files directly inside. A
  hardlinked file the walk counted once but two parent folders counted
  under each appears as the signed `adjustment` row, never as missing bytes.
- **The hard-link flag can lag.** A unit's `hardlinked` (in `--json`) is
  what its last walk of each subfolder saw. A subfolder an observe replays
  (no change under it) keeps that answer: a hard link made from outside the
  unit to a file inside it fires no change there, so it is seen only when
  that subfolder next changes. The bytes are exact either way; whether
  another link shares them is checked when the folder changes.
- **Not measured is not zero.** A folder the process could not list is
  shown as `not measured` (`bytes: null`); one with an unreadable folder
  below it is `partial` and its size is a lower bound. Unlisted unreadable
  folders are counted in the remainder row.
- **Names, sizes and dates only.** Folder names are shown as they are on
  disk; no file content and no `Info.plist` is read for them.
- **From the one walk.** The rows come from the per-directory rows the
  folded walk already produces, are stored in `unit_children.parquet`, and
  are replayed with the unit when it is unchanged. In the TUI the External
  view's row opens (`Enter`) onto them; the selected row's detail line
  carries the last-used fact.

### Model caches

The Hugging Face hub cache (`HF_HUB_CACHE`, else `$HF_HOME/hub`, else
`~/.cache/huggingface/hub`) and the Ollama store (`OLLAMA_MODELS`, else
`~/.ollama/models`) are listed one model at a time, in Reclaim (`models` under
the store's row, and each repo folder's row) and in External (each repo folder's
row; the Ollama tags under "identified interior"). Each says:

```text
   968.9MB  mkrausio/EmoWhisper-AnS-Small-v0.1@e613edc6  (model)  whisper · 241.7M params · float32
      last read: Sep 29 (file access time of model.safetensors)
      regeneration: downloaded again from huggingface.co (mkrausio/EmoWhisper-AnS-Small-v0.1@e613edc6) when needed; size 968.9MB
      main -> e613edc6; 1 revision(s): e613edc6; 10 file(s) in the shown revision; 10 blob(s)
      moving this folder frees about 1.9MB; 967.0MB stays in the hub's shared blobs/
   522.7MB  qwen3:0.6b  (ollama model)  qwen3 · 751.63M params · Q4_K_M
      regeneration: downloaded again with `ollama pull qwen3:0.6b` when needed, if the registry has it (a model made with `ollama create` exists only here); size 522.7MB
      moving this manifest frees none of its layers: the layers (522.7MB) stay in blobs/; `ollama rm qwen3:0.6b` removes the model and the layers no other model uses
```

- **What it is** comes only from files already on disk: the model card's YAML
  front matter (`pipeline_tag`, `library_name`, `license`, `base_model`, `tags`,
  `language`), `config.json` (`model_type`, `architectures`, `torch_dtype`), the
  `*.safetensors` header (an 8-byte length and a JSON table of dtypes and shapes:
  the parameter count is exact, and no tensor is read; it counts one copy of the
  weights: the set `model.safetensors.index.json` names, else one shard set
  `<name>-0000i-of-0000N`, else one file, with a note when the snapshot holds
  other copies such as Mistral's `consolidated.safetensors` or diffusers' fp16
  files; a set with shards missing or more than 16 files gives no count, with the
  reason), a `.gguf` header
  (`general.architecture`, `general.name`, quantization; the count when every
  tensor entry fits in the read), and Ollama's config blob (family, Ollama's own
  parameter-size label, quantization). A field none of them states is absent. The
  card's first paragraph is in the detail pane, with control, bidirectional and
  zero-width characters removed and at most 400 characters.
- **Never read through a link.** Every folder of the layout (`blobs/`, `refs/`,
  `snapshots/`, the hub's `blobs/<xx>/`, Ollama's `blobs/` and `manifests/`) is
  `lstat`ed first; one that is a symlink is a fact on the row ("is a link, not
  followed") and nothing in it is listed, read or counted. Files are opened with
  `O_NOFOLLOW` (and `O_NOATIME` on Linux).
- **Swamp's own read is not a use.** The last-read date comes from a weight file
  (the largest blob that is not the README, `config.json` or the shard index,
  which the card pass opens). On macOS (APFS) a read sets a file's access time
  only when it is not newer than the file's mtime, so swamp does not read a
  weight file in that state: the row says "parameter count not read yet: no
  program has opened this file since it was written", and a later observe reads
  it once a program has. On Linux reads use `O_NOATIME`. Where a read does move
  the time anyway (a network volume), the time from before swamp's read is kept
  in the card cache and shown; after that cache is deleted, the first pass cannot
  tell an earlier swamp read from a use.
- **Size.** Snapshots are links into `blobs/`; each blob is counted once, and a
  blob two revisions share once. A blob two repos (or two Ollama tags) share is
  counted under the first by path, and the other says so. The models plus the
  store's own `blobs/` row add up to the store. Incomplete downloads
  (`*.incomplete`, Ollama `-partial`), links that point at nothing, links that
  leave the cache (never followed, not counted), a ref with no snapshot, and blobs
  "not referenced by any manifest" are facts on the row.
- **Regeneration.** A repo with a ref or a revision hash says it is downloaded
  again from huggingface.co at that revision; one with neither (made locally)
  keeps "cannot be regenerated (no source recorded)". An Ollama tag names its
  `ollama pull`.
- **Trash.** A repo folder or a manifest can be marked like any row (in the TUI,
  an Ollama tag is its own row under its store in Reclaim and External). The
  confirm and the detail pane say what stays: "moving this folder frees about X;
  Y stays in the hub's shared blobs/", and for a manifest that its layers stay in
  `blobs/` (`ollama rm` is the tool's own removal). A row belongs to its own
  store only: `~/.cache/huggingface` above the hub cache lists no models.
- **Cost.** Every content read goes through a cache in the store
  (`associations/model_cards.parquet`), keyed by what cannot change under the key:
  a revision's files (name, blob and size), a manifest's size and modification
  time, a config blob's digest. A revision is parsed once; a later observe over
  unchanged files reads no file content (only listings and `lstat`). At most 64
  new parses happen per observe; the rest say "not yet read" until the next one. A
  weight file costs 64 KiB of header (1 MiB at most when its header is larger).
  On this machine the cold pass read 67,043 bytes and took 4.6 ms for the hub
  cache and 1,348 bytes in 0.7 ms for Ollama; the warm pass read 0 bytes (0.9 ms
  and 0.3 ms, debug build). The TUI reads only what observe stored.
- **Hub facts (off by default).** With `swamp config set hf-enrich on`
  (`hf_enrich = true` in `config.toml`), a scheduled or CLI `swamp observe` asks
  `https://huggingface.co/api/models/<id>` (or `datasets/`, `spaces/`) once per
  repo with `/usr/bin/curl` (allow-listed, 10 s timeout, 1 MiB at most, no header
  and no token sent: `HF_TOKEN` is never passed, because a token on curl's
  command line is visible to every process; a gated or private repo shows as
  "did not answer"; behind a proxy, curl gets `HTTPS_PROXY`, `ALL_PROXY`,
  `NO_PROXY` (either case) and `SSL_CERT_FILE`/`CURL_CA_BUNDLE`, nothing else), and the row says `from huggingface.co, fetched <date>`:
  downloads, likes, last modified, whether the revision here is the Hub's current
  one, and, for that revision only, the pipeline tag, library, license and base
  model where the local files did not say. Facts tied to a revision are never
  fetched again; the counts are fetched again after 7 days; a failure (404, gated,
  offline) is kept for a day. At most 16 requests per observe. The TUI's own
  refresh never asks the network; `report` never does.

### The Reclaim view

```bash
swamp report --view reclaim
swamp report --view reclaim --json
```

One row per unit of developer storage (every external unit, and each
standalone Cargo target directory), largest allocated size first, ties by
path. Each row says, as facts with their sources:

- **Size and growth**, and, for a unit that is drilled into, its folders
  (bounded top rows plus one remainder, adding up to the unit's size; a folder
  that could not be read is `not measured`, never `0B`).
- **Regeneration cost.** In order of precedence: the consequence text a build
  adapter stated for the unit's own interior ("reinstall is a download --
  iOS_23F77 is downloaded again when a simulator needs it"); the detector's
  recovery hint with its command (`brew reinstall <formula>`); the category
  default below. `cannot be regenerated` is said for local state and models.
- **Last used**, with its source (`Sep 6 (Xcode DerivedData record)`, `Jul 8
  (file access time)`) or `no record`, exactly as in the section above.
- **Consumers**, in two tiers: declared (a project's declaration, a lockfile, a
  manager's global default) and recorded links (what the tool itself recorded,
  such as Xcode's `WorkspacePath` or a standalone Cargo target's dep-info).
- **What a package manager reports**, quoted verbatim and attributed:
  `Homebrew reports unneeded (brew autoremove): "Would autoremove 4 unneeded
  formulae:"`, `mise reports prunable (mise prune --tools --dry-run): "mise
  poetry@2.1.3 is prunable: ..."`. Swamp never says a unit is unused, obsolete or
  safe; these are the manager's sentences.
- **The removal path that exists.** Every unit and every listed folder: Trash
  after review (Space, then Backspace in the TUI; see "Cleanup and recovery").
  A unit whose manager swamp runs removal for (mise installs, simulator
  runtimes, including `/Library/Developer/CoreSimulator/Volumes`) also names
  that manager's own command, which is permanent with no Trash (Backspace with
  nothing marked); the text never says the command is "not available yet". JSON: `removal.kind` is `trash_reviewed` or `trash_or_tool_command`.

| Storage category | Class | Words when nothing more specific exists | Source |
|---|---|---|---|
| installation | download | reinstall is a download | each manager documents a reinstall command; the detector's hint has the exact one |
| downloads, cache | download | downloaded or derived again by the tool on next use | npm, pip, uv, Cargo and Gradle document their caches as refilled on use |
| build-output | rebuild | rebuilt by the tool's build command | the tool's own build command |
| environments | not established | recreating restores what the manifest names, not data added later | an emulator's apps and data are not in a manifest |
| local-state, models | not regenerable | cannot be regenerated | state a tool wrote for the user; a model's source may be gone. A Hugging Face or Ollama store says more per model (see "Model caches") |
| unclassified | not established | no detector says what is inside | none |
| standalone-cargo-target | rebuild | rebuild with `cargo build` | Cargo's own consequence, stated on the row |

When a build adapter's own text is used, it decides the class: text that names a
download or reinstall is `download`, one that names a rebuild is `rebuild`, one that
says the bytes are gone or unique is `not-regenerable`, one that says removal breaks
something, or says nothing about cost, is `not-established` and stays out of the
regenerable total.

**Scope statement.** Every listing prints `consumer evidence checked against N
projects in M declared roots`; with no declared root it says `consumer evidence covers
the built-in default roots only` and marks the evidence incomplete. If a declared root is missing, unreadable, only
partly read or excluded, it says `incomplete` and names the root; an explicit
root named on the command line says the declared roots were not used; with no
declared roots it says only the built-in roots were checked. A row with no
declared consumer says `none found among N projects in M declared roots`, never
that nothing needs it: a tool used only by a project outside those roots looks
exactly the same.

**Defaults and requested installs are held out.** A rustup default toolchain, a
tool in mise's global configuration and a Homebrew formula installed on request
are marked `active default` / `installed on request` and excluded from the
regenerable total. A unit that cannot separate them (Homebrew's remainder unit)
is held whole. If the manager's own record could not be read, the row says
`unknown` and is held out too; before the first manager pass every such row is
`unknown`.

**The manager pass** runs in `swamp observe` (the scheduled run included) after
the observation, and nowhere else: `report` and the TUI read what it stored and
start no process. It asks only `brew autoremove --dry-run`, `brew list --formula
--installed-on-request`, `mise prune --dry-run` and `mise ls --global --json`,
and reads rustup's `settings.toml`. Each command is an allow-listed shape in the
spawn layer, counted, killed after 20 seconds (the pass after 45). The program is
found at a fixed absolute path (`/opt/homebrew/bin`, `/usr/local/bin`, and for mise `~/.local/bin`, `~/.cargo/bin` when `HOME` is absolute), never through `PATH`; a candidate must be an executable regular file (a symlink such as Homebrew's is followed) owned by root or you and not group or world writable, in a directory owned by root or you that is not world-writable and is group-writable only for the macOS `admin` group (admin members can already use sudo, so this grants nothing new; it is standard Homebrew's `/opt/homebrew/bin`), else it refuses (it is never skipped for a later one; see "Programs swamp runs, and from where"); the child's environment is built from scratch
(only `HOME`, a fixed `PATH`, colour, pager and Homebrew auto-update/analytics/cleanup
off, and, for mise only, the directory settings the mise detector honors and mise itself reads: `MISE_DATA_DIR`, `MISE_CONFIG_DIR`, `MISE_CACHE_DIR`, `MISE_GLOBAL_CONFIG_FILE`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, so the probe describes the store the unit measures); and it runs from `/`, so a project
directory cannot change what mise lists. Output over 1 MiB is refused. A hold (a default, a global tool, an install on request) read more than a day before the listing is `unknown` and held out, and one read hours before says so. Every quote shows
when it was recorded and says `older than this listing` when a later observation did not
run the pass; a quote naming one version of a tool says so. A missing
binary, a time-out, a non-zero exit or output that is not the expected shape is
one `not observed` line in the view's coverage notes, never an error. The answers
are stored in `manager_facts.parquet`, a table older versions ignore (the store
marker does not change); a store without it reads as "not observed yet".

**JSON** (`result` of the `--view reclaim` envelope):

```text
observed_at
scope        { projects, declared_roots, complete, incomplete_because[], statement }
totals       { count, bytes, regenerable_bytes, held_bytes,
               not_regenerable_bytes, not_established_bytes,
               per_kind[ { kind, count, bytes, regenerable_bytes } ],
               scope_statement }
coverage_notes[]
rows[]       { path, kind, detector, bytes, growth_bytes?,
               regeneration { class, words, source },
               last_used { at|null, source, atime? }, last_used_text,
               consumers { declared[], recorded_links[], unknown[], summary },
               manager[ { manager, subject, quote, attribution } ],
               hold? { kind, label, subjects[], whole_unit },
               removal { kind, text }, regenerable_bytes, held_bytes, note?,
               children[ { kind, name, bytes|null, measure, last_used,
                           last_used_text?, text, manager[], hold? } ] }
```

`bytes == regenerable_bytes + held_bytes + not_regenerable_bytes +
not_established_bytes`; a row's children add up to its `bytes` (`bytes: null`
is not measured). `totals` is the object the storage headline reuses (`remainder_bytes` is the part of it that is not developer storage; see "Developer storage: the headline").

In the TUI, Reclaim is the first view of the Tools section (`2`, or `Tab` from Projects). It is built from the
stored facts, scans nothing on open, and keeps the same layout as every view:
the scope statement sits under the heading, the cost, last-used fact and removal
path are the signals (and the detail pane's first lines at any width), `→` or
`Enter` opens a unit onto its folders, and a default is flagged beside its
name. `R` refreshes exactly as before.

### Standalone Cargo target directories

A directory `CARGO_TARGET_DIR` builds into, inside a root you declared,
is recognized by Cargo's own signature: a regular `CACHEDIR.TAG` whose
first line is `Signature: 8a477f597d28d172789f06886806bc55` **and** a
regular `.rustc_info.json` beside it. It shows in the unowned view as
`standalone-cargo-target` with its allocated size, its modification time,
and the consequence "rebuild with `cargo build`".

- A directory with only the tag (pytest, uv and others write the same
  signature) is not called Cargo's, and neither is a `target/` sitting next
  to a `Cargo.toml` (a Cargo project that is not a git checkout keeps its
  build directory there).
- Swamp does not link it to a project and guesses none. Whether the
  directory itself records one varies, and was checked here: a target built
  from a git worktree records absolute workspace source paths in
  `<profile>/deps/*.d` (`/private/tmp/swamp-cli-brokenpipe-target-145` records
  `/private/tmp/swamp-fix-cli-defects/crates/...`), while targets built in
  place record relative ones and `.fingerprint/*` records only hashes. When
  absolute paths are there the row shows their common directory as a
  **recorded link** (second tier, labelled "recorded in dep-info, not checked
  to exist"); it is never used to select, order or authorize anything.
- It has its own section in `swamp report --view external` (and
  `standalone_cargo_targets` in `--json`, and rows in the TUI's External view),
  counted under unowned and not in the external total.
- A project's own `target/` is a build artifact of that project and is
  counted once there, never also as a standalone target.
- It is plannable through the same reviewed Trash flow as any build output:
  mark it in the TUI's unowned view (the plan is `actions::propose`'s, the
  same call every other plan unit goes through). The confirm line says what
  it is, what a rebuild costs, and shows the in-use reading taken when it
  was planned. Nothing is removed without the human's Enter.
- Only roots you declare are scanned; `/private/tmp` is not in the default
  scope.

## Agent-tool storage

Coding-agent tools (Claude Code, Codex, its desktop app, Oh My Pi,
OpenCode, Gemini CLI, Pi, Aider, GitHub Copilot CLI, Cursor, Windsurf,
Cline, Roo Code, Continue -- every tool in the required matrix now has
real identification code) keep session
transcripts, caches, logs, checkpoints and
configuration under their own home directory. `swamp` identifies that
storage the same way it identifies external storage above -- the home
directory itself is one external unit -- and additionally classifies
its *interior* into finer-grained units (sessions, caches, logs,
checkpoints, protected config, ...), each with its own size/growth
history and, where evidence supports it, a link to the swamp project a
session's `cwd` names:

```bash
swamp report --view agents
swamp report --view agents --project my-repo
swamp report --view agents --json
```

Read `docs/agent-storage.md` for the full category/linkage/privacy
contract and the required tool matrix (which tools are identified
today vs. named-and-planned). In short:

- **Privacy is a hard contract.** Identification reads directory names,
  file sizes/mtimes, and -- for a session's project linkage -- bounded
  metadata from the tool's supported source. Claude Code uses a capped
  transcript header; Codex uses its read-only SQLite thread index and
  reads no rollout transcript content. A Claude Code session whose `cwd`
  is absent, missing, or not a checkout may be linked by *inference* from
  its `projects/<slug>` folder name when that slug re-encodes exactly one
  known worktree; the report retains the failed `cwd` reason, says
  `inferred`, and recomputes the link each pass
  (`docs/agent-storage.md`). No
  prompt, response, attachment or credential content is ever read into
  a report, a plan, the ledger, or a log.
- **Categories carry different consequences.** Cache/log categories
  (`shell-snapshots`, `debug`, plugin/skill `.trash`) are regenerated
  automatically and are, with the legacy directories below, the only
  categories the TUI can mark for the Trash. A few directories are
  neither: Claude Code's `statsig/`, `logs/` and unmatched `todos/`
  entries are documented upstream as legacy and **no longer written**, so
  removing them costs nothing and nothing comes back. Removing a session
  means losing its resume/rewind/checkpoint history -- the linked
  project's own files are never touched. Credentials, settings, skills, commands and
  similar automation definitions are kept by default: `A` leaves them out,
  Space marks one, and its confirm says what the tool loses (it may stop
  signing in or lose settings until restored from Trash). A category swamp
  has no rule for, and a database file (SQLite, WAL, SHM), mark the same
  way with their consequence named. Only your own `swamp protect` mark
  refuses.
- **A tool whose layout is not confirmed against its own source is
  identified, and its paths can still be moved.** `swamp report --view
  agents` measures Cursor's and Windsurf's storage and reports linkage as
  unresolved; the TUI marks each path individually, and the confirm says
  the layout is not verified against upstream. The same holds for GitHub
  Copilot CLI's `session-state/`: the *directory names* are documented by
  GitHub, what is inside them is not. `docs/agent-storage.md`'s matrix
  says which is which, and every row's citation is pinned to an upstream
  commit and checked by CI.
- **Human keep/protect intent survives refresh:**
  `swamp protect add <path>` / `swamp protect list [--json]` /
  `swamp protect remove <path>` -- independent of, and never overridden
  by, anything observation infers. The TUI's mark step refuses a
  protected unit, or one containing a protected path.
- **Removal is TUI-only.** Space marks a session/cache row, Backspace
  shows its current facts (what member files exist, what removing them
  costs), Enter moves it to the Trash and appends one ledger line.
  Nothing wider than the exact marked unit is ever affected. A session
  removal that partially fails (some members moved, then a later one
  could not be) leaves a `restore.json` recovery manifest inside its
  Trash envelope naming exactly what moved and what did not.
- **Project-linked view:** `swamp report --project <name>` (text or
  `--json`, no `--view` needed) includes this project's own linked
  agent storage -- a collapsed "Agent storage (linked)" summary row per
  contributing tool in the text tree, and an `agent_storage: {units,
  total_bytes}` object in JSON.

The TUI has a dedicated Agents view (Tools section: `2`, then `v` to Agents): the same per-unit facts as `--view agents`. `Space`/
`Backspace` mark the selected unit and open the confirm banner showing
its real consequences (session-removal loss warnings, the linked
project); `Enter` moves it to the Trash through the same
background-worker path every other TUI deletion uses -- never blocking
the event/render thread. A protected row, or one whose category has no
Trash move at all (credentials, settings, an unconfirmed layout), cannot
be marked; the status rows name the exact reason. Bulk marking (`Shift+A`)
reaches the Agents view too: it marks every markable row on screen the
same way, skipping protected/unmarkable ones and naming the skip in the
status rows. The project tree's own Tree view also shows the collapsed
"Agent storage (linked)" summary row (informational; marking a specific
unit still happens in the Agents view).

## The TUI's sections and views

The TUI has three sections, each holding a few views. `Tab` and `Shift-Tab` move
between sections, `1` `2` `3` jump to one, and `v` cycles the views inside the
current section (wrapping). Nothing else opens a view.

| Section | Views (first is the default) |
|---|---|
| 1 Projects | Projects, Tree, Builds, Deps, Types, Kinds, Unowned |
| 2 Tools | Reclaim, Docker, External, Agents |
| 3 Disk | Summary (the stored volume ledger's parts), Not measured (unreadable and not-yet-measured folders, the largest measured folders outside developer storage) |

A row under the headline names the three sections with the current one in reverse
video; the line below it names the view (`view: Tools › Reclaim (1 of 4 · v next)`).
`?` help lists every section and view with a line on each. **Changed in 0.8.0:** in
0.7.x the digits `1`-`9` selected views; now `1`-`3` select sections and the old
digits are unbound.

## Developer storage: the headline

The first thing `swamp report` prints, the top of the TUI and the top of
`swamp report --view reclaim` is one line:

```text
Developer storage: 224.1GB across 40 projects and 77 tool locations (57.4% of used)
```

It reads stored facts only: no listing, no `stat`, no program (a test counts
work and asserts zero). Sizes use the one byte formatter (decimal, `GB`).

**What it counts.** Allocated bytes of:

- **projects**: everything under the declared (or built-in default) source
  roots, less the standalone Cargo target directories found there (they have
  their own row), **plus git worktrees outside those roots that a checkout's
  worktree registry names** (on the reporting machine 23.9 GB of a 42.0 GB
  projects row: mostly agent worktrees under `/private/tmp`, and
  `~/.codex/mcp/...`). The unit that holds such a folder already leaves the
  worktree out (`bytes_counted_elsewhere`), so nothing is counted twice in the
  headline;
- **toolchains and SDKs** (installations and environments), **caches**
  (downloads, caches, build output), **agent storage** (an AI tool's home,
  from the detector's own declaration), **containers and VMs** (Docker,
  OrbStack), **other developer units** (local state, models, and anything a
  build of swamp does not place: a unit is never left out for want of a row).
  A folder the catalog cannot attribute to one tool (category `unclassified`:
  `~/Library/Caches`, 36.5 GB on the reporting machine) is counted here and named
  on its own line as "other, mixed owners (not only developer tools)";
- **standalone Cargo targets**.

**What it never counts:** system volumes; the measured "Everything else"
bucket; the *remainder* of a location after its developer tooling (Homebrew's
"other", selected by the detector's `remainder_of` capability, never by name);
mounted disk images (a view of image files: the disk cost is the image files
themselves, which the disk view lists under **Everything else**, not in
developer storage; the mounted size is taken out only when the disk ledger knows
the mounts); the unattributed residual; and
protected or not-yet-measured estimates. A location is counted when it holds
bytes; the count is the number of rows added.

**The percent** is developer storage divided by the container's used bytes from
the disk ledger, the number `df` calls used, never the Data volume's. It is
rounded **down** to one decimal, in integer arithmetic, so it is never larger
than the truth (99.96% reads 99.9%). It is absent, and the line says why, when
there is no ledger yet (`disk ledger: not measured yet; run swamp observe
--volume`), when the ledger is unreadable, written by a newer swamp or dated in
the future, when the container's used bytes are missing or zero, when the report
covers only a root named on the command line, and when developer storage is
larger than used (a `FLAG` line instead: a measurement is wrong or counts shared
bytes twice; never a percent above 100).

Below the line, in order:

- the ages: `observed 4 min ago; disk ledger measured 3 h ago`, and, when the
  ledger is more than a day older than the observation, that the percent divides
  newer developer storage by an older disk reading;
- when the numbers cover a previous scope (roots changed since), or one root
  named on the command line, the sentence that says so;
- one row per breakdown category with its bytes and count. The rows add up to the
  headline **exactly** (in bytes; the exact figure is printed once, after the
  rows, because each row is rounded on its own);
- what was left out on purpose, with bytes (the remainder unit, mounted images);
- from the ledger: **Everything else** with its five largest measured folders,
  **System volumes** (named, one sentence on sharing the free space),
  **Not measured: N directories** with the first names, and, when something is
  unreadable, "protected folders ... the unexplained part of the Data volume, up
  to X, may be inside them (an estimate, not part of any check)";
- the walk's spot audit: `FLAG: walk spot audit disagrees: see swamp report
  --view disk` when it disagrees (the only check that can catch a walk that
  undercounts), otherwise how many folders were audited.

**How the numbers relate.** On one store, with `walked` the source roots'
walked total and `standalone` the standalone Cargo targets:

```text
developer storage = (walked - standalone)                        projects
                  + Reclaim totals (regenerable + held out
                    + not regenerable + cost not established)   units and targets
                  - Reclaim remainder units
                  - mounted disk images
disk view "accounted" = developer storage + remainder units
```

`swamp report --view reclaim` prints the first line after its headline, and its
JSON carries `headline_relation` (`holds`); `headline.disk.accounted_check`
carries the second. They differ when units on another volume are counted here but listed
apart by the ledger, when worktrees outside the declared roots count under projects
but not in the ledger's declared rows, when the ledger's accounted rows were measured
at another time than the units read here, or when a unit's row was lost. The line
names the other-volume bytes and the unexplained rest as numbers, and mentions the
measurement time only when the two times differ.
Whenever they differ the report prints a plain line (`disk view check: the
ledger's accounted bytes (X) differ from developer storage plus the remainder units
(Y) by Z`) and the Disk view repeats it on its Accounted row. (A same-path
collision that used to lose ~/.codex's 6.8 GB is fixed: an agent tool's home at a
catalog unit's path folds into that unit's row as a note.)

A report that covers a previous scope shows no percent (it would divide one scope's
storage by today's disk). When the observation is more than a day older than the
ledger, or the ledger more than a day older than the observation, the ages line says
which way the percent mixes them.

`swamp report --json` carries the same numbers as a `headline` object
(`developer_bytes`, `locations`, `categories`, `not_counted`, `percent_of_used`,
`disk` with its `state`, `ages`, `flags`, and `line`, the text's first line).

In the TUI the same block is four rows under the header on every view and in
every state (headline, breakdown, disk state and ages, then the pointers to
Reclaim and Disk), two rows on a terminal under 22 rows tall (the headline and
the pointers), one under 16, none under 12. Nothing changes its height, so no
row moves when a warning appears. Until you have opened Tools or Disk once, the
pointer row says `New: Tab opens Tools (Reclaim) and Disk. Hides after you open
either.`; that is remembered in `ui_state.json` (`views_seen`; an older swamp
ignores the key).

## Where the whole disk went (the volume ledger)

`swamp report` answers for the roots and locations swamp knows. Everything
else on the disk (and the parts of the disk no path reaches) is the volume
ledger's job: one scheduled measurement, read back without touching the disk.

```bash
swamp observe --volume        # measure now (a plain scheduled observe does it when due)
swamp report --view disk      # read it: never a walk, never a program run
swamp report --view disk --json          # totals plus the 50 largest rows
swamp report --view disk --json --all    # every row
```

`report --view disk` prints, each with the time it was measured:

- **Disk**: the APFS container's total, used and free (one `statfs`). `df`'s
  "used" is the whole container, not the Data volume.
- **Accounted**: the catalog and declared locations, taken from the
  observation that just ran (not walked again), counted once. Units are
  disjoint as the observation measures them (`/opt/homebrew` without the
  formulae under it), so nested units add up. An agent tool's sessions and
  caches are a finer view of a folder measured whole elsewhere: shown, never
  added, and their folder is walked like any other.
- **Everything else**: a coarse measurement of the rest of the data volume,
  one row per folder at depth 1 of `/` and depth 2 under your home, `/Library`,
  `/opt`, `/private`, `/Applications`, `/Users` and `/System/Library`, with the
  five largest shown. Sizes are allocated bytes (`st_blocks`), `lstat` only: no
  file is opened, no symlink followed, no FIFO or socket touched. A file with
  several hardlinks is counted once for the whole pass (a bounded set of about 68 MB; past two
  million linked files a link may be counted twice, and the run says so; a
  hardlink whose two folders were measured in different runs of a resumed pass
  is counted in each run).
- **System volumes**: System, Preboot, Recovery, Update, VM and the like from
  `diskutil apfs list`, with the sentence that they are separate volumes
  sharing the container's free space. This machine's Data volume is the one
  `diskutil info` says is mounted at `/System/Volumes/Data`; any other volume
  with the Data role in the container is its own row. **Purgeable** space and
  local **snapshots** (`tmutil listlocalsnapshots /`, names only: `tmutil`
  reports no sizes) appear when `diskutil` and `tmutil` say so, and are never
  added to the total (purgeable space is already inside the folders above).
- **Not measured**: every folder that could not be read (macOS privacy-protected
  folders such as Photos, Mail, Messages, Safari, Group Containers and
  Containers), with the exact count and the first 200 names. Never zero, never
  dropped. swamp does not ask for Full Disk Access; the report only says it
  would change this. **Not measured yet this pass** lists, by name, the
  locations the cursor has not reached (an unfinished pass).
- **Protected folders: not measured (N folders)**, with the sentence "the
  unexplained part of the Data volume, up to X GB, may be inside them". X is the
  Data volume's own consumed bytes minus everything measured. It is an
  ESTIMATE, it exists only while something is unreadable or not yet measured, and
  it is not part of any check.
- **Unattributed: allocation not explained by any measured part (bookkeeping)**:
  the parts, estimate included, minus the container's used bytes, signed
  (negative when clones or shared extents were counted once per file). That the
  parts add up (`bookkeeping_balanced`, within 1%) is arithmetic, not evidence:
  the estimate is a leftover, so it can balance anything, and a `FLAG` prints only
  when even that arithmetic fails. It says nothing about whether the walk is
  right. `unexplained_bytes` in the JSON is the container's used bytes minus the
  measured parts alone (protected folders included); on a Mac with protected
  folders it is large, and that is not a claim about the walk either.
- **Walk spot audit** is the check that can fail. Each pass, after the walk and
  inside the budget, up to five readable folders measured this run (the largest
  one always, the rest rotating by day number, never a folder with an
  unreadable part) are measured again by a naive, independent method: a plain
  recursive sum of `st_blocks * 512`, a hardlink once per audit, sharing only
  the system calls with the walker. Each is compared with the ledger's row; a
  difference beyond `max(1%, 4 MiB)` sets `audit_flag` and prints `FLAG: walk spot
  audit disagrees on <path>: ledger X vs audit Y (Z%)`. Otherwise the report says
  "walk spot-audited: 5 folders, max difference 0.3%". Audit time is part of the
  budget; a spent budget skips it with a note. The results are stored as
  `audit` rows in the ledger (no new columns).
- **On other volumes**: a declared root on another volume is listed as
  "not part of this container" and never added to the internal disk's
  accounted bytes.

Mounted disk images (the simulator runtime volumes under
`/Library/Developer/CoreSimulator/Volumes`) are a *view* of the image files
stored under `/System/Library/AssetsV2`: those image files are counted once,
where they are stored, and the mounted volumes are listed as "not added"
notes. Another volume of the same container and a network share are listed the
same way, with no size.

### How it runs

- **Only in `swamp observe`.** Never on `swamp ui` open, never in `report`.
  A plain `observe` runs it when the last complete pass is older than
  `volume_pass_interval_hours` (default 24; `0` turns the automatic pass off).
  `observe --volume` runs it now. It needs the configured scope: with explicit
  roots the accounted part would be those roots only, so it is refused (and the
  automatic pass does not run when `$HOME` is not the account's home directory,
  as in a sandbox or a test fixture).
- **After the observation, under its own lock.** The observation finishes and
  releases its lock first; the pass takes `volume-pass.lock` (transient), so a
  slow or stuck pass can never make a scheduled observe say "another
  observation is running". The observation writer lock is taken only for the
  two small ledger writes.
- **The mount table is read first.** A mount point is never statted, and nothing
  behind a network, FUSE or automounter filesystem (`smbfs`, `nfs`, `afpfs`,
  `webdav`, `fuse*`, `sshfs`, `cifs`, `9p`, `autofs`) is touched: a stalled
  server cannot hang the pass. Every mount is listed and none is entered. (On
  Linux, a mount that shares the root filesystem, such as a btrfs subvolume at
  `/home`, is measured as a folder of its own rather than dropped.)
- **Low priority, bounded, resumable.** Its threads run at background
  priority, three at a time. One run is bounded by `volume_pass_budget_secs`
  (default 120; values under 5 are raised to 5): the three system queries
  (each killed after 20 s) and planning (limited to half a budget; a plan that
  hits it is incomplete and the cycle is not called complete) come first, the
  walk stops at the budget minus a slice kept for the spot audit (checked per
  entry, so mid-folder: a folder that did not finish leaves no row), and when
  those overheads leave the walk less than half a budget it gets half a budget
  anyway. Past 2 budgets from the start of the run the pass stops waiting for a
  worker stuck in a system call. The real bound is therefore 2 budgets plus the
  system queries. A stuck folder is recorded as not measured, the path is
  logged, the lock released, and the NEXT run tries it again; after three runs in
  a row it is skipped until the next cycle (with the date) and that is logged once.
  The cursor is stored in the ledger: every row keeps its own measured time, and
  a partial pass shows a partial ledger with honest ages. A folder that fills a
  whole run by itself is measured as its children from then on. An unfinished
  pass continues at every observe whatever the interval says. Rows dated in the
  future are not believed and are measured again. A ledger file that cannot be
  parsed is moved aside as `*.corrupt-<time>` (removed after seven days) and the
  pass starts fresh; a file that merely could not be read (an I/O error) is left
  where it is and the pass is skipped.
- **Skipped, with one line, when the disk-full guard trips** (`min_free_bytes`)
  or when the store's format marker is not this build's generation (older, or
  newer: this build never writes into a newer swamp's store).
- **New files only.** The ledger is `volume_ledger.parquet` and
  `volume_ledger_meta.parquet` in the store. No existing table changed, the
  store-format marker did not move, and an older swamp ignores both files. A
  format reset leaves them alone: the ledger is a measurement, not derived from
  another table.
- **The programs are fixed.** `diskutil` and `tmutil` run from `/usr/sbin/diskutil`
  and `/usr/bin/tmutil`, never through `PATH`, like every program swamp runs.
- **A second pass over an unchanged disk gives identical bytes but is not
  faster.** There is no event replay for the whole disk (yet), so every pass
  measures every row again; the budget bounds it instead.
- **On Linux** the container is the filesystem under `/`, the mount table comes
  from `/proc/self/mounts`, and there are no system-volume, purgeable or
  snapshot lines (nothing to ask).

## Cleanup recommendations

Age is a cleanup signal, not a proof requirement. Supported Cargo cleanup groups
are ranked oldest-modified first, then largest when ages match. Missing or future
timestamps sort last. There is no minimum-age gate: recent builds remain reviewable.
The project tree shows modification age and rebuilding cost.
Modification age is not last execution or access time. Existing project/worktree
activity signals provide additional context; they are not required to suggest a
build cleanup candidate. Old is a reason to look, never a verdict -- the human still decides.

## Terminal controls

```bash
swamp ui ~/src
```

With no subcommand, `swamp` opens the UI at the current directory. It paints the last stored report at once, at any age, and scans only when there is none (in the background, with progress in the header). `R` refreshes on demand; if another process is already observing, `R` says so instead of starting a second walk. The UI opens no filesystem watch and, when nothing changes, draws nothing.

| Key | Action |
|---|---|
| Up / Down | Move selection |
| PgUp / PgDn | Move one screenful; in help, the blocked list and the picker they scroll or jump the same way |
| Home / End | First / last row (or first / last help line, blocked item, picker field) |
| Right / Left | Open or expand / collapse or return |
| Enter | Open a project or confirm the pending action |
| Esc | Cancel the current interaction or return to projects |
| Space | Mark or unmark a row |
| Backspace | Request removal of the selected row or marked set |
| `A` | Mark actionable rows in the current view, excluding the checkout fallback |
| `k` | Toggle keeping supported compiled outputs before removal. The result line says whether it is now on or off; the choice is remembered |
| `b` / `d` | List what the last check or delete could not include, with the reason and next step (`d` on the plan). Inside the list, `r` checks again |
| `/` | Open the filter form |
| `:` | Edit the filter expression; Tab completes terms |
| `0` | Clear the filter |
| `Tab` / `Shift-Tab` | Next / previous section: Projects, Tools, Disk. While you type a filter, Tab completes it as before |
| `v` | Next view inside the current section, wrapping around |
| `1`, `2`, `3` | Jump to a section: 1 Projects, 2 Tools, 3 Disk (help lists them; the legend does not) |
| `g`, `s`, `n`, `t`, `a` | Sort by growth, size, name, ecosystem, or age |
| `r` | Reverse the sort |
| `?` | Show help |
| `q` | Quit (closes help, the list or the picker first; while a check or a move runs, it stops it after the current item) |

The initial filter is `growth > 100MB in 7d`. Filter, sort, reverse, and keep-executables choices are saved in `ui_state.json`. Clear the filter if the first observation shows no matching rows.

Rows show size and signed growth. Red bars extend right for increases; green bars extend left for decreases. Bar length is logarithmic, so use the number to compare exact changes.

## Report views

Run `swamp observe ~/src` at least once before any of these -- `report`
never scans on its own (R12).

```bash
swamp report ~/src --all
swamp report ~/src --project api
swamp report ~/src --project api --dirs --depth 2
swamp report ~/src --view builds
swamp report ~/src --view rust
swamp report ~/src --view deps
swamp report ~/src --view types
swamp report ~/src --view docker   # also: BuildKit records per builder, in the daemon's terms
swamp report ~/src --view worktrees --filter 'merge-complete idle > 48h'
swamp report ~/src --view unowned
swamp report ~/src --view reconciliation --verify-du
swamp report --view disk         # the whole-disk ledger; needs no root and no observation
```

Replace `api` with a project name from your report. Additional views include `kinds`; `--worktree <path>` prints one worktree's signals.

`--view builds` now ends with a per-container breakdown for every build
container swamp could identify the interior of -- Cargo, Node, Gradle,
Maven, Python, Go, Xcode/SwiftPM and Android. Each container prints one collapsed row per role family
(outputs, tests, intermediates, dependencies, shared store, metadata,
residual): the family, review guidance, and what removing it would
cost in that ecosystem's own words, then the count, the size **with its
accounting basis stated**, and the oldest known *modification* time.
Only nonempty supported units are counted; unrecognised entries and
bytes no unit accounts for are one "Not identified" line, so the
families and the residual add up to the container. Units of unknown age
are counted separately and never rank as ancient. `--view builds --json`
and `--view deps --json` carry the same summary and every unit under an
`interior` key on each identified row. In the TUI, Space marks supported
project-local outputs, test output and intermediates, individually or by
family. The selection uses the same non-overlapping members as the displayed
family. A shared store, an installation or an unknown layout has no cleanup rule: Space on its own row still moves that exact path to Trash, and the confirm lists what the adapter could not establish. See
[docs/build-artifacts.md](build-artifacts.md) for the capability matrix,
the per-ecosystem layouts that are covered, and the attribution limits.

Identification never runs `npm`, `gradle`, `mvn` or `cargo`, never loads
a JavaScript config file, and never evaluates a Gradle build script or a
Maven plugin. Where the answer is only available that way, the row says
so: a `build/` subdirectory a plugin chose is an unidentified residual,
a `package.json` that cannot be read leaves the package's identity
unknown rather than guessed from its directory name, and a Maven
artifact's origin comes from its `_remote.repositories` entries (a
repository id means downloaded; an empty id means `mvn install`) or
`maven-metadata-local.xml`. With neither -- or with only a
`*.lastUpdated` attempt record -- it has **unknown origin**, and swamp
does not promise it can be downloaded again. Machine-wide stores (npm
cache, pnpm store, Gradle user home, `~/.m2/repository`) are reported
as whole external units for now; their per-entry identification exists
but is not yet joined into the report (see
[docs/build-artifacts.md](build-artifacts.md)). The Rust view explains Cargo target/build storage as nested containers, profiles, dependencies, test/example outputs, build-script output, incremental state, final outputs, and companion metadata. Dependencies remain a folded directory aggregate, not a per-crate breakdown. Group sizes are allocated bytes; unknown subgroup hardlink charges are not reclaimable-space estimates. The view prints evidence limits and unknown variants. Final outputs have no selective cleanup rule: each can be marked on its own row.

Rust inspection does not invoke Cargo or build scripts. It reads layout and existing fingerprints; hashed filenames alone do not establish ownership, last execution, or obsolescence. Opening a project in the TUI shows cleanup groups under each build profile: **Compiler caches**, **Compiled tests & examples**, and **Build-script output**, when supported members exist. Space marks a group's exact members for review; Backspace opens confirmation. Expand with → to choose Tests, Examples, or individual age-ranked members instead. Unrelated dependencies are not part of these groups. **Inspect directories** retains the physical layout as another view of the same bytes. No switch to Builds is required. The selected-row details explain cleanup recommendations and rebuilding consequences. Incremental compiler caches are suggested as a starting point if slower subsequent builds are an acceptable trade-off—not because Swamp has proved them obsolete. Compiled dependencies remain a folded aggregate without selective dependency cleanup.

In the project tree or Builds view, mark an identified test/example executable or an individual incremental/build-script directory to review an exact cleanup group. Purpose groups and profiles mark their supported members, not the entire profile directory. Executable groups include existing dep-info and debug-symbol companions. Cleanup is TUI-only; there is no CLI cleanup plan or approval command.

### On-demand Cargo dependency inspection

Select a Cargo profile in the TUI and press `i`. Inspection runs in the
background; Esc or Ctrl-C cancels while it runs. The results are scrollable
with ↑/↓ and close with Esc. To request the same details from the CLI:

```sh
swamp inspect-cargo ./target/debug --json
```

This reads only that profile's immediate `deps/` entries and bounded
`.fingerprint/` metadata. Default limits are 262,144 entries, five seconds and
8 MiB of metadata; a limit produces explicitly partial results. It does not
run Cargo, read source files, or save a file inventory. Ordinary scans keep
dependencies folded.

For a large profile, request a larger bounded inspection with
`--max-entries 262144 --max-ms 30000`. These are hard ceilings, not a new
background scan schedule. A partial result describes only inspected entries.

Groups name targets and variants when fingerprint evidence matches; target
names are not package identities. Unmatched or ambiguous files remain
unattributed. Allocated bytes may count hardlinks repeatedly; the unique
total deduplicates only the inspected files, and neither means reclaimable
space. These diagnostics do not enable per-crate deletion.

### Build details: choose what to give up

Open a project with → to see cleanup groups beneath each Cargo build profile. Choose by the cost of rebuilding, then expand a group if you want to remove only older members.

![Swamp's project tree showing debug compiler caches, compiled tests and examples, and build-script output, with sizes, removal consequences and an oldest-candidate preview.](images/cargo-build-cleanup.png)

Its sizes and ages are one observation of Swamp's own build directory, not expected savings for every project. Old `slop_livin` names are build artifacts left from the project's earlier name.

| Group | In this screenshot | What removal changes |
|---|---|---|
| Compiler caches | 11.7 GB allocated, 617 groups | Discards incremental compiler state. Start here if a slower subsequent build is acceptable. |
| Compiled tests & examples | 8.8 GB allocated, 206 groups | Removes identified test executables and examples. Rebuild before rerunning; unrelated compiled dependencies are not selected. |
| Build-script output | 123.6 MB allocated under debug | Scripts run again on a later build and may need external tools or network access. |
| Inspect directories | Another view of the profile's bytes | Shows the physical layout, including dependencies, final outputs and metadata. It is not another cleanup group or additional storage. |

**Choose a group or individual members.** Space marks a cleanup group's exact supported members. → expands it; Compiled tests & examples splits into Tests and Examples, then individual members ordered by modification age. Backspace opens review for the marked selection, Enter confirms, and Esc cancels confirmation. Marking a fully marked group clears its marks. A failed member check rolls back newly added marks rather than silently selecting only part of the group. Stop builds before cleanup; marking can take time because it checks the selected contents.

**Read age as a suggestion, not proof of disuse.** `Oldest 3d` means the oldest known modification age among the group's candidates, not that every member is three days old or has gone unused for three days. Expand to choose older members; selecting the collapsed group includes recent members too. Unknown age appears as `?`. The lower preview shows only the stated subset—31 of 617 in this screenshot—not the full selection.

**Do not add all the displayed sizes.** A `*` marks allocated bytes, which can count shared hardlinks more than once. That is why debug can show 34.5 GB while the containing build target shows 30.1 GB on a different accounting basis. Parent rows include their children, and Inspect directories repeats the same storage by path. A dash in a purpose group's Change column means no aggregate growth value is supplied, not zero growth.

**Candidates are not guaranteed free space.** The debug profile's 20.7 GB candidate total covers supported cleanup members, not all debug output. Missing groups or “Selective cleanup unsupported” describe Swamp's action support, not a requirement to retain those files. Final outputs and the remaining compiled dependencies have no selective cleanup rule; each is marked on its own row. Space or Backspace on a profile reviews all supported cleanup groups beneath it, not the entire profile directory. Use the candidate total, not the profile's full size, to understand that selection.

Cleanup moves supported filesystem groups to Trash; those bytes are not immediately freed. Emptying Trash later may reclaim space, but surviving hardlinks and filesystem snapshots can limit the result. Source files and unrelated dependency artifacts are outside these purpose-based selections. Protection is checked when marking, and Cargo's advisory lock is held during removal. There is no post-mark occupancy veto or separate approval grant.

### Progress and cancellation

Marking runs review checks in the background. After you confirm, a **Deleting**
bar shows processed/total groups, successful and refused counts, elapsed time,
and the current path. It measures groups processed, not bytes freed. Review and
deletion keep the terminal responsive; additional actions wait until they finish.

**Esc or Ctrl-C stops after the current group.** Swamp finishes that group's
check or move and records its outcome before stopping. Completed moves remain in
Trash; refused and unattempted selections remain marked for explicit review or
retry. Cancelling review preserves the selection you had before review started
and does not delete anything. Ctrl-C exits when no operation is running.

### Reviewing Cargo build groups

The Rust text view shows the largest 30 units per container by default; add `--all` for the full list. Category totals include their children: do not sum them. A category is not an individual cleanup selection. Report JSON includes the same guidance under each nested row's `cleanup` field.

There is no `swamp cleanup-check` command any more. Report and inspection commands do not remove scanned data; `observe` writes report state. To act on a Cargo purpose group, open the TUI's Rust view, mark the group (Space), read its current facts on the confirm banner (Backspace), and press Enter -- the group's exact member list (selected build output plus its `.d`/`.dSYM` companions) moves together into one Trash envelope with a restore manifest. Cargo's advisory lock is held for the duration of the move (so a concurrent `cargo build` does not race it), not as a "did anything change" check.

The CLI's *text* rendering applies `--filter` only to the root `--view worktrees` output; it does not filter the builds view, project drill-down, or overview text. For a filter that narrows every row, add `--json`: see below and [the agent interface](#agent-interface).

`--json`, with or without `--view`, applies `--filter` (when given) to the whole report before computing any output, and scopes to `--project` when given -- narrower than an unfiltered dump and consistent between the full report and every named view:

```bash
swamp report ~/src --json | jq '.summary.by_type'
swamp report ~/src --json | jq '.reconciliation'
swamp report ~/src --view grown --json --filter 'kind:BuildOutput' | jq '.result.grown'
swamp report ~/src --json |
  jq '[.projects[].worktrees[].artifacts[] | select(.growth_bytes > 1000000000) | {path, growth_bytes}]'
```

Large results can be bounded with `--limit`/`--offset`; the envelope's `total`/`truncated` fields say whether a page is the whole answer. See `skills/swamp/references/commands-and-json.md` for the full schema.

Filesystem reconciliation and Docker accounting are separate. `--verify-du` adds an independent `du -skPx` comparison and can take extra time.

## Filters

Supported interfaces share the core parser, but apply predicates to their own row types. Combine predicates with spaces:

Growth predicates filter the report's already-computed growth values. Their `in <duration>` clause does not currently recompute the baseline. For an explicit comparison, run `swamp observe --since <window>` before `report` (window moved to `observe`, R12). The TUI obtains its report window from configuration; changing the filter form's window can change the displayed label without changing those measurements.

| Expression | Meaning |
|---|---|
| `growth > 500MB in 7d` | Grew by more than the threshold; requires a report computed with the same window |
| `growth < 500MB in 7d` | Shrank by more than the threshold; not “grew by less than 500 MB” |
| `size > 1GB`, `size < 10MB` | Size threshold |
| `age > 30d` | Artifact's newest recorded modification is older than this; unknown age does not match |
| `idle > 48h` | Worktree idle time exceeds the threshold |
| `kind:BuildOutput`, `kind:deps` | Artifact kind by enum name or display label |
| `type:rust`, `type:js`, `type:python` | Ecosystem |
| `project:api`, `project:api-*` | Project name substring or glob |
| `merge-complete` | Combined branch-merge, clean, and unpushed facts |
| `pr:open`, `pr:merged`, `pr:closed`, `pr:none` | GitHub PR selection |

Filter durations include `30m`, `48h`, `7d`, and `1w`. Size units are decimal (`1MB = 1,000,000 bytes`); use `MiB` or `GiB` for binary units. Do not treat a `pr:none` match as proof of a successful GitHub lookup: unavailable facts can also lack a PR row.

## GitHub and Docker context

Install and authenticate `gh` to collect GitHub facts:

```bash
swamp observe ~/src
swamp report ~/src --view worktrees
```

Each worktree also carries `tip_reachable`, a fact `observe` computes from the repository's own refs, offline and without `gh`: whether the worktree's HEAD commit is contained in a remote-tracking branch (`refs/remotes/*`, `*/HEAD` excluded). A remote-tracking ref is the state at the last fetch, so the term says so: `tip_reachable=yes (origin/audit/x, as of last fetch Sep 30)`, using the time of `FETCH_HEAD` (else the ref's reflog, else `fetch time unknown`); a branch deleted on the remote since then still reads yes until the next fetch. The local `main`/`master` counts only from a linked worktree, whose own folder can be removed while the shared `.git` keeps that branch, and is named `local main, not pushed`; in a primary checkout a local branch lives in the folder's own `.git` and never counts. It is separate from `merged`, which stays the pull-request fact: a branch with no PR reads `merged=unknown` and can still read `tip_reachable=yes`. The worktree's own checked-out branch is never its own proof; a detached HEAD is judged by its commit. A repository that cannot be opened, a shallow clone where nothing was found, more than 400 remote branches, or a 2-second budget (checked at every commit of the walk) read `tip_reachable=unknown`, never `no`. A squash-merged branch reads `tip_reachable=no` because its commits are in no branch; the `merge-complete` verdict still counts a merged PR as landed.

Reports use the GitHub facts the last `observe` cached. `observe` queries GitHub unless given `--no-enrich`, and reuses a cache entry that is still valid. GitHub cache validity uses the tip SHA: a worktree whose branch is already merged is terminal and is never re-enriched automatically, and every other row is refreshed after a 24-hour TTL. `observe --enrich` is the on-demand override: it refetches everything, ignoring the TTL and the merged rule.

Docker facts are cached for five minutes, with fresh reads during enrichment. Docker must be installed and its daemon reachable. Unavailable Docker data is reported in notes.

Images, volumes, and build-cache records join to projects using Compose metadata or source-remote evidence. Unmatched objects remain unowned. Filesystem and Docker sizes should not be added to predict how much physical disk space an action will reclaim.

## Cleanup and recovery

**Swamp reports; the human removes.** `swamp report` and its views read stored facts. `observe` writes observations, history, and enrichment; `protect add/remove` writes a keep-list, while `protect list` reads it. Configuration and scheduling commands can also write state. Removal is a separate TUI flow: Space marks a row, Backspace shows its current facts, Enter moves filesystem selections to the Trash. Docker image/volume removal uses Docker and has no Trash recovery. Checkouts and linked worktrees can be selected as well as artifacts and Cargo groups. Dirty, unpushed, and untracked facts are shown on the confirm banner for judgment; they do not block removal.

A project action expands to its actionable artifact rows. If it has none, a direct project action can offer the checkout. Bulk marking with `A` skips that fallback. The `ignored` and `untracked` remainder totals cover scattered files, so those summary buckets are not themselves deletion units.

**What you see and own, you may move to Trash.** Every row with a real folder or file behind it can be marked: a Reclaim unit and each folder listed under it, an External unit and its folders, a store-interior folder, a measured folder in the Disk views, a build folder no cleanup rule covers, a config or credentials file an AI tool keeps. The category, the location and what swamp does not know are **lines on the confirm, never a refusal**: local state and models say they cannot be regenerated, an installation says its tool will still list it (and, where swamp runs the tool's own removal, names that command), the whole `~/Library/Caches` says it is the folder every app keeps its cache in, Claude session scratch says a running session breaks, a size with a coverage gap says it is a lower bound, a path outside your home says the system may refuse, "last used: no record" and "regeneration cost not established" say what swamp could not find, and a process holding a file open is named (or "could not be checked"). The confirm lists every exact path and size, then these lines, then "Trash is the way back"; it fits the whole plan on screen or Enter is not offered.

The only reasons swamp refuses, each stated on screen:

1. **Not a real deletable folder or file**: gone, a socket or device, not an absolute path, a `.` or `..` in it, a daemon's record rather than a path (a Docker build-cache record, an `ignored`/`untracked` aggregate, a category total, a remainder row), or a folder the walk could not read.
2. **The OS refuses**: permission denied, a protected system path, a different filesystem. The OS error is the reason.
3. **The plan changed**: the target changed since you marked it (a symlink swapped in for the folder, a different folder renamed into place, a path that now resolves elsewhere). Nothing moves and the mark stays.
4. **The ledger cannot be written**: a `started` row is written before a Reclaim/External move, so a store swamp cannot write means nothing moves.
5. **Overlapping marks**: a folder and one inside it cannot both be marked.
6. **Your own `swamp protect` mark**: `protected by you (...); swamp protect remove <entry> takes the mark off`, where `<entry>` is the keep entry that covers the row (it may be a folder above or below it, not the row's own path). `swamp protect remove` on a path that matches no entry now says `nothing matched`.
7. **Swamp's own ledger or the Trash**: a folder that is, or holds, swamp's store (the one `SWAMP_DIR` names included) or the Trash the move goes into cannot be moved by swamp, because the move is recorded in that ledger: `this holds swamp's own ledger, which records this move; move it yourself in Finder if you want it gone`. A protect list that cannot be read is not an empty list and does not block you: the confirm says `could not read your protect list ... your keep marks were not checked`.

Facts read when you mark a row: what it contains (a folder above known units says how many and which, your whole Library, the system temp folder, Homebrew's whole prefix, a mounted volume's root, the folder swamp was started in), mounted volumes inside it (their bytes live on the disk images, so moving it frees about nothing), a git checkout at it or one level below, a file with other hard links, the Trash on another volume (swamp never copies, so that move is refused), and what holds it open. Enter re-reads the entry and what holds it open, and refuses with "changed since review" if either is different. A failed move leaves a `failed:` ledger row with the OS error, never a `started` row. Two folders with the same name can move in one confirm. Control and invisible characters in a folder name are shown escaped.

A symlink itself can be moved (the link, never its target). The Reclaim, External and Disk rows use the same review: Space marks (and says why if not), Backspace opens the confirm (it never acts directly), Enter confirms, Esc cancels; no other key acts while the confirm is open. `A` marks each top-level unit once; a folder listed under a unit, a path no cleanup rule covers, and what swamp keeps by default are marked one at a time with Space. Moving a folder out of a unit takes its bytes off the unit on screen at once; the next observation remeasures. The result line says "Sizes are from the last observation" and that space is freed when the Trash is emptied. Other Trash moves (artifact rows, worktrees, Cargo groups, agent units) are not re-derived between marking and Enter, and Enter refuses only for an ordinary OS error. Tool-managed removal (below) has no Trash and keeps its review-to-`Y` recheck of the facts you were shown.

### Tool-managed removal: mise versions and simulator runtimes

Installs that Trash would break are removed by their own manager, permanently, and only from the TUI. In the external view, Backspace on the mise installs row or a simulator runtimes row (the row says `removed by mise itself · Backspace`) opens that manager's own list: mise's installed versions (marked "mise reports prunable" where mise's own prune says so), or the simulator runtimes `simctl` lists with its size and last-use record. Nothing is read from a manager until that key press. Enter on an item reviews it:

1. The manager's list is read again, and swamp's refusals run (below).
2. The manager's **own dry run** runs with the exact command plus its dry-run flag: `mise -C / uninstall --dry-run <tool>@<version>` or `xcrun simctl runtime delete <UUID> --dry-run`.
3. Open files under what the dry run names are checked (`lsof`).

The confirm shows what is removed, "No Trash recovery: this cannot be undone", the exact command and the program path, the size (simctl's `sizeBytes`, swamp's stored measurement, or "not measured"), what reinstalling costs ("Reinstall is a download ..."), the open-file answer, mise's own reasons quoted (`mise says: "... is prunable: ..."`), which devices use a simulator runtime, warnings (always, for mise: "Versions pinned by environment variables in your shell are not visible to swamp."), and the dry run verbatim (control, bidi and zero-width characters stripped, bounded). **Enter never runs a removal.** `Y` does, and only after the confirm has been on screen, in full, for 1 second; input already queued when the confirm appears is dropped, and a paste is never read as keys. `Y` runs the whole review again and refuses if anything changed, including the dry run's text and mise's reasons; otherwise it runs exactly the command shown, reads the manager's list back and says plainly what it observed. Esc goes back and nothing runs.

Facts swamp shows as **warnings on the confirm** (you decide; `Y` re-reviews and refuses if any of them changed):

| Warning | Why it is a warning |
|---|---|
| A config requests the version (`mise ls` names a source); the global config is named as such; mise reports it active | `mise uninstall` does not check this itself, and after it goes mise in that directory reinstalls it or fails |
| mise's prune does not list the version | mise does not report it as unneeded; only prune knows the configs mise tracks, and `source: null` from one directory is not "nothing requests it" |
| A simulator on the runtime is anything but exactly `Shutdown` (Booted, Booting, Shutting Down, no state), or the device list could not be read; the runtime has no identifier, is not Ready/Unusable | `simctl` shuts running simulators down and deletes anyway; the warning names them |
| Files are held open, the open-file check could not finish, or simctl reports no mount path | there is no Trash to recover from; the warning names the process or says the check did not finish |

Refusals (a removal that would not be the removal you reviewed, or a manager that will not do it):

| Refusal | Why |
|---|---|
| mise's list has a field swamp cannot read, or lists the version twice and one entry cannot be read | an unreadable fact is not an absent one |
| It is a symlink install, its install dir (or a directory above it, up to `installs`) is a symlink, or it is not at `<data dir>/installs/<tool folder>/<version>` | removing it would go through a link or a directory nobody reviewed |
| The dry run exited non-zero, timed out, printed over 1 MiB, printed an error, a line swamp does not recognize, no dry-run marker, a configuration-links line, a path with `..`, or a path that is not exactly this version's install or cache directory | a text swamp cannot read is not a preview |
| simctl reports the runtime is not deletable | the tool itself refuses |
| Anything changed between the confirm and `Y`, including any warning that appeared, disappeared or changed (swamp reviews everything again when you press `Y`) | swamp runs only what that re-review still shows; a change in the milliseconds between that re-review and the command starting is not seen |
| The confirm does not fit the terminal, or was never drawn | `Y` runs nothing it has not shown in full |

A mise install folder or simulator runtime can also be marked with Space and moved to Trash like any other folder; the confirm says the manager will not know it is gone. Backspace on a row that is not marked opens the manager's own list.

The confirm names how many simulator devices use a runtime and which, and they stop working until you create them again. When simctl shows no Xcode Previews device set the confirm says other device sets are not visible to swamp. A manager version other than the one swamp was tested against (mise 2026.9.15, xcrun 72) is a warning on the confirm. The manager is resolved from a fixed list of directories (never `PATH`); a candidate that exists but is not owned by you or root, or is writable by others, refuses rather than falling through to another. A directory swamp runs a program from must be owned by you or root and not writable by everyone; it may be group-writable only for the macOS `admin` group, because admin members can already use sudo, so this grants no power they lack (standard Homebrew on Apple Silicon keeps `/opt/homebrew/bin` that way). Any other group, any other owner, or any group-write on Linux refuses, and the refusal names the group ("/opt/homebrew/bin is writable by group staff"). The program file itself must still be owned by you or root and not group- or world-writable. Its environment is built from nothing (`HOME`, a fixed `PATH`, `NO_COLOR=1`, `LC_ALL=C`, pagers off), from `/`. mise gets only its own directory settings (`MISE_DATA_DIR`, `MISE_CONFIG_DIR`, `MISE_CACHE_DIR`, `MISE_STATE_DIR`, `MISE_GLOBAL_CONFIG_FILE`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`), the same ones its detector and swamp's manager probe honor; xcrun gets `DEVELOPER_DIR` only when it is a real directory owned by you or root, not writable by others, holding a trusted `simctl` (otherwise the confirm says "DEVELOPER_DIR ignored"). Two swamp instances are not locked against each other for the removal itself: if both confirm the same removal, the second run finds nothing to remove (the manager does nothing, or errors). The ledger is locked: every write takes an advisory lock beside it (`ledger.lock`, waiting up to 10 s), so neither instance loses the other's row. The `started` row is written before anything moves or runs: a ledger that stays locked means it cannot be written, so nothing moves. The final row comes after the move, so if the ledger is locked then, the item is gone and only the `started` row remains to say what was about to happen (the result line says so plainly). Within one swamp, removals run one at a time. A removal still running is not stopped by Esc or Ctrl-C: a manager killed halfway can leave a half-removed install; after 10 minutes it is killed and reported as not finished, and never retried.

Not in this release: `brew uninstall` and `rustup toolchain uninstall` have no dry run, so swamp does not run them; `brew autoremove` waits for a captured real non-empty dry run; a bulk `mise prune --tools` removal is left out (its set is decided when it runs, and its whole path list cannot always be shown): remove the versions mise reports prunable one at a time.

| Unit | Removal and recovery |
|---|---|
| Filesystem path | Moved to Trash; swamp records its recovery location. Bytes remain on disk until the trashed data is removed. macOS: `~/.Trash`. Linux: the freedesktop Trash your file manager shows (`~/.local/share/Trash`, or the mount's own `.Trash-$uid`), with a `.trashinfo` record of the original path and deletion time, so the file manager's Restore works. A move that is not a rename on one filesystem is refused, never copied. |
| Cargo purpose group | The selected build output and its `.d`/`.dSYM` companions move together into one Trash envelope, with a `restore.json` manifest naming each member's original path. |
| Linked worktree or checkout | Moved to Trash; `git worktree prune`/`git worktree repair` follow for a linked worktree. Inspect warnings about local work and repository context first. |
| Docker image | Removed by Docker, permanently -- no Trash. Pulling or rebuilding depends on the image still being available or reproducible. |
| Docker volume | Removed by Docker, permanently. Swamp creates no copy of its contents. |
| Docker build-cache record | Reported, but individual removal is refused (Docker has no per-entry API for it). |
| mise tool version | Removed by mise (`mise -C / uninstall <tool>@<version>`), permanently -- no Trash. Reinstall with `mise install`. |
| Simulator runtime | Removed by simctl (`xcrun simctl runtime delete <UUID>`), permanently -- no Trash. Reinstall from Xcode > Settings > Components; whether Apple still offers the version is not checked. |

`--keep-executables` (a TUI toggle) copies supported Rust executables from `target/{release,debug}` and Python wheels/shared libraries from `dist` or `build` into the worktree's `bin/` before removal. It is not a backup of everything in the selected directory.

The ledger lives at `~/.local/share/swamp/ledger.parquet` (+ `ledger_facts.parquet`, the facts shown on the confirm line): one row per Trash move, naming the path, recovery location, bytes and time. Trashed bytes and freed disk space are different quantities -- moving to Trash does not free space until the Trash itself is emptied. Consult the reported recovery location for restoration; swamp has no general undo command.

The ledger lives at `~/.local/share/swamp/ledger.parquet`. Trashed bytes, permanent removals, and measured free-space change are different quantities. A tool-managed removal is one `tool-remove` row, written as `started` before the command runs and replaced by its outcome (so a removal cut short still leaves a row) (a swamp older than 0.8.0 reads it as an ordinary removal) with no recovery location; its facts name the manager and its version, the program path, the exact command and dry-run command, the dry run's digest and first 4 KB, the targets, the exit code, whether it timed out, and what the re-read observed (`completed`, `completed_with_error:<code>`, `failed:<code>`, `failed:exit 0, still listed`, `unknown:timed_out`, or `refused:<reason>` when `Y` refused). A ledger swamp cannot read is never overwritten: it is kept beside itself as `ledger.parquet.corrupt-<time>`, a new ledger starts, and the result line says so. Consult the reported recovery location for restoration; swamp has no general undo command.

## Decision evidence

Beyond size and growth, an artifact row, external unit, agent-storage
unit and nested build-artifact unit can carry `evidence`: a list of
sourced facts in five domains (#40's W2a/W2b), never a safety verdict.
Each fact states its own provenance and limits rather than a bare
value:

- **kind**: `activity`, `consumer`, `current-use`, `recovery`, or
  `reclaimability`.
- **status**: `known` (with a typed value), `unknown` (consulted, no
  answer), `unavailable` (the source itself could not be reached this
  pass -- distinct from "checked and found nothing"), or `conflicting`
  (two sources disagree; both are kept).
- **source**: what produced it (`filesystem-metadata`, `tool-reported`,
  `process-query`, `manager-lock`, `config-declaration`, `lockfile`,
  `build-metadata`, `docker-api`, `statvfs`, or `inferred`), with
  enough detail to judge how much to trust it.
- **observed_at** / **event_at**: when swamp looked, versus when the
  underlying thing happened -- a file's modification time observed
  today is not "modified today".
- **freshness**: an optional expiry (short-lived facts like an open-
  file check are rechecked at the next action boundary, never trusted
  indefinitely) and/or a stated coverage limit (e.g. "only measured
  children the folded walk recorded this pass").

`report --json` includes each row's `evidence` array in every view --
the default report view, `--view external`/`--view agents` (whole
structs), and `--view kinds`/`--view builds`/`--view deps`/
`--view unowned`/`--view worktrees`/`--view docker` (each row's own
evidence; a `kinds` bucket, which aggregates many rows into one, carries
the concatenation of all of them; a `worktrees` row carries its own
`Source` row's evidence). The interactive CLI text output
(`--view external`) prints one line per fact; so does the TUI's
selected-row detail area, ordered activity/consumer/current-use/
recovery/reclaimability so a narrow terminal shows the most
decision-relevant facts first, and the TUI's inline delete-confirmation
row adds a short warning for a declared consumer, current use, or an
uncertain recovery/reclaimability fact next to the existing git-status
warnings. Marking a row in the TUI snapshots its report-row evidence
plus a fresh current-use reading at that moment; the confirm banner
shows that reading as a fact, and Enter does not re-take it -- there is
no second check between marking and moving.

Tool-version declarations (#56) and dependency-lockfile/shared-store
associations (#57) are wired live into the report/external-unit
pipeline (`crates/core/src/consumer_wiring.rs`), not just implemented
as library code: a project's `.tool-versions`/`mise.toml`/
`.python-version`/`.ruby-version`/`.nvmrc`/`rust-toolchain(.toml)` is
read (cached per worktree, keyed by those files' own mtimes) and
matched against measured mise/asdf/pyenv/rbenv/rvm/nvm/rustup
installations; a project's `Cargo.lock`/`package-lock.json`/
`pnpm-lock.yaml`/`go.sum`/`gradle.lockfile`/`pom.xml` (cached the same
way) is joined against the Cargo registry/Go module cache/Gradle
caches/Maven local repository via one path-existence check per declared
dependency -- never an enumeration of the shared store. npm's cacache
and pnpm's content-addressed store cannot be matched to a specific
declared name+version, so they report that limit explicitly (`unknown`
for npm; a coarse "declared by this project's lockfile" fact for pnpm)
rather than guessing. A resolved match attaches a `consumer` fact both
ways: on the installation/shared-store unit (which project(s) declare
it) and on the declaring project's own `Source` row (which
installation it resolved to); rustup's `settings.toml` global default
gets its own distinct role, never folded into a project's declaration.

What each domain actually establishes:

| Domain | What it can show | What it cannot |
|---|---|---|
| Activity | Newest recorded modification among a unit's measured children (never "last used"); a tool's own reported use timestamp (Docker's `last_used`, a Cargo fingerprint), kept distinct from filesystem age; and, for external units, the separate **last-used** fact above | Whether a human intentionally used the content; access time when the mount suppresses `atime` (`noatime`/`relatime`, detected and reported `unavailable`) |
| Consumer | A declared reference: a version-manager pin, a dependency lockfile entry, a Docker join -- who *asks for* something; and, one tier down, a **recorded link** a tool wrote about its own output (an Xcode `WorkspacePath`), worded as such | Whether that reference was ever actually exercised at runtime |
| Current-use | A live, bounded, read-only check: an open file handle (`lsof`), a running Docker container, a simulator's booted state, a manager lock file's holder | Whether something not currently open/running/locked has no consumer at all -- absence here is not proof of no use |
| Recovery | A sourced restoration path (rebuild from present source, network-fetch from a named lockfile, local reinstall from a known version, or "potentially unique local state" for mutable environments) with named prerequisites and a concrete smallest useful follow-up check | Whether the network/registry/credentials needed at restore time are actually available -- always stated as a material unknown, never assumed |
| Reclaimability | Allocated bytes (always known), an estimated-reclaimable figure that is bounded rather than exact when hardlinks/APFS clones/snapshots are in play, and an observed post-action free-space change (`statvfs` before/after) | An exact reclaimed-byte guarantee from a scan alone; Trash, snapshots, open files and concurrent writers can all suppress the expected change |

Inventory of which artifact/detector domains have real activity evidence
today versus report unknown. This table is **generated** from
`activity::ACTIVITY_EVIDENCE_INVENTORY` and checked by
`crates/core/tests/evidence_contract.rs::the_usage_table_matches_the_activity_inventory`,
so the constant and the prose cannot drift apart. Edit the constant, not
the table.

<!-- BEGIN ACTIVITY_EVIDENCE_INVENTORY -->
| Domain | Activity evidence this pass can establish |
|---|---|
| filesystem artifact rows (build output, cache, dependency trees) | modification age (folded mtime_max); access time only where the mount does not suppress atime |
| Docker build-cache entries | daemon-reported last_used, kept distinct from filesystem mtime |
| Docker images/volumes | unknown: the daemon reports creation time and container references, not a last-used timestamp |
| Cargo nested build artifacts | fingerprint file mtime (tool-reported build time), where a .fingerprint entry exists |
| agent-tool session/category units | modification age of the session/category's own recorded mtime_max; no tool reports a distinct use timestamp |
| external location detectors (version managers, package caches, SDKs) | modification age of the measured directory; plus a separate last-used fact where a source is declared (tool record, else key-file access time, else none) |
<!-- END ACTIVITY_EVIDENCE_INVENTORY -->

`swamp protect add/remove` changes the keep-list; `protect list` reads it.
The TUI's mark step refuses protected paths for ordinary filesystem
artifacts as well as agent-storage units, showing a refusal reason.
Only an explicit, human-authorized command can add or remove a
protection; a scanned project file or an agent's own observation
cannot.

See `docs/architecture.md`'s "Decision evidence contract" section for
the implementation, and `skills/swamp/references/evidence.md` for the
same summary aimed at an agent reading the skill.

## Agent interface

Through v0.6.x, agent access went through a separate `swamp-mcp` stdio
server. That server is removed (#104): the CLI's `--json` output is now
the sole supported machine interface, and `skills/swamp/` packages it
as an installable agent skill. Use absolute root paths in commands.

### Installing the skill

Install with the [skills package](https://github.com/vercel-labs/skills):

```bash
npx skills add open-horizon-labs/swamp --skill swamp
```

Choose your agent in the installer. Installation is project-local by default;
add `--global` to make the skill available across projects. The package discovers
`skills/swamp/SKILL.md` and includes its references. This installs instructions,
not the binary: the skill's [installation reference](../skills/swamp/references/install.md)
explains platform detection, binary installation, and PATH verification.

Without Node/npm, copy or symlink the complete skill directory into your
agent client's skills location:

```bash
# Claude Code project-scoped skill, from a checkout of this repo:
mkdir -p .claude/skills
ln -s /absolute/path/to/swamp/skills/swamp .claude/skills/swamp

# Or copy it in (e.g. for a user-level skills directory some clients read):
cp -R /absolute/path/to/swamp/skills/swamp ~/.claude/skills/swamp
```

Consult your specific agent client's own documentation for where it
looks for skills; swamp does not assume or silently modify a client's
configuration file to register one. A release archive built from this
repository includes `skills/swamp/` alongside the `swamp` binary so
both ship together.

### The JSON contract

Build/dependency interiors include at most 30 detailed units per container
by default. Use `--unit-limit` and `--unit-offset` for those lists; their
`units_total` and `units_truncated` fields disclose omitted units. Family
summaries still cover the whole container. Ordinary `--limit`/`--offset`
control the outer result rows, or the flat Cargo `--view rust` list.

Every command below is noninteractive. With `--json`, results and
structured errors go to stdout; diagnostics go to stderr. A missing
observation returns `{"error":"no_observation","scope":...}` on
stdout with exit `2`. Clap argument errors instead return exit `2`
with usage/error text on stderr and no JSON stdout. Check both status
and body; nonzero does not imply empty stdout. Full schemas, the
historical MCP-tool-to-CLI mapping, pagination, and the exit-code
contract: `skills/swamp/references/commands-and-json.md`.

| Command | Main flags | Result |
|---|---|---|
| `report <root> --json` | `--project`, `--view`, `--filter`, `--dirs`, `--limit`/`--offset` | Full report or named view, bounded |
| `report <root> --view grown --json` | -- | Growing artifacts (from the last `observe --since` window) plus coverage/history information |
| `report <root> --view projects --json` | -- | Ranked project summaries |
| `report <root> --view worktrees --json` | `--filter` | Worktree and GitHub facts |
| `report <root> --view docker --json` | `--project`, `--unowned-only` | Docker objects and attribution |
| `report --view disk --json` | -- | The stored volume ledger: container, accounted, everything else, system volumes, not measured, named residual |

`report --json` is a pure read: it never records a new observation, never
shells out, and never re-derives GitHub/Docker facts -- run `swamp
observe` first. Result metadata differs by view;
do not assume every response includes the same history fields -- check
`skills/swamp/references/commands-and-json.md`.

Example calls, with an illustrative root:

```bash
swamp observe /Users/you/src --since 7d
swamp report /Users/you/src --view grown --json
swamp report /Users/you/src --view builds --json --filter 'type:rust size > 1GB'
```

There is no CLI command, for an agent or a human, that deletes or moves
anything -- `swamp report`'s views and `swamp protect` are the whole
CLI surface with any effect, and `protect` only ever changes a
keep-list. See
[the trust model](../skills/swamp/references/trust-model.md) for the
full statement of what swamp is now and what actually removes data (the
TUI's own Trash move, or a human's own shell command).

## Configuration

```bash
swamp config show
swamp config path
swamp config init
swamp config list                 # every key `set` writes, its effective value and meaning
swamp config get hf-enrich
swamp config set hf-enrich on     # one key; other keys, tables and comments are kept
```

`config set` writes one top-level key, with `-` and `_` spelled either way.
An unknown key is refused with the list of valid ones; a value outside the
range the code honours is refused with that range (never clamped); nothing
is written then. The file is edited in place (comments, other keys and
`[scan]` stay as written) and replaced atomically.

| Key | Values | What it does |
|---|---|---|
| `since` | 1 to 31622400 seconds (as 24h, 7d, 30m) | how far back growth is measured by default |
| `retention_days` | 1 to 3650 days | days of history kept in the store |
| `large_file_min_bytes` | 1 to 9223372036854775807 bytes | files at least this large are tracked individually |
| `observe_timeout_sec` | 60 to 86400 seconds | watchdog budget for one observe |
| `observe_stall_secs` | 30 to 86400 seconds | stop an observe with no progress for this long |
| `min_free_bytes` | 0 to 9223372036854775807 bytes (0 disables; unset: the default) | refuse to observe below this much free space |
| `volume_pass_interval_hours` | 0 to 8760 hours (0: only `observe --volume`) | hours between volume passes |
| `volume_pass_budget_secs` | 5 to 86400 seconds | seconds one volume-pass run may measure |
| `hf_enrich` | on, off | ask huggingface.co about each hub repo during scheduled observes (off by default) |

`[scan]` roots are `add-root` / `remove-root`.

`config init` writes a file only if none exists. Defaults:

```toml
since = "24h"
retention_days = 30
large_file_min_bytes = 1048576
observe_timeout_sec = 1800
observe_stall_secs = 300
# min_free_bytes = 1073741824   # unset: the greater of 1 GiB and 1% of the volume; 0 disables
volume_pass_interval_hours = 24 # a plain `observe` runs the volume pass when the last is older; 0 = only `observe --volume`
volume_pass_budget_secs = 120   # one run of the volume pass measures for at most this long
hf_enrich = false               # true: a scheduled or CLI observe asks huggingface.co about each hub repo (see "Model caches")

[scan]
defaults = true
include = []
exclude = []
disabled_detectors = []
enabled_detectors = []
```

### Stuck observations

`observe_timeout_sec` bounds a whole `swamp observe`. Separately, every
step that touches your files (a walk directory, a repository's git
signals, its ignore rules, an agent session store, an external unit)
reports progress. When nothing has progressed for `observe_stall_secs`
(default 300, minimum 30), the pass is stopped: the writer lock is
released and `observe.log` records `timeout(stuck <N>s in <phase> at <path>)`,
so the next observation is not left waiting behind it. That path is then
skipped for 24 hours and reported as `not measured (stalled on <date>)`
(kept in `stalled-paths.tsv` in the store, which a store reset keeps), so
one blocking path cannot fail every scheduled pass.

Files are never opened when opening them could block. A FIFO or device
is refused by `stat` alone (opening even the read end of a FIFO would
wake a program waiting to write to it). A dataless file provider
placeholder (an iCloud Drive or CloudStorage item whose contents are not
downloaded) is neither opened nor listed; such a directory is reported as
not measured. Before git reads a repository, its git directory is swept
once (loose objects and packs skipped); a repository with a FIFO, device
or dataless file there, or with more than 50,000 entries to check, is
reported as not measured. When your global git config could block, git
reads repositories without it.

### Programs swamp runs, and from where

swamp runs a fixed set of programs, each from a fixed list of locations, **never through `PATH`**: a `git`, `docker` or `brew` placed earlier on your `PATH` (a shim, a wrapper, a checkout's `bin`) is never what swamp runs. The first location that exists is the one used, and it is checked: the program file (a symlink such as Homebrew's is followed to the real file) must be an executable owned by you or root and not group- or world-writable, and so must every directory on the way to it: the location's own directory, the directory of every link in between, and the real file's. A directory swamp runs a program from must be owned by you or root and not writable by everyone; it may be group-writable only for the macOS `admin` group, because admin members can already use sudo, so this grants no power they lack. Any other group, another owner, or any group-write on Linux refuses, and the refusal names what failed ("/opt/homebrew/bin is writable by group staff"); swamp never falls through to a later location. A program found nowhere is "not available", the same as not installed. Locations in your home (`~/.local/bin`, `~/.cargo/bin`, `~/.docker/bin`, `~/.orbstack/bin`) come after every system location and are used only when none of those has the program: any process you run can put a file there, so they are trusted about as much as your own `PATH`. `brew`, `mise` and `curl` run in an environment built from scratch (`curl`, only for the opt-in Hugging Face lookup, additionally gets exactly `HTTPS_PROXY`, `https_proxy`, `ALL_PROXY`, `all_proxy`, `NO_PROXY`, `no_proxy`, `SSL_CERT_FILE` and `CURL_CA_BUNDLE` when set, so a machine behind a proxy works; never `HTTP_PROXY`, `CURL_HOME` or any token); the others keep your environment, because `git`, `gh` and `docker` need your credentials and contexts, except loader and hook variables (`DYLD_*`, `LD_*`, `GIT_CONFIG*`, `GIT_SSH*`, `GIT_EXEC_PATH`, `GIT_EXTERNAL_DIFF`, askpass, editor and browser variables), and with `PATH` set to the program's own directory, `/usr/bin:/bin:/usr/sbin:/sbin`, then `/opt/homebrew/bin` and `/usr/local/bin` if they pass the same directory check.

| Program | macOS | Linux |
|---|---|---|
| `git`, `gh` | `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin` | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/home/linuxbrew/.linuxbrew/bin` |
| `docker` | `/usr/local/bin`, `/opt/homebrew/bin`, `/usr/bin`, `~/.docker/bin`, `~/.orbstack/bin` | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/home/linuxbrew/.linuxbrew/bin`, `~/.docker/bin`, `~/.orbstack/bin` |
| `brew` | `/opt/homebrew/bin`, `/usr/local/bin` | `/home/linuxbrew/.linuxbrew/bin`, `/usr/local/bin`, `/usr/bin` |
| `mise` | `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin` | `/usr/local/bin`, `/usr/bin`, `/home/linuxbrew/.linuxbrew/bin`, `~/.local/bin`, `~/.cargo/bin` |
| `lsof` | `/usr/sbin` | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin` |
| `du`, `id` | `/usr/bin` | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin` |
| `df` | `/bin` | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin` |
| `systemctl`, `loginctl` | (not used) | `/usr/bin`, `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin` |
| `xcrun`, `plutil`, `defaults`, `tmutil` | `/usr/bin` | (not used) |
| `diskutil` | `/usr/sbin` | (not used) |
| `launchctl` | `/bin` | (not used) |
| `curl` | `/usr/bin` | `/usr/bin` |

`~` locations are used only when `HOME` is an absolute path.

### Full-disk guard

Before any walking, `swamp observe` does one `statfs` on the volume that
holds the swamp store. If free space is below `min_free_bytes` (unset:
the greater of 1 GiB and 1% of the volume; `0` disables the check; TOML
integers top out at 2^63-1), it exits with **code 3**, printing the free
bytes, the threshold, the volume's store path and that nothing was
written. It runs before the scope is persisted and before the writer lock
is taken, so an aborted run changes no store file and no coverage fact
(roots are never marked missing because of it). The scheduled LaunchAgent
run logs that one line and exits; there is no retry loop. `swamp ui` does
not wait on it: it shows the last stored report at once with a
`disk nearly full: refresh skipped (X free)` banner, and starts no
refresh. With no stored report yet, it exits with the same
message instead.

`config init`'s `[scan]` table is not a frozen copy of the built-in
default roots or the detector catalog -- it documents the five keys
with their meaning; the actual defaults and detector catalog live in
the binary and can grow across releases without editing every user's
config. See [Scope and coverage](#scope-and-coverage) for what each key
does and `swamp scope --json` for the resolved result. `config show`
and `config init` both refuse (nonzero exit, message on stderr) on a
`config.toml` with a malformed `[scan]` table, rather than silently
falling back to the all-defaults scope.

The file is `~/.local/share/swamp/config.toml`. `SWAMP_DIR` changes the store directory; give the CLI and UI the same value (interactively or from an agent's `--json` calls) to share history. On Linux `$XDG_DATA_HOME` moves the store if it is set to an absolute path, and the schedule log defaults to `$XDG_STATE_HOME/swamp/observe.log` (`~/.local/state/swamp/observe.log`); on macOS the log defaults to `~/Library/Logs/swamp/observe.log`. `SWAMP_LOG_DIR` overrides the log directory on both. The log is capped: when `observe.log` reaches 1 MiB it becomes `observe.log.1`, older ones shift to `.2` and `.3` (the oldest is dropped) and a new log starts, so it takes at most about 4 MiB. A log that cannot be rotated or written is reported once on stderr and never fails the observation. If `HOME` is unset and `SWAMP_DIR` is not given, swamp fails with a message rather than writing the store into the current directory. The observation timeout applies to `observe`, not every interactive operation.

`swamp scope` (text and `--json`) reports which platform's conventions
produced its roots, so a scope read on the other machine is not just a
list of missing paths. Detectors that do not apply to the running platform are
listed rather than omitted -- under *not applicable on this platform* in
text, as `not_applicable_detectors` in JSON, each with the platforms it
does apply to -- so "does not apply here" is never confused with "found
nothing" or "failed".

For a trace of stage timings, work counters (directories listed, files
statted, cache hits/misses), and -- per external-unit candidate --
whether its container cache allowed reuse and how many directories/
files that one unit cost (`swamp observe` is the only command that
scans; `report` is a pure read and has nothing to trace):

```bash
SWAMP_TRACE=1 swamp observe
```

See [architecture](architecture.md) for the meaning of incremental updates, history retention, and cached enrichment.
