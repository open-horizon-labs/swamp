# Why swamp exists

`df` says the disk is full. `du` says where the bytes are. Neither answers the
question you have on a developer machine: which project owns this, what would
it cost to remove, and is anything using it right now. swamp answers those
three and then gets out of the way while you decide.

## The problem

Coding agents fill a disk with worktrees, build output, dependency caches and
session scratch before you notice. A disk-usage tool shows you a tree of
folders. It cannot tell you that two clones and five linked worktrees are one
project, that a 36 GB cache folder has a dozen owners, that a 42 GB simulator
volume is a view of 28 GB of image files already counted elsewhere, or that a
build directory is in use by a process right now. The build tools' own cleanup
commands each know only their own corner.

On the machine swamp was built on, the report covered 16 GB under `~/src`
while `df` said 443 GB were in use. The other 427 GB was developer storage
that no tool attributed to anything.

## What swamp does

**It observes once and reports from what it stored.** A scheduled pass (a
LaunchAgent on macOS, a systemd user timer on Linux) walks the roots you
declared and a built-in catalog of tool locations, and writes Parquet tables.
Opening the report or the TUI reads those tables and nothing else: no
filesystem walk, no network call. On macOS the pass replays the system's own
change history instead of re-walking, so an unchanged pass touches only what
moved.

**It attributes bytes to projects, and says what it could not attribute.**
Clones and worktrees are grouped by their Git remote. Build output is grouped
by what removal would cost: a compiler cache you can rebuild is not the same
as a final binary, and a cache shared by other projects is not the same as one
project's own. A folder swamp could not read is *not measured*, never zero.
Parts add up to the total or the residual is named. On the build machine:
427.2 GB used, of which 204.6 GB in catalog and declared locations, 132.5 GB
in everything else, 46.6 GB in system volumes, 43.3 GB estimated for
protected folders, and a residual of +219 MB.

**It tells you when something was last used, and where it learned that.** A
tool's own record comes first: Xcode's `LastAccessedDate`, Cargo's global
cache, a package manager's own answer. File access time is the fallback, and
"no record" is the honest third state. The source is always on the row: "Sep
25, file access time". swamp never says "unused". The build fails on verdict
words in anything it prints.

**It reports; you remove.** Mark a real path in the TUI and review it before moving it to Trash. The review shows destination totals, common warnings once, and item-specific exceptions: “cannot be regenerated”, “last used: no record”, “in use right now by ...”, “967 MB of this stays in the shared blobs folder”. Read the primary summary, then confirm; `l` opens the optional inventory of every exact path, size and member. Enter is disabled inside that inventory. Long selections no longer require every path to fit on one screen.

These facts inform the decision. Swamp refuses when it cannot perform the move correctly: an invalid target, an OS denial, a mark changed since review, overlapping marks, an unwritable ledger, your own protect entry, or its own ledger folder. Every move is written to a locked ledger before it happens. Trash is the way back. The [usage guide](usage.md#terminal-controls) covers current controls and the difference between Trash and permanent manager removal.

For the two managers whose removals cannot be undone (mise versions,
simulator runtimes), the manager's own command runs only on `Y`, only after
the confirm has been on screen for a second, and only after a fresh check
shows the same facts.

**It stays inside the lines.** Roots are declared by you; swamp never reads
shell history, editor recents or Spotlight to guess them. Every program it
runs comes from a fixed location per platform, never from `PATH`. Symlinks
are never followed. A store written by a newer swamp is read, never modified.

## What 0.8.0 added

```text
Developer storage: 224.1GB across 40 projects and 77 tool locations (50.4% of used)
toolchains 57.1GB · projects 42.4GB · agents 40.7GB · other 39.3GB · VMs 23.5GB · caches 22.8GB
```

- **The headline and the Reclaim view.** Every unit of developer storage,
  largest first, with what getting it back costs, when it was last used and
  from what record, who is known to need it, and what the package manager
  itself reports, quoted.
- **The disk view.** A volume ledger that reconciles with `df`, names the
  system volumes that share the container (System, Preboot, Recovery, Update,
  VM), and counts simulator mounts as views of image files, not twice.
- **Model caches.** One row per Hugging Face repo and Ollama tag, with a line
  built from files already on disk: "whisper · 241.7M params · float32",
  "qwen3 · 751.63M params · Q4_K_M". Sizes match `du` to the byte. A warm
  pass reads 0 bytes. Asking huggingface.co is off by default and sends no
  token.
- **Trash from anywhere.** Reclaim, External and Disk rows and the folders
  under them, with warnings instead of refusals.
- **Faster unchanged passes.** Median 42.7 s to 31.3 s at background priority
  on a copy of the build machine's store. Sealed read-only simulator volumes
  replay in 9 ms instead of a 32 s walk.
- **A nested TUI.** Three sections (Projects, Tools, Disk) instead of a flat
  row of single-letter views. The old keys are gone.
- **`swamp config set/get/list`**, declared roots with `add-root`, and a
  Homebrew policy that counts developer formulas by default and everything
  else as one row.

The numbers are from [CHANGELOG.md](../CHANGELOG.md), measured on one
machine. They are not performance guarantees.

## Current limits

- Promise how many bytes a deletion frees. Trash keeps the bytes until
  emptied; other hard links, APFS clones and snapshots can keep them longer.
- Measure folders it has no permission to read. Without Full Disk Access they
  are listed as not measured (981 on the build machine), and the unexplained
  part of the volume is an estimate.
- Move to a Trash on another volume. swamp moves by rename and never copies.
- Update a unit's "hard-linked" label when a link is made from outside a
  folder it replayed. The bytes stay exact; the label catches up when the
  folder changes.
- Guarantee reuse after a gap in event history. On macOS, missing or invalid FSEvents history requires a full measurement; older cursors without the event-store UUID need one full pass before reuse resumes.
- Promise live daemon facts from a stored report. Routine observations reuse Docker facts for up to five minutes and show their age; `swamp observe --enrich` requests fresh daemon facts.
- Provide a CLI deletion command. Reports and JSON provide evidence; removal is a separate TUI action.
- Replay history on Linux. Linux walks everything unless the opt-in collector
  has been running.

swamp is a developer-storage tool, not an audit of every file, writer and
access. Where it is unsure, it says so on the row.
