# Changelog

Release notes describe behavior at the named version. Older timings are individual
observations, not general performance guarantees. See the README for current use.

## v0.8.0

- **An unchanged scheduled observe does less work (#181).** Measured on a copy of a real store (89 external units, 6,150 agent units, 42 projects) at the LaunchAgent's background priority, five unchanged passes each: median 42.7 s before, 31.3 s after (process time 47.6 s to 32.0 s; files stat'ed per pass about 750,000 to 360,000). What changed: a busy unit such as `~/Library/Caches` (163,000 files) is no longer walked whole when something under it changes; only the subfolders with events are walked and the rest are replayed from their stored totals, with the same total, newest-modified time and drilldown a full walk gives (a folder renamed into place, a hard link across folders, or an unreadable folder still walks what it must). `~/Library/Caches` and `~/Library/Developer` now have their own change cursor (they had none, so every pass walked them). One folder FSEvents cannot itemize no longer makes every location on the disk re-walk, including the 1.77 million files of simulator volumes (19 s, 132 s at background priority); only the locations that contain it are walked. Homebrew's own reports are reused while its Cellar and Caskroom measure the same (up to 24 h), saving two `brew` runs a pass; a warm pass no longer runs `gh auth status`. Still asked every pass, by design: Docker (no change key swamp can read), mise (reads configuration outside its folders). A pass whose event history was dropped by the system still walks everything (measured 108 s), and says so in coverage.
- **Tests no longer write to your real log.** The CLI tests appended 3,354 fake `projects=0` outcome lines to `~/Library/Logs/swamp/observe.log` between Sep 19 and Oct 1 (they set the store directory but not the log directory). Every test run of swamp now refuses a store, log or LaunchAgents path outside the temp directory, and never runs the real `gh`. Existing polluted lines are left in place: they are the lines with `projects=0`..`3` and `walked_total` under 10 MB.
- **`observe.log` is capped.** At 1 MiB it becomes `observe.log.1` and a new log starts, so the log takes at most about 2 MiB (it had reached 2.1 MB in two weeks). A test simulating six months of observations confirms the store's history stays bounded by its 30-day retention.
- **`swamp config set <key> <value>`, `config get <key>`, `config list`.** Editing `config.toml` by hand was the only way to change a setting. `set` writes one top-level key (`since`, `retention_days`, `min_free_bytes` or `unset`, `hf_enrich`, ...; `hf-enrich` and `hf_enrich` both work), refuses an unknown key with the list of valid ones and a value of the wrong kind before writing anything, keeps every other key, table and comment as written, and replaces the file atomically. `list` shows each key's effective value and what it does.
- **Hugging Face and Ollama models are listed one model at a time, with what each one is.** The hub cache (968.9 MB here) and `~/.ollama/models` (522.7 MB) were one opaque row each, labelled "cannot be regenerated (local state or models)", which is wrong for a downloaded model. Each repo or `model:tag` is now its own row in Reclaim and External, with a one-line "what it is" from its own files (model-card front matter, `config.json`, the safetensors or GGUF header, Ollama's config blob; a field none of them states is absent): on this machine `mkrausio/EmoWhisper-AnS-Small-v0.1@e613edc6` is "whisper · 241.7M params · float32" (the exact count, 241,734,912, from the safetensors header), and `qwen3:0.6b` is "qwen3 · 751.63M params · Q4_K_M". Each row has its revision and refs, its size (blobs counted once; the two stores measure 968,941,568 and 522,678,272 bytes, equal to `du -sk`), when its largest weight file was last read (file access time, labelled, with swamp's own header read set aside), and how it comes back: "downloaded again from huggingface.co (<repo>@<revision>) when needed" or "`ollama pull qwen3:0.6b`"; a repo with no ref keeps "cannot be regenerated (no source recorded)". The confirm and the detail pane say what a move leaves behind ("moving this folder frees about 1.9 MB; 967.0 MB stays in the hub's shared blobs/"; an Ollama manifest's layers stay in `blobs/`, and `ollama rm` is Ollama's own removal); Ollama tags are their own rows in the TUI's Reclaim and External views. A parameter count is one copy of the weights (the set `model.safetensors.index.json` names, else one shard set), never a sum across Mistral's `consolidated.safetensors` and its shards or a partial shard set; no folder of the cache that is a symlink is listed, read or counted, and files are opened with `O_NOFOLLOW`. Swamp does not read a weight file whose access time its read would set, so the last-read date is never swamp's own. Other facts on the row, incomplete downloads, links that point at nothing or leave the cache, a ref with no snapshot (the `laion/Empathic-Insight-Voice-Small` folder here), and blobs "not referenced by any manifest". Every content read is cached by content identity in the store, so a revision is parsed once: the cold pass here read 67,043 bytes of headers in 4.6 ms (hub) and 1,348 bytes in 0.7 ms (Ollama), the warm pass 0 bytes; at most 64 new parses per observe. Optional, off by default: `swamp config set hf-enrich on` lets a scheduled or CLI observe ask huggingface.co's public API once per repo (`/usr/bin/curl` with only the https proxy, no-proxy and CA-bundle variables, no token sent: a gated repo shows as "did not answer", at most 16 requests per pass, revision facts never fetched again, counts after 7 days, failures after a day), shown as "from huggingface.co, fetched <date>". The TUI and `report` never fetch or read model files.
- **Worktrees without a pull request now say whether their commit is in another branch.** `swamp report --view worktrees` printed `merged=unknown, tip_reachable=unknown` for every branch with no GitHub PR (audit branches, work merged into an integration branch). `tip_reachable` is now its own fact, computed offline from the repository's refs: is the worktree's HEAD contained in a remote-tracking branch or the default branch, with the branch named (`tip_reachable=yes (origin/audit/x)`). `merged` is still the PR fact. Closes #208.
- **The TUI key legend is never cut mid-hint.** At some widths the bottom row ended `q ` with no word. Hints are now dropped whole from the right, counting wide glyphs conservatively. Closes #210.
- **swamp never runs a program found through `PATH`.** Every program it runs (`git`, `gh`, `docker`, `lsof`, `du`, `df`, `diskutil`, `tmutil` and the rest) now comes from a fixed list of locations per platform, checked like the package managers already were; before, all but a few were looked up by name on `PATH`, so a shim earlier on it ran instead. A directory swamp runs a program from must be owned by you or root and not writable by everyone; it may be group-writable only for the macOS `admin` group, because admin members can already use sudo, so this grants no power they lack. A program at none of its locations is "not available". The lists are in the usage guide. `git`, `gh` and `docker` keep your credentials and contexts but not loader or hook variables (`DYLD_*`, `LD_*`, `GIT_CONFIG*`, `GIT_SSH*`, `GIT_EXEC_PATH` and the like), and get a fixed `PATH`.
- **A running observation's lock is no longer taken from under it on a minimal Linux.** Whether the lock holder is alive was asked by running `kill`, which a minimal image may not have; then a live holder read as dead and a second observation reclaimed its lock. It is now a direct check that starts nothing.
- **What you can see in the TUI, you can move to Trash.** Reclaim, External and Disk rows, and the folders listed under them, could not be marked ("view only", "nothing to delete on this row"); you could open `~/Library/Caches` (36.8 GB) and its `hiphi-endpoints` (19.5 GB) and delete neither. Every unit, every listed folder, every store-interior folder, every measured folder in the Disk views, every build folder no cleanup rule covers, and every config or credentials file an AI tool keeps can now be marked with Space and moved to Trash with Backspace then Enter (Esc cancels, nothing else acts while the confirm is open). Swamp no longer decides for you: local state, models, installations, the whole Caches folder, Claude session scratch, a path outside your home, a version a mise config requests, a simulator that is not shut down, and a file held open are **lines on the confirm**, each a plain fact ("cannot be regenerated", "regeneration cost not established", "last used: no record", "bytes are a lower bound", "in use right now: ...", "could not be checked", "the tool will still list it"), never a refusal. The confirm lists every exact path and size and that Trash is the way back, and Enter is offered only when the whole plan fits the screen. It refuses only for: a path that is not a real deletable folder or file, an OS refusal (with the OS error), a mark that changed since you made it (a symlink swapped in, a different folder renamed into place, a path that now resolves elsewhere; nothing moves), a ledger that cannot be written (a `started` row is written before the move), an overlap with another mark, your own `swamp protect` mark (`protected by you ...; swamp protect remove <entry>`, naming the entry that covers the row), and swamp's own ledger or Trash (a folder that holds either cannot be moved by swamp, because the move is recorded there). The confirm also says what a folder contains (units, your whole Library, the system temp folder, Homebrew's prefix), mounted volumes inside it (moving it frees about nothing), git checkouts, hard links and a Trash on another volume, refuses if what holds it open changed since you looked, records a failed move as `failed:` with the OS error, and shows control and bidi characters in folder names escaped. A protect list that cannot be read no longer blocks every mark: the confirm says your keep marks were not checked. `A` marks each top-level unit once; folders, paths no cleanup rule covers, and what swamp keeps by default are marked one at a time. A moved folder leaves the screen and comes off its unit's size at once. Numbers: Trash frees space only when emptied, and the result line says so and that sizes are from the last observation. Fixes found on the way: the review worker had no store, so the protect check was silently skipped for ordinary rows marked with Space or Backspace; it now carries the store, store interiors and agent units. The same rule reaches build adapters (a shared store, installation or unknown layout, final outputs: "inspection only" is now "no cleanup rule", and the reason is on the confirm), AI-tool storage (credentials, settings and skills are kept by default: `A` leaves them out, Space marks one and says what the tool loses; database files warn instead of refusing), and mise and simulator removal (advisory refusals became warnings; `Y` still runs the manager's own command only after a full re-review shows the same facts, and Space on the same row offers Trash). The agent and JSON plan path is unchanged: it still refuses what an AI tool must not decide. Docs, help, guardrails (`protection-fails-closed`, `tool-removal-refuses-on-manager-facts` and the three retired guardrails' exception paragraphs) say the same.
- **Homebrew works on a standard Mac again.** The program resolver refused `brew` because `/opt/homebrew/bin` is writable by the `admin` group, so Homebrew detection and the brew manager pass never ran on most Macs. A directory swamp runs a program from must be owned by you or root and not writable by everyone; it may be group-writable only for the macOS `admin` group, because admin members can already use sudo, so this grants no power they lack (standard Homebrew on Apple Silicon keeps `/opt/homebrew/bin` that way). Any other group, any other owner, or any group-write on Linux refuses, and the refusal names the group ("/opt/homebrew/bin is writable by group staff"). The program file itself must still be owned by you or root and not group- or world-writable.
- **Fresh files no longer read as near-empty, and one unreadable entry no longer collapses a directory (Linux).**
  On Docker's overlayfs a 5 MB tree written a moment earlier measured 204,800 bytes,
  then 5.97 MB after the kernel flushed it. Recent files that report almost no blocks
  are still counted as the filesystem reports them but are now flagged as pending,
  with the most they can still add. Entries
  that vanish mid-walk are skipped; other per-entry errors mark only that entry as
  not measured. Closes #196, #197.
- **A FIFO in a git checkout no longer hangs `swamp observe` forever.** A FIFO
  named `.git/config`, `.git/HEAD` or `.git` parked one walk worker in
  `open(2)` while the rest idled at 0% CPU, holding the writer lock; the
  same pass now finishes in about 2 s. A FIFO or device is refused by
  `stat` alone, never opened (which would wake a program waiting to write
  to it). A repository whose git directory holds one (loose refs, reflogs,
  `info/exclude`, alternates), a FIFO `.gitignore`, a FIFO Codex database
  sidecar, or a FIFO global git config no longer parks the pass either.
  Dataless iCloud/CloudStorage placeholders are neither opened nor listed
  (reported as not measured).
- **A stalled observation stops, says where, and is not repeated.** New
  `observe_stall_secs` (default 300, minimum 30): when no step has made
  progress for that long, `observe` stops, releases the writer lock and logs
  `timeout(stuck <N>s in <phase> at <path>)`, instead of holding the lock for
  the full 30-minute `observe_timeout_sec`. That path is skipped as
  `not measured (stalled on <date>)` for 24 hours, so one blocking path cannot
  fail every scheduled pass.
- **The first line of `swamp report`, the TUI and the Reclaim view is now "Developer
  storage: X across N projects and M tool locations (P% of used)".** It replaces headlining ~/src alone
  (16 GB on the reporting machine, while developer storage was about 224 GB). It
  counts the source roots (less standalone Cargo targets, which get their own row), every catalog unit
  (toolchains and SDKs, caches, agent storage, containers and VMs, other), and never
  system volumes, the measured "everything else", Homebrew's remainder unit or
  mounted disk images. The percent is of the container's used bytes from the disk
  ledger (never the Data volume), rounded down to one decimal, and is absent, with the
  reason, when there is no ledger, when it is unreadable, from a newer swamp or dated
  in the future, when used is zero or missing, or when developer storage exceeds
  used (a FLAG instead of a percent above 100). The breakdown rows add up to the
  headline exactly; below them: everything else (five largest), system volumes, not
  measured with its count, the walk's spot-audit warning when it disagrees, and the ages of
  the observation and the ledger. `report --json` has the same numbers as a `headline`
  object, and the Reclaim JSON adds `headline_relation`. This also puts the "System
  volumes" line (#170) in the TUI. Reading it starts nothing and lists nothing.
  It also fixes the volume ledger losing a folder's bytes when an agent tool's home and a
  catalog unit share a path (~/.codex, 6.8 GB on the reporting machine, was counted in no row).
- **BREAKING (TUI keys): views are nested in three sections.** In 0.7.x the digits `1`-`9`
  selected views; now `1` Projects, `2` Tools, `3` Disk select *sections*, `Tab` /
  `Shift-Tab` move between them, and `v` cycles the views inside the current one
  (Projects: Projects, Tree, Builds, Deps, Types, Kinds, Unowned; Tools: Reclaim, Docker,
  External, Agents; Disk: Summary, Not measured). Digits 4-9 are unbound. A row under the
  headline names the sections (current one in reverse video) and the view line reads
  `view: Tools › Reclaim (1 of 4 · v next)`. Disk is new: Summary is the stored volume
  ledger as rows, Not measured lists the unreadable and not-yet-measured folders. The
  legend is `Tab section  v view  / filter  R refresh  ⌫ delete ...`; `?` help lists every
  section and view. The headline block points at Reclaim and Disk (`2 for Tools`, `3`),
  and a store that has never opened Tools or Disk shows one line saying so until it does
  (`ui_state.json` `views_seen`). The block is four rows plus the strip, so on a 24-row
  screen the table area (its heading included) is 14 rows where it was 19.
- **mise versions and simulator runtimes can be removed through their own manager, from
  the TUI.** Trash would break these installs, so they had no removal at all. In the
  external view, Backspace on the mise installs row or a simulator runtimes row shows the
  manager's own list; Enter on one runs the manager's own dry run and shows a confirm with
  the exact command, the dry run verbatim, the size (simctl's `sizeBytes`, 8.4 GB for
  iOS 26.2 here, or "not measured"), what reinstalling costs, and "No Trash recovery: this
  cannot be undone". `Y` on the confirm (never Enter, and only after it has been on screen
  for a second) checks everything again and runs exactly that command:
  `mise -C / uninstall <tool>@<version>` or `xcrun simctl runtime delete <UUID>`. swamp refuses, with the reason and the next step,
  a version a config requests (mise's own dry run does not check this for the global
  config), a version mise's prune does not list, a simulator that is not shut down (simctl would
  shut it down and delete anyway), files held open or an open-file check that could not finish,
  a dry run it cannot read, and anything that changed since the confirm. Devices on a
  runtime are named on the confirm. Each removal is one `tool-remove` ledger row with the
  command, the manager's version and what the re-read showed; there is no CLI for it.
  `brew uninstall` and `rustup toolchain uninstall` have no dry run and stay facts only.
- **`swamp report --view reclaim`, and a Reclaim view in the TUI (`v`).** One row per
  unit of developer storage, largest first, with what getting it back costs in the
  tool's own words, when it was last used and from what record, who is known to
  need it (declared, and recorded by the tool), what Homebrew or mise itself
  reports (quoted and attributed, never swamp's verdict), and which removal path
  exists. Every listing says what its consumer evidence was checked against and
  when that is incomplete; a rustup default toolchain, a mise global tool and a
  formula installed on request are marked and held out of the regenerable total,
  and `unknown` when the manager's record could not be read. A scheduled `observe`
  asks the managers two read-only questions each (dry runs only, from fixed program paths, a scrubbed environment and one fixed directory) into a new
  `manager_facts.parquet` that older versions ignore; `report` and the TUI start
  no process.
- **Each unit says when it was last run or opened, and where that comes from.**
  `swamp report --view external` (and `--json`, and the TUI's selected-row detail)
  now shows `Last run or opened: Jul 8 (file access time)`, `... Sep 6 (Xcode
  DerivedData record)` or `... no record`, never "unused". A tool's own record wins:
  Xcode DerivedData's `LastAccessedDate` (on this machine, the one project's Sep 6)
  and Cargo's `~/.cargo/.global-cache` (opened read-only, per subtree). Otherwise the
  access time of the files directly in a `bin` folder (rustup toolchains, mise
  installs, pyenv versions, Homebrew Cellar, Android SDK packages, ESP-IDF tools),
  never a directory's own time and never a symlink's; on this machine rustup showed
  Sep 25 for 1.90 against Sep 30 for stable. No signal is `no record`, never a date
  taken from a modification time. The docs list which source each kind uses, which
  were checked here, and the ways access time misleads (backups, antivirus,
  indexers, `--version`). It is read fresh on every observation, opens no file, and
  `swamp report` only reads the stored value.
- **`~/Library/Caches` and the other big roots are no longer one number.**
  `~/Library/Caches` (36.9 GB, one row) now lists its top 15 child folders with size,
  modification time and last-used, plus a remainder row, so the rows add up to the
  total exactly (it needs one signed adjustment row for hardlinks counted once, -3.7
  MB here). A folder that cannot be listed says `not measured`, never `0B`. Toolchain
  roots (rustup, mise, pyenv, Cellar, ESP-IDF, Android packages), DerivedData and the
  Cargo stores list their children the same way. Names, sizes and dates only.
- **Standalone Cargo target directories are their own kind.** A directory that
  `CARGO_TARGET_DIR` built into (Cargo's `CACHEDIR.TAG` signature and
  `.rustc_info.json`), inside a root you declared, shows as `standalone-cargo-target`
  with its size, age and "rebuild with `cargo build`" instead of unowned residual, and
  can be planned for the Trash with the usual in-use reading. They have their own section
  in `--view external`, `--json` and the TUI's External view. A `target/` beside a
  `Cargo.toml` is not called standalone, and when a target's dep-info records absolute
  source paths (one of the four here does) they show as a labelled recorded link, never
  as a selector. Declared over `/private/tmp`
  here it found the four from the report (6.65, 1.66, 1.25 and 1.11 GB, matching `du`) and
  every other worker's `CARGO_TARGET_DIR`, 53 GB in 24 directories. A directory with only the
  tag (pytest, uv) is not called Cargo's, and a project's own `target/` is still counted
  once, under the project.
- **The overlap note is data now.** The bytes a unit holds that are counted under
  project worktrees are two fields (`bytes_counted_elsewhere`, `overlap_count`) in
  `--json` and the store; the sentence is rendered from them. This is a store-format
  change: the first `swamp observe` after upgrading rebuilds the derived tables
  (configuration, protection, notes and the ledger are kept).
- **Upgrading does not reset your store, and two installs can share it.** The store
  format stays as v0.7.5 has it. Everything v0.8.0 adds lives in new tables that v0.7.5
  ignores, and `external_units` keeps its v0.7.5 columns. Checked with the real v0.7.5
  binary in both directions on a copy of this machine's store: neither reset it, lost data
  or printed `no_observation`. A v0.8.0 that meets a store written by a newer swamp reads
  it and refuses to modify it ("store written by a newer swamp; not modifying it")
  instead of resetting it.
- **An adapter change under the same catalog version no longer replays old rows.** The
  stored identification of build stores now carries a digest of the adapters' source,
  so developers no longer need `swamp observe --full` after editing an adapter.
- **Homebrew's dev-tool formulas show last-used too** (llvm, zig, cmake, dotnet and the
  rest of the default list), read from each formula's `bin/`. A date in the future (a
  tracker in milliseconds, a 2099 plist) is set aside, not shown. Registering a worktree
  inside a unit no longer shows as the unit shrinking: growth is not shown across that
  coverage change.
- **Xcode's `WorkspacePath` is worded as what it is.** It was listed as a declared
  consumer; it is a link Xcode recorded about its own output, so it shows as a
  recorded link.

- **ESP-IDF's tool directory is reported.** `~/.espressif` (or `IDF_TOOLS_PATH`) was
  8.3 GB here and invisible. `swamp report --view external` now shows `tools/`,
  `dist/` (downloaded archives) and `python_env/` as separate rows, each with what
  reinstalling costs in Espressif's own words (`install.sh`). A machine without it
  lists the location as missing.
- **The Android SDK's big folders are reported.** Only `platforms/`, `system-images/`,
  `build-tools/` and `emulator/` were measured, and on this machine they held almost
  nothing while `ndk/` held 5.9 GB. `ndk/`, `cmdline-tools/`, `platform-tools/` and
  `cmake/` now appear, each package with its `sdkmanager` reinstall command. An NDK
  that `ANDROID_NDK_HOME` or `ANDROID_NDK_ROOT` names outside the SDK is measured too.
  `licenses/` stays out. An SDK with no NDK shows no NDK row and no error.
- **The rest of `/Library/Developer` is reported.** Besides the simulator runtime
  volumes (still counted once, through `CoreSimulator/Volumes`, never also through
  the `AssetsV2` disk images behind them): `CoreSimulator/Caches`, `Images`,
  `Cryptex` and `Profiles`, `CommandLineTools` (reinstall with
  `xcode-select --install`), `DeveloperDiskImages`, `CoreDevice` and `DeviceKit`.
- **Claude Code's session scratch is reported.** `/private/tmp/claude-<uid>` was
  several GB here and unmeasured. It shows as a cache (rows per project directory
  under "Caches & intermediates"), with swamp's own note that removing it during a session
  breaks that session. It is one exact path; nothing else in `/private/tmp` is scanned.
  Git worktrees that live inside it are counted under their projects, and the row says how
  much. On this machine every entry there was newer than the last boot; swamp does not
  assume it is cleared. The simulator runtime row is labelled as mounted size: runtimes
  that are not mounted are not measured.
- **You declare where your source code is; swamp never guesses.** `swamp config add-root
  <path>` and `remove-root <path>` edit `[scan] include` in your `config.toml` in place:
  comments, key order and other settings stay, the write is atomic, and two calls at once
  both land. A path that does not exist, is a file, is unreadable, or sits inside a root
  you already declared is refused with the reason; a root you already declared is a no-op
  however it is spelled (trailing slash, `..`, a symlink, `~`). `--allow-missing` records
  a root that is not mounted yet. On a first run (nothing observed yet, no `[scan]`
  section), an interactive `swamp ui` or `swamp observe` asks `Where is your source code?
  Press Enter for ~/src` (offering `~/src` only if it exists); without a terminal it never
  waits and prints `swamp config add-root <path>` instead. `swamp scope`, `swamp config
  show` and `swamp report` list each declared root as present (with its bytes from the
  last stored observation), missing or unreadable. A source check now fails the build if
  any code reads shell history, editor recent projects, git configuration or Spotlight to
  propose a root. Replaces the hard-coded `~/src` as the only way to say where your code is.
- **Homebrew counts developer tooling by default, and shows the rest as one line.** It was
  off by default because `/opt/homebrew` also holds GUI apps. Now the language toolchains
  and build tools on an allowlist (`llvm@20` and `llvm@21` both match `llvm`; `gopls` does
  not match `go`), the developer casks (`android-platform-tools`, `android-studio`) and
  Homebrew's Android command-line tools are units of their own, and everything else under
  the prefix is one `Homebrew (other)` unit, so the parts add up to the prefix. The Android
  bytes are counted once. `[scan] enabled_detectors = ["homebrew"]` still reports Cellar
  and Caskroom whole. Ambiguous tools (qemu, ansible, pandoc, duckdb, mlx) stay in
  `Homebrew (other)`, shown with the setting that reports it whole and no recovery
  hint (it is Homebrew itself, `bin`, `lib` and the GUI casks). A file hardlinked between a
  dev keg and the rest of the prefix is counted once, in the dev unit. Removal of a dev
  unit, if you decide to, is `brew reinstall <formula>`.
  **Upgrade note:** `disabled_detectors = ["homebrew"]` still turns off all Homebrew
  reporting (it now names the family of three); `enabled_detectors = ["homebrew"]` still
  means the full detector.
- **Changing your roots no longer hides your last observation.** The stored observation is
  now keyed by your project roots, not by every detector location, so an upgrade that adds
  a detector, or `swamp config add-root`, no longer makes `swamp report` say "no observation
  yet" or `swamp ui` start a scan. When the roots did change, the most recent observation
  that overlaps the new scope is shown, labelled `showing the previous scope (N roots); new
  roots not yet observed; press R`, from stored tables with nothing walked and no growth
  computed across the two scopes. The first run after this upgrade shows your v0.7.5
  observation that way; `R` or the next scheduled `observe` builds the new one. `swamp ui`
  scans on its own only when the store holds no observation at all.
- **Declared roots are in the TUI header and help and in `report --json`** (`declared_roots`).
- **`swamp report --view disk` says where the whole disk went.** Until now `report`
  headlined 16 GB under `~/src` while `df` said 443 GB used. `swamp observe --volume`
  (or any scheduled `observe` when the last pass is older than
  `volume_pass_interval_hours`, default 24) now measures the rest of the data volume
  once and stores it in a new ledger (`volume_ledger.parquet` and a meta file), and
  `report --view disk` (and a `disk` object in `report --json`) reads it back with the
  time each row was measured, never walking, statting or running anything. On this
  Mac: container 494.4 GB, 427.2 GB used = catalog and declared locations 204.6 GB
  (counted once) + everything else 132.5 GB (330 folders, top five shown: Library
  31.7, AssetsV2 27.8, /private/tmp 14.7, /private/var 11.6, ~/.local 7.4) + system
  volumes 46.6 GB + "not measured (unreadable folders or not yet measured),
  estimated" 43.3 GB + a residual of +219 MB (0.05% of used).
  Measured against `du` on the same disk: /Applications 24.27 GB (du 24.27), /opt
  25.31 (25.31), /Library 14.45 (14.45), /System/Library/AssetsV2 27.81 (27.80),
  /private 60.22 (60.12), home: the ledger's figure exceeds `du`'s (163.1 GB); the difference is not fully
  explained here (`du` cannot read some protected containers, so part of it may be the
  OrbStack data image).
- **The walk is spot-audited, and the sum is called what it is.** That the parts
  add up to the container's used bytes is bookkeeping (the estimate for protected
  folders is a leftover, so it balances anything), and the report says so
  (`bookkeeping_balanced`). The check that can fail is new: each pass re-measures up
  to five readable folders (the largest, plus a daily rotation) with an independent
  naive sum and compares them with the ledger; a difference beyond max(1%, 4 MiB)
  prints `FLAG: walk spot audit disagrees on <path>` (JSON `audit_flag`). The
  protected-folder estimate is shown as "not measured (N folders); the unexplained
  part of the Data volume, up to X GB, may be inside them (estimate)" and is not
  part of any check.
- **A folder swamp cannot read is `not measured`, never zero.** 981 folders here
  (Photos, Mail, Containers, Group Containers, `/private/var/db`, the Spotlight
  index, ...) are listed by name (the first 200 in `--json`) with the exact count.
  swamp does not ask for Full Disk Access; the report says what it would change.
- **`df`'s "used" is explained.** The System line lists System (14.1 GB), Preboot
  (21.8), Recovery (3.1), Update (1.2), VM (6.4) from `diskutil apfs list`, as
  separate volumes that share the container's free space; purgeable space and
  local snapshots (three here: names only, `tmutil` reports no sizes) appear when
  `diskutil` and `tmutil` say so. A missing or failing `diskutil`/`tmutil` is a note
  on a "not measured" row, never a 0 B line. The three queries are new allow-listed,
  read-only, counted spawns.
- **Nothing is counted twice.** The simulator runtime volumes mounted under
  `/Library/Developer/CoreSimulator/Volumes` (43.6 GB) are views of the image files
  in `/System/Library/AssetsV2`: the images are counted once, where they are stored,
  and the mounts are listed as "not added" notes. `/System` is not skipped as sealed:
  the pass walks the data volume from its own mount point, where `/System` holds
  exactly the 27.8 GB of runtime images (every path under `/` reports one device, so
  a device test cannot tell sealed from data).
- **The pass respects its budget and resumes.** It runs after the observation, under
  its own `volume-pass.lock`, and reads the mount table before touching any path (a
  network mount is never statted). It measures at background priority, three folders
  at a time, for at most `volume_pass_budget_secs` (default 120, minimum 5; the system queries and planning come first, planning limited
  to half a budget), then stops even mid-folder and continues at the next `observe`; each
  row keeps its own time. Past two budgets (plus the system queries) it stops waiting for
  a stuck path, logs it and tries it again next run (skipped after three). Here a warm pass takes about 30-35
  s and an 8-second budget stopped at 8.0-8.1 s five runs in a row. A second pass over
  an unchanged disk gives the same bytes but is not faster (no event replay for the
  whole disk yet). It is new files only: no existing table changed, the store-format
  marker did not move, an older swamp ignores them, and the writer lock is held for the
  two small writes only.

## v0.7.5

- **`swamp ui` opens immediately and no longer scans on its own.** v0.7.4 said `swamp ui`
  opened without waiting on a full observation; that was only true when the disk was
  nearly full. In the normal case it still ran a complete observation before drawing
  anything (about 9 to 30 seconds here, longer on a big tree). Now it paints the last
  stored report at once (about 0.4 seconds against a real 67 GB store) and scans only
  when there is no stored report yet, in the background, with progress in the header.
  An existing report is shown at any age. The schedule keeps it current, and `R`
  refreshes it on demand. The file-watch that used to re-scan the UI on every change is
  gone from the UI.
- **The header always says how old the data is and whether anyone is scanning.** It
  shows `observed 4m ago`, counting up while the UI is open. Older than 15 minutes it
  turns bold and says so, with a hint to press `R`. While the UI scans it shows
  `⠋ observing 12s` first on the line, with the bytes seen after it. While another swamp
  process (the scheduled `swamp observe`) holds the observation lock it shows
  `another observation running (pid N, 1m 12s)`, shortened to `observation running
  1m 12s` on a narrow terminal, never waiting on the lock, and it reloads the stored
  report when that run ends.
- **Quitting is instant.** Exit used to wait several seconds joining the file-watch
  threads. There are none now (8.6 seconds before, about 0.01 seconds after).
- **Reviewing a mark or delete no longer waits about 16 seconds.** The open-file check
  ran `lsof` with name resolution on, so it looked up a host or port name for every
  network socket. It now runs `lsof -n -P -F n`, which lists the same files: 0.2 seconds
  instead of 16.3 here. A failed or partial listing is still reported as unknown, never
  free. While the check runs, the status rows read `Checking what is in use` with elapsed
  time and no promised duration.
- **The delete confirmation leads with what matters and no longer clips it.** It used to
  be one line that ended in the size, the count and where the files go, so a long warning
  pushed them off the screen at any width. It now wraps onto rows: `Move 5 items (15.0GB)
  → Trash. Space is freed when Trash is emptied.`, then any docker items named as
  `Remove 2 docker items (1.2GB) for good, no Trash.`, then the names, then each warning on
  its own line. `Enter confirm · Esc back` stays on the bottom row.
  A bulk mark that skipped rows now says how many and why, not only the first reason.
- **Marks are visible and Esc no longer leaves hidden ones.** A project row shows `✗` when
  everything in it is marked and `~2/6` when some is. Space on a project says how many are
  marked and how large. Esc on a confirm unmarks what that Backspace or `A` marked (marks
  you made with Space stay, drawn on their rows), so a later Backspace on another row asks
  about that row. When a project has nothing rebuildable, Backspace names its `checkout`
  in the confirmation, and the help no longer says the checkout always stays.
- **The key legend no longer disappears after a delete.** The result of a delete shows
  above the legend instead of replacing it. The legend is shortened to fit narrow
  terminals and always keeps `? help  q quit`.
- **The screen no longer jumps.** Starting a check, opening the confirmation, finishing
  a delete and dismissing the result used to add and remove rows under the table, so the
  whole list slid up and down (2 to 3 rows each time). The table now stays exactly where
  it is: the bottom of the screen is always two status rows and the key legend, and the
  confirmation is a fixed 10-row sheet drawn over the bottom of the list. The result of
  a delete or a check stays until your next key instead of vanishing on a 20-second
  timer with a repaint.
- **One Down key moves one row.** The detail pane under the list used to change height
  with the selected row, so a single Down could move the selection 6 or 7 rows on an 80x24
  screen. The pane is now a fixed 4 rows, one fact per row, and the list scrolls only
  when the selection leaves the window.
- **A running check never looks frozen.** The status rows show a moving glyph and the
  elapsed time first, at any width: `⠋ 18s  Checked 41 of 342 · 184 ready · 2 blocked`,
  then the project and item being checked by name. The old percent, which disappeared
  partway through, is gone. Item names are plain (`swamp · swamp_tui incremental build
  (target/debug)`), not build hash directories.
- **A check says it changes nothing, and what happens next.** Checking no longer shows
  `successful` and `refused` counters, which read as a delete in progress. It shows ready
  and blocked, `Nothing has been changed`, and `Next: review, then Enter to move to
  Trash`. The confirmation is now a plan: ready and blocked counts, the plan by project,
  what is removed for good, then names and warnings. `d` (or `b` after a check) lists
  each blocked item with its reason and the next step.
- **The delete result says what happened in plain words.** `Moved 338 items (17.0GB) to
  Trash. Space is freed when Trash is emptied.` replaces the line with `planned` and
  `measured` figures, and docker items removed for good are counted separately. There is no
  free-space figure any more: a move to Trash on the same volume frees nothing until Trash
  is emptied, so the measured change was noise (the `-10.5MB` people saw). Anything that
  could not be moved is counted (`2 could not be moved (b to see why)`) and listed with `b`,
  each with its reason, instead of naming only the first.
- **The growth filter picker shows your history.** It said `growth off, no observations yet`
  next to a header that showed weeks of history, because it looked for history in the wrong
  directory. It now reads the same store as the header, so the growth and window fields
  offer the windows your history covers.
- **The selected row is readable on any theme.** It was a fixed dark gray bar
  (`#303030`) with your normal text color on it, unreadable on a light terminal theme. It is
  now one full-width bar in reverse video, which follows the terminal's own colors and
  survives `NO_COLOR`. Warnings and the confirmation are bold instead of yellow, results
  are plain instead of cyan (both were close to unreadable on light themes), and dim
  replaces dark gray for zero change.
- **PgUp, PgDn, Home and End work.** They move a screenful or to the ends in the list, the
  help, the blocked list, the cargo inspection popup and the filter picker. Before, they did
  nothing, in a list where `A` can mark 342 items.
- **Help is readable and scrolls.** The `A` and Backspace entries used to run together on
  one row and clip; each key now has its own row, long entries wrap under themselves at 50,
  80 and 120 columns, the badge legend wraps, and the whole text scrolls (`↑↓`, PgUp, PgDn,
  Home, End; `Esc` or `q` closes). The title shows where you are, `22-43 of 68`.
- **`k` says what it did.** It flips a setting that is remembered between sessions and
  used to do it in silence. The result line now reads `Keep executables is now on: release
  and debug programs are copied to bin/ before their folder goes to Trash. Remembered for
  next time. k turns it off.`, the confirmation shows the current value, and the help and
  usage table document it.
- **The key legend keeps what you reach for.** At 80 columns it reads `/ filter  v view  R
  refresh  ⌫ delete  Space mark  A mark all  ↑↓ move  ? help  q quit`. Movement keys go
  first when space runs out, not filter, view, refresh and delete.
- **Views keep their place and say where you are.** `v` and Esc used to send the cursor to
  the top; each view now remembers its cursor. The view line reads `builds of mole (3 of 10
  · v next · Esc: projects)`. An empty list says why and what to press: `Nothing matches
  the filter "growth > 900GB in 7d". Press / to change it, or 0 to clear it and see
  everything.`, or `No Docker images, containers or volumes found. Press v for another
  view.` instead of `no rows match` under `filter: none`.
- **The detail pane is plain.** Every row used to show `reclaimability (estimated
  reclaimable): conflicting [0B, 24.6KB] (APFS clone/snapshot extent sharing outside this
  selection is not queried ...)`. It now says what the row is and what getting it back
  costs (`440.4MB · Installed dependencies. Installing again brings them back, usually over
  the network.`), then only the facts that change a decision (`In use right now.`, `Used by
  2 projects: mole, swamp.`, `Last changed 1h ago.`), and one line naming what could not be
  established (`Unknown: how to get it back.`). The space-freed caveat shows only when the
  possible gap is at least 1MB. `groups`, `units` and `occupancy` are gone from the screen.
  The full evidence, with sources, is still in `swamp report` and `--json`.
- **The header's net change is labeled.** `-41.4GB` next to the history sparkline now reads
  `-41.4GB in 1w`.
- **The picker prints its keys once.** They were in the box and again in the footer; they
  are in the footer only.
- **`d` shows the whole blocked reason, and `r` checks again.** In the blocked list, each
  item's reason and next step wrap in full instead of being cut, and `r` runs the same
  check again from a fresh look at the disk (a process that held an item open may have
  exited). It replaces the marks that check added and keeps marks you made another way.
- **`R` says so when another observation is running.** With the schedule (or a manual
  `swamp observe`) already holding the lock, `R` used to look like nothing happened. It now
  says `An observation is already running (pid N, 32s). Its result loads here when it
  finishes.` and starts nothing. The header no longer calls a manual run `scheduled`.
- **The terminal always comes back.** A panic on the UI thread, SIGTERM, SIGHUP or SIGINT
  used to leave your shell in raw mode on the alternate screen with no cursor (measured:
  after SIGTERM, echo and line mode stayed off). The terminal is now restored on every way
  out, and a panic message prints on the normal screen where you can read it. The restore
  runs before the child-process cleanup that v0.7.4 added; both still happen.
- **The screen is no longer blank while the index loads.** It shows `swamp · reading the
  last observation…` from the first moment instead of an empty terminal for about half a
  second.
- **An idle UI draws nothing.** It used to repaint five times a second, a 25-byte burst
  each time (49 writes in 10 seconds over ssh or tmux). It now paints when a key, a
  resize or a result arrives, every 200 ms while something is busy, and when the age in the
  header reads differently: 0 writes and no measurable CPU in 10 idle seconds.
  Sort, filter and `k` choices are saved on a worker thread, so a slow or full disk cannot
  stall a keypress; the last choice is flushed on exit.
- **The screen updates when a check or a move finishes.** It used to stay on the old
  `Checked 0 of 1 ... starting` rows until you pressed a key. The result, the plan and
  the header now appear on their own. The age reads `just now` for the first minute, then
  minutes, so an idle screen stays still.
- **Esc on a confirm undoes exactly what its own press marked, every time.** That now holds
  after `r` (check again) in the blocked list and after pressing `A` again while the
  confirm is open. A scan that finishes while a confirm or a check is open no longer
  changes what Enter would move: the new data waits (`new data available` in the header)
  and loads when the confirm closes. Marks for items the new scan no longer lists are
  dropped, and the result line says how many.
- **The plan is easier to read at any size.** It has `Ready:` and `Blocked:` headings, the
  per-project list says `Ready, by project:`, and the `Includes:` line names each kind
  once with a count (`node_modules (40), .build (3)`). A small terminal fills the sheet
  with the count, size and destination first. When a plan removes docker items for good
  and the terminal is smaller than 40x8, Enter is not offered: it says to enlarge the
  window.
- **`R` during a check says so** (`A check is running; press R after it finishes`), and
  `swamp ui` without a terminal says to use `swamp report` instead of `Device not
  configured`.
- **Long tree rows keep their outline.** Truncating a long name no longer cuts the tree
  rail (`│  │ … ├─ node-server`); only the name gives way.

## v0.7.4

- **Reviewing cleanup candidates is fast again.** `swamp ui` used to run one
  `lsof +D <dir>` per cleanup group to see whether anything held it open, and each
  one walks the whole directory tree. Reviewing 1,239 groups in a 35 GB cargo
  `target/` took about an hour. A review now takes one machine-wide open-file snapshot
  (`lsof -F n`, no tree walk; about 16 seconds on the machine it was measured on) and
  answers every group from it by path prefix. If the snapshot fails, times out, or is
  permission-limited, a group it cannot find open is reported as unknown, never free.
  Checking a single path is unchanged.
- **A nearly full disk no longer makes swamp crawl.** `swamp observe` first checks free
  space on the volume holding the swamp store. Below `min_free_bytes` (default: the
  greater of 1 GiB and 1% of the volume; `0` turns the check off) it exits with code 3
  and one line on stderr, before walking anything. It writes nothing, takes no lock and
  changes no coverage fact, so an aborted run never marks a root missing. `swamp ui`
  without a root no longer waits on a full observation before opening: it shows the
  last stored report immediately, with a "disk nearly full: refresh skipped" banner
  when the guard trips. Only the store's volume is checked.
- **Swamp cleans up every process it starts.** Each child (`git`, `gh`, `lsof`,
  `xcrun`, `docker`, ...) now runs in its own process group and is killed, with its
  descendants, and reaped on timeout, on error or panic, when you press Esc or Ctrl-C
  on a running operation, on quit, on SIGINT, SIGTERM or SIGHUP, and at process exit.
  Children run with git and gh pagers and credential prompts disabled, so none can sit
  waiting for a terminal. Killing swamp with SIGKILL still leaves its children behind.
  Closes [#156](https://github.com/open-horizon-labs/swamp/issues/156).
- **GitHub enrichment re-fetches far less.** A worktree whose branch is merged is final:
  it is not re-enriched automatically while its tip commit is unchanged, and a `gh`
  outage no longer overwrites its merged state with `unknown`. Every other row is
  refreshed after 24 hours instead of six. `swamp observe --enrich` is back as the
  on-demand override: it refetches every worktree now, ignoring both rules.
  `--no-enrich` still skips GitHub. This does not make `observe` faster; its time is
  the filesystem scan.

## v0.7.3

- `swamp observe` now enriches worktrees from GitHub by default, as its help and the
  usage guide already described. Before this, only `--enrich` made any `gh` call, so
  the scheduled observation (which passes no flags) never did and every worktree's
  `merged` state stayed `unknown`. Results are cached by tip SHA for six hours, so
  repeat observations make few or no calls. `--no-enrich` skips it; `--enrich` is
  removed because it is now the default.

## v0.7.2

- Count linked worktrees that live outside every scan root. Each discovered
  checkout's `.git/worktrees/` registry is read, and every entry that points back
  to the same repository is measured under its project, on full and incremental
  observations alike. Deleted, moved, symlinked or foreign entries are skipped.
  `report` lists the paths reached this way.
- Codex managed worktrees (`~/.codex/worktrees`, or `[desktop] git-worktree-root`)
  now appear as linked worktrees of their projects instead of inside Codex's
  unclassified residual. The Codex agent view cross-references them without
  counting their bytes twice.

## v0.7.1

- Automatically reset incompatible Swamp scan state before observation and rebuild
  it with a fresh scan. An unversioned/incompatible store loses incompatible derived
  history; valid current-format history is retained. Reset is limited to recognized
  Swamp-owned derived generations before cached tables are read. Configuration,
  protection intent, notes, ledger state, user-declared consumers, unrelated files,
  and active enrichment are preserved.
- Compatible stores retain history across narrow-root observations. Reset recognizes
  retired Parquet views, JSON sidecars, external measurement caches, association
  caches, reverse-delta generations, reports, and plans; no age-based pruning or
  root-liveness inference is used.
- Serialize observation writers across CLI, scheduled, and TUI refresh paths.

## v0.7.0

Swamp 0.7.0 adds Linux support, toolchain and version-manager storage discovery,
and project-linked agent storage. It extends disk usage and history beyond
checkouts, with build details that help you choose what to keep or remove.

### See installed toolchains and which projects reference them

- Discover storage managed by mise, asdf, pyenv, uv, Conda, rbenv, RVM,
  ruby-install, nvm, and rustup, including supported location overrides.
- Distinguish installations, environments, downloads, caches, shims, and other
  manager state rather than treating each tool home as one unexplained total.
- Match project declarations such as `.tool-versions`, `mise.toml`,
  `.python-version`, `.ruby-version`, `.nvmrc`, and `rust-toolchain.toml` to
  measured mise/asdf/pyenv/rbenv/RVM/nvm/rustup installations. Show resolved
  references on both the project and the installation; keep rustup's global
  default separate from project declarations.
- These links identify declared consumers, not proven runtime use. An unmatched
  installation is not necessarily unused, and discovery does not imply that
  every installation supports cleanup. See the
  [tool-location catalog](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/locations.md).

### Run Swamp on Linux

- Linux x86_64 joins Apple silicon macOS as a supported release target, with
  a glibc-based archive built on Ubuntu 24.04 and tested on a newer Ubuntu runner.
- Linux uses inotify while the TUI or opt-in collector is running; uncovered
  intervals trigger a full walk. macOS continues to use persisted FSEvents.
- Linux scheduling uses systemd user timers; macOS uses LaunchAgent.
  Scheduled observation does not perform cleanup.

### Find growth across your development environment

- One effective scope combines defaults, enabled developer-tool detectors, and
  configured additions/exclusions. Inspect it with `swamp scope`, or pass explicit
  roots consistently to observation, reporting, and the TUI.
- External tool homes and agent storage have their own units and project links.
  Claude Code, Codex, Oh My Pi, OpenCode, and the wider agent catalog have
  adapter-specific coverage; missing or ambiguous ownership remains visible.
- Codex attribution reads its own thread-state database read-only rather than
  scanning conversation JSONL. Swamp's observation store remains Parquet.
- Multi-root reports preserve root coverage and observation-time history.
  Missing roots and newly added scope are not reported as ordinary storage changes.

### Understand the cleanup trade-off

- Building on 0.6's Cargo cleanup groups, shared role families extend build
  identification and consequences to other ecosystems. Cargo group/profile
  selection still expands to supported members, not the entire profile.
- Shared build-role families carry identification, age, accounting, and removal
  consequences into other ecosystem adapters. Project-local cleanup and
  shared-store inspection remain distinct capabilities.
- Age prioritizes review without claiming a build is obsolete. Shared hardlinks
  do not automatically prohibit removal or justify a promise about freed space.
- The TUI retains background observation, selection review, deletion progress,
  and cancellation between groups while extending the storage it can explain.
- On-demand Cargo inspection provides deeper bounded detail without making
  every ordinary refresh index individual dependency files.

### Keep repeated observation practical

- Folded directory measurements reuse unchanged containers. Current typed facts
  and reverse-delta history stay in compressed Parquet, without a separate JSON
  artifact cache or permanent inode inventory.
- Fast refresh retains explicitly stale unique-byte estimates instead of
  rewalking unchanged roots for hardlink accounting. `swamp observe --full`
  reconciles the observed scope and bounded container-sharing summaries.
- Report reads do not rescan directories or spawn enrichment processes.
  JSON build-unit pagination bounds detail while preserving full family summaries.
- Project/worktree deduplication, scope exclusions through path aliases, growth
  coverage, and text/JSON project filtering received regression fixes.

### Use the CLI and installable agent skill

- The agent interface is the CLI plus an installable skill. Install it through
  `npx skills add open-horizon-labs/swamp --skill swamp`;
  a bundled reference covers platform-appropriate binary installation.
  The MCP server and CLI deletion/approval commands are removed; removal is confirmed
  by a human in the TUI.

### Know the boundaries

History begins with observation. Ownership and cleanup coverage vary by adapter;
see the checked [build](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/build-artifacts.md) and
[agent-storage](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/agent-storage.md) matrices. Homebrew discovery is opt-in.

Filesystem cleanup moves paths to Trash, which must be emptied to reclaim space.
Docker image/volume removal has no Trash recovery. Allocated row sizes are not
guaranteed freed bytes, and cleanup does not re-check every fact after marking.

The [usage guide](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/usage.md) covers the workflow and controls. The rewritten
[architecture guide](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/architecture.md) explains folding, history, adapters,
enrichment, and the costs that remain.

## v0.6.3

- Show Cargo build details directly in project trees, grouped by cleanup consequence: compiler caches, compiled tests and examples, and build-script output. Keep the directory view available without counting it as additional storage.
- Show candidate counts, allocated sizes, modification ages and removal consequences. Use available terminal space for candidate previews; improve Unicode alignment, narrow layouts and scrolling.
- Allow selecting a cleanup category or profile to review its supported groups. Profile selection does not delete the entire profile directory or silently include unsupported outputs.
- Run cleanup review and execution in background workers. Show progress, completed/refused counts and the current path; Escape or Ctrl-C stops between groups without interrupting an in-flight move. Cancelled reviews preserve the prior selection, and failed or unattempted cleanup selections remain available for review.
- Add a repository-specific AST audit against known blocking cleanup/review calls on TUI event and rendering paths, with regression fixtures in workspace tests.
- Document build details with a real screenshot and explain age, shared-link accounting, selection scope and Trash behavior. Age suggests what to review; it does not prove disuse. Dependencies remain a folded aggregate, not a per-crate size map.

## v0.6.2

- Allow reviewed Cargo groups containing hardlinks to move to same-filesystem Trash. Unselected links remain intact; uncertain reclaimed space no longer blocks cleanup. Content, membership, identity, lock and authorization checks remain in place.
- Isolate observations, history and replay checkpoints by canonical scan root, fixing proposals after switching between a project and its parent in one store. Root aliases share a scope; each new scope starts its own baseline.
- Add `cleanup-check --offset` paging and `--within` discovery scope, candidate totals, unchecked counts and coverage limits. Show hardlink/accounting warnings with results so a small checked page cannot be mistaken for the total cleanup opportunity.

## v0.6.1

- Added `cleanup-check`: a bounded review of individual Cargo groups that produces unapproved plans or specific refusals, without widening selections or deleting anything.
- Distinguished category totals, unchecked groups and blocked outputs in Rust reports, JSON and the Builds TUI. Rust text reports show 30 rows by default (`--all` restores the complete list), with accounting guidance before the rows.
- Replaced confusing Cargo category-selection errors with instructions to choose individual groups. Added hardlink and lock reason codes, retry guidance, exact reviewed members and recovery details.
- Kept existing hardlink, freshness, lock and human-approval protections. This release improves cleanup discovery; it does not add support for removing hardlinked groups or prove that old builds are unused.

## v0.6.0

- Added Rust build drilldown for Cargo profiles, folded dependencies, test/example executables, incremental-cache and build-script groups, and final executables/libraries. Group history does not inflate project totals. Dependency sizes remain a directory aggregate, not a per-crate breakdown.
- Added reviewed selective cleanup for evidenced test/example executables and individual incremental/build-script groups. Cleanup checks contents, producer evidence, locks, hardlinks and occupancy before moving the selection to Trash with restore metadata. Shared dependencies and final outputs remain inspection-only; age is not proof that a build is unused.
- Kept ordinary compiler files folded in reports and history, with trusted unchanged-container reuse instead of a persisted per-file inventory.
- Made incremental hardlinked-artifact refreshes update directory allocations without a whole-artifact rewalk. Unique-byte totals are explicitly marked stale until a full scan reconciles them; stale unique measurements create history gaps and cannot spend standing-grant budgets.
- Compacted small reverse deltas and delayed replay-checkpoint publication until report persistence succeeds.
- Fixed Cargo cleanup lock lifetime under concurrent process creation. Added native Cargo build/cleanup/rebuild verification and parallel stress coverage.

## v0.5.2

- Narrowed Ruby dependency attribution to `vendor/bundle`, preserving unrelated vendored source.
- Added declarative project-name fallbacks for Gradle settings, Cabal, and Python `setup.cfg` manifests.
- Made strict JSON manifest names structural and top-level only.
- Added regressions for shared Rust workspace targets and independent nested project targets.

## v0.5.1

- Fixed project badges with linked worktree counts so the worktree glyph and multi-digit count remain visually separated in the terminal UI.

## v0.5.0

- Fixed persistence of existing artifact byte changes in `current.parquet`. Added regression coverage for successive updates and unchanged observations after an update.
- Renamed the repository, source packages, binaries, environment variables, and release artifacts to `swamp`. The v0.5.0 archive contains `swamp` and `swamp-mcp`.
- Reorganized the documentation into a product overview, usage reference, architecture guide, and contributor guide. Corrected outdated UI, filter, installation, and history claims.

## v0.4.0

### Docker removal

- Added image and volume removal to the CLI, MCP, and TUI through Docker. This includes objects without project attribution.
- Added object-specific recovery information to plans, confirmations, and ledger records. Filesystem paths go to Trash; Docker removals do not. Images may be pulled or rebuilt if their sources remain available; swamp makes no copy of volume contents.
- Refused individual build-cache removal because the action path does not support that unit.
- Added live object checks before removal and surfaced Docker's refusal text.
- Separated trashed bytes, permanent removals, and measured free-space change in results.
- Added a Docker fixture with attributed and unattributed images, a volume, dangling images, build cache, and a container that prevents image removal.

### Git tracking and actions

- Split the worktree remainder into tracked `source`, `ignored`, and `untracked` buckets using directory-level Git status with large-file corrections. Apportioned totals preserve the walk's measured bytes.
- Prevented deletion of the ignored/untracked aggregate buckets as single paths.
- Reused the Git exclude stack within a checkout.
- Allowed direct project actions to select its actionable artifacts. When none exist, a direct action can offer the checkout; bulk marking skips that fallback.

### Terminal UI

- Replaced row sparklines with logarithmic change bars: growth extends right in red, shrink left in green, and changes below 1 MB use a small tick.
- Sorted growth by signed value, so increases precede decreases.
- Made Right open/expand and Left collapse/return, matching the displayed navigation.
- Fixed negative growth formatting, duplicated carried-forward GitHub signals, and empty container names.

### Storage and checks

- Changed report-history Parquet writes to close a temporary file before renaming it over the destination. This protects readers from unfinished individual files; it is not a multi-file transaction.
- Corrected source checks that matched their own explanatory comments and removed their ripgrep dependency.
- Corrected the fixture's dangling-image case on Docker installations using the containerd image store.

## v0.3.0

### Live and incremental observation

- Added a live FSEvents watch while the TUI is open, with observation after 400 ms of quiet.
- Retained directory detail inside folded artifacts so updates can re-list changed interior directories.
- Added a whole-artifact fallback for hardlinked units and local byte measurements to support deduplicated incremental accounting.
- Re-listed known remainder directories without walking their entire worktree when possible.
- Built one history index per growth-annotation pass and skipped unchanged current-file writes.
- Reused and aged Git signals for untouched worktrees, cached Docker facts for five minutes, and limited discovery around changed directories.

The release recorded these observations on one `~/src` tree with 55 projects and about 44 GB. Hardware details, repeated samples, and a reproducible benchmark harness were not supplied with the table.

| Case | v0.2.0 | v0.3.0 |
|---|---|---|
| Source file touched | 7.5 s | 75 ms |
| Change inside a 16 GB `target/` | 7.5 s | 2.4 s |
| No change | 7.5 s | 86 ms |

### Artifact recognition

- Added recognition of regular `CACHEDIR.TAG` files with the required signature.
- Vendored ignore/language lists and added a harvest utility to report candidate artifact names without editing the classification table.
- Expanded marker-gated recognition across ecosystems, including ESP-IDF, Godot, Jekyll, Elm, Erlang, OCaml, Clojure, and Nim.
- Added ESP-IDF `managed_components/` and build variants, CMake build variants, and Python `requirements*` markers.

### UI and reporting

- Changed growth to red and shrink to green; used a dark selection background to preserve those colors.
- Added history charts based on observed changes and ecosystem badges after project names.
- Preserved selection during background refresh.
- Included directory rows in the startup observation.
- Corrected progress counters and limited percentage display to cases where the previous total is a usable estimate.
- Added `--version` and release-version smoke checks.

## v0.2.0

- Added project ecosystem detection, marker-gated artifact classification, type filters and sorting, a types view, and manifest-based names for repositories without remotes.
- Added size and age predicates, project globs, more sorts, and persisted UI choices.
- Added the keep-executables option for supported Rust and Python outputs.
- Added observation progress and UI refresh after removal.
- Forced a full walk after classification-rule changes.
- Added time-series display with unobserved buckets represented separately from zero changes.
- Reorganized report construction into consumers on an in-memory event bus. See [ADR 001](docs/ADRs/001-event-bus-report-pipeline.md).
- Added source audits for selected implementation constraints and a mutation script for walker checks.
- Added configuration commands and MCP report filtering.

## v0.1.0

- Introduced a terminal UI, CLI, and stdio MCP server for disk growth by project, checkout/worktree, and artifact.
- Added Parquet history with reverse deltas, FSEvents-based incremental observation, and optional scheduled observation.
- Grouped clones by remote and exposed worktree activity, Git tracking, and cached GitHub facts.
- Added Docker attribution by Compose/source evidence, with unmatched objects reported as unowned. Docker removal arrived in v0.4.0.
- Added filesystem reconciliation and an optional `du` comparison.
- Added action plans, CLI approval and standing grants, execution, and ledger records.

The initial release targeted Apple silicon macOS with unsigned binaries. History began with the first observation. Performance figures were observations from one machine.
