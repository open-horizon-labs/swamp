# Why swamp exists

On a developer's machine, `df` says the disk is full and `du` says where the
bytes are, and neither answers the question you have: which project owns this,
what would it cost to remove, and is anything using it right now. swamp answers
those three, and then gets out of the way while you decide.

Every claim below cites the file that enforces it or the changelog entry that
measured it. Where the repo does not back a claim, this document does not make
it.

## The problem

Coding agents fill a disk with worktrees, build output, dependency caches and
session scratch before you notice. A disk-usage tool shows you a tree of
folders. It cannot tell you that two clones and five linked worktrees are one
project, that a 36 GB cache folder has a dozen owners, that a 42 GB simulator
volume is a view of 28 GB of image files already counted elsewhere, or that a
build directory is used by a process right now. The builders' own cleanup
commands know only their own corner.

On the machine swamp was built on, the gap was the whole picture: the report
covered 16 GB under `~/src` while `df` said 443 GB were in use
([CHANGELOG](../CHANGELOG.md), the `report --view disk` entry).

## What swamp does about it

### 1. It observes once and reports from what it stored

A scheduled pass (a LaunchAgent on macOS, a systemd user timer on Linux) walks
the declared roots and the built-in catalog of tool locations, and writes
Parquet tables. Opening the report or the TUI reads those tables and nothing
else: no filesystem walk, no GitHub call. On macOS the pass replays the
system's own change history instead of re-walking, so an unchanged pass
touches only what moved.

Evidence: [architecture.md](architecture.md) ("observation versus reporting");
guardrails `no-second-traversal-on-report-path` and `fsevents-before-full-walk`
in `.oh/guardrails/`. The TUI opens in about 0.4 s against a real 67 GB store,
down from 9 to 30 s before 0.7.5; quit went from 8.6 s to about 0.01 s
([CHANGELOG](../CHANGELOG.md), v0.7.5).

### 2. It attributes bytes to projects, and says what it could not attribute

Clones and worktrees are grouped by their Git remote. Build output is grouped
by what removal would cost: a compiler cache you can rebuild is not the same
as a final binary, and a cache shared by other projects is not the same as one
project's own. A folder swamp could not read is *not measured*, never zero.
Parts add up to the total or the residual is named.

The disk view on the build machine: container 494.4 GB, 427.2 GB used, of
which catalog and declared locations 204.6 GB, everything else 132.5 GB across
330 folders, system volumes 46.6 GB, not measured (estimated) 43.3 GB, and a
residual of +219 MB, which is 0.05% of used.

Evidence: [README](../README.md) "Start with the decision";
`crates/core/src/headline.rs` (`bookkeeping_balanced`, `NotMeasuredPart`);
[CHANGELOG](../CHANGELOG.md), `report --view disk`. The balance itself is
bookkeeping: the estimate for protected folders is a leftover, so it balances
anything. The check that can fail is the spot audit, which re-measures up to
five folders per pass and prints `FLAG` if one differs by more than 1% or
4 MiB.

### 3. It tells you when something was last used, and where it learned that

A tool's own record comes first: Xcode's `LastAccessedDate`, Cargo's global
cache, a package manager's own answer. File access time is the fallback, and
"no record" is the honest third state. The source is always on the row
("Sep 25, file access time"). It never says "unused": a grep gate and a token
audit fail the build on verdict words in delivered strings.

Evidence: `crates/core/src/last_used.rs`; `scripts/check.sh` (the grep gate);
guardrail `agent-interface-facts-not-verdicts`.

### 4. It reports, and you remove

Anything you can see in the TUI you can mark and move to Trash after one
reviewed confirm. The confirm lists every exact path and size, and every fact
swamp knows that you might want first: "cannot be regenerated", "last used: no
record", "in use right now by ...", "bytes are a lower bound", "967 MB of this
stays in the shared blobs folder". Those are lines on the confirm, never
refusals.

swamp refuses only when it could not do the thing correctly: a path that is
not a real folder or file, an OS denial, a mark that changed since you
reviewed it, an overlap with another mark, a ledger it cannot write, your own
`swamp protect` entry, and its own ledger folder. Every move is written to a
locked ledger before it happens. Trash is the way back, so there is nothing to
justify.

For the two managers whose removals cannot be undone (mise versions, simulator
runtimes), the manager's own command runs only on `Y`, only after the confirm
has been on screen for a second, and only after a fresh re-check shows the
same facts.

Evidence: [CHANGELOG](../CHANGELOG.md), the Trash entry;
`crates/core/src/ledger.rs`; guardrails `trash-backend-owns-every-move`,
`protection-fails-closed`, `tool-removal-refuses-on-manager-facts`.

### 5. It stays inside the lines

- Roots are declared by you. swamp never reads shell history, editor recents
  or Spotlight to guess them; a source-audit rule bans those literals.
- Every program it runs comes from a fixed location per platform, never from
  `PATH`, and every spawn is counted.
- Symlinks are never followed. An unreadable protect list is shown as unknown,
  not treated as empty.
- A store written by a newer swamp is read, never modified. Upgrading from
  0.7.5 keeps your store; the new tables sit beside the old ones.

Evidence: guardrails `roots-are-declared-never-inferred`,
`every-spawn-is-counted`, `symlinks-never-followed`;
`crates/core/src/fs_gate/program_paths.rs`; `crates/core/src/growth.rs`
(`store_marker_is_foreign`).

## What 0.8.0 adds

```text
Developer storage: 224.1GB across 40 projects and 77 tool locations (50.4% of used)
toolchains 57.1GB · projects 42.4GB · agents 40.7GB · other 39.3GB · VMs 23.5GB · caches 22.8GB
```

- **The headline and the Reclaim view.** Every unit of developer storage,
  largest first, with what getting it back costs, when it was last used and
  from what record, who is known to need it, and what the package manager
  itself reports, quoted and attributed.
- **The disk view.** A volume ledger that reconciles with `df`, names the
  system volumes (System, Preboot, Recovery, Update, VM) that share the
  container, and counts the simulator mounts as views of image files, not
  twice.
- **Model caches.** One row per Hugging Face repo and Ollama tag, with a line
  that says what it is from files already on disk: "whisper · 241.7M params ·
  float32", "qwen3 · 751.63M params · Q4_K_M". Sizes match `du` to the byte.
  A warm pass reads 0 bytes, because parsed headers are cached by content
  identity. Asking huggingface.co is off by default and sends no token.
- **Trash from anywhere.** Reclaim, External and Disk rows and the folders
  under them, with warnings instead of refusals (section 4).
- **Faster unchanged passes.** Median 42.7 s to 31.3 s at background priority
  on a copy of the build machine's store; files stat'ed per pass about 750,000
  to 360,000. Sealed read-only simulator volumes replay in 9 ms instead of a
  32 s walk.
- **A nested TUI.** Three sections (Projects, Tools, Disk) instead of a flat
  row of single-letter views. This is a breaking change to the old keys.
- **`swamp config set/get/list`**, declared roots with `add-root`, and a
  Homebrew policy that counts developer formulas by default and everything
  else as one row.

Every figure is quoted from the [CHANGELOG](../CHANGELOG.md) `## v0.8.0`
section as written there. The 0.8.0 release candidate carries 2,319 `#[test]`
functions, of which 152 are adversarial tests that name the wrong
implementation they fail.

## What it does not do

- It is not a forensic inventory, and it does not promise how many bytes a
  deletion frees. Trash keeps the bytes until emptied; other hard links, APFS
  clones and snapshots can keep them longer.
- Without Full Disk Access, protected folders (981 on the build machine) are
  listed as not measured, and the unexplained part of the Data volume is an
  estimate.
- A Trash on another volume is refused: swamp moves by rename and never
  copies.
- A unit's "hard-linked" label can lag when a link is made from outside a
  replayed folder; the bytes stay exact and the label catches up when the
  folder changes.
- The stored change cursor is not yet checked against the FSEvents database
  UUID; that is a store-format change left for a later release.
- Docker and mise are still asked on every pass. The agent and JSON plan path
  still refuses what an AI tool must not decide on its own.
- Linux walks everything unless the opt-in collector has been running; there
  is no change history to replay.

Evidence: [architecture.md](architecture.md) "Implementation limits";
[usage.md](usage.md); `crates/core/src/reclaim_trash.rs`; issues #212 and
#211.

## Where this sits

The repo does not compare itself to other disk tools, and this document does
not invent that comparison. The design position is in
[PRODUCT.md](../PRODUCT.md): a developer-storage tool, not a perfect audit of
every file, writer and access. Where swamp is unsure, it says so on the row.
