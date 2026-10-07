# swamp

Disk growth, by project.

Coding agents can fill a disk with worktrees, builds, dependencies, and session data before you notice. Swamp helps you answer three questions: **what grew, which project owns it, and what would removing it cost?**

It groups separate clones and linked worktrees by Git remote, keeps observation history, and explains development-specific storage. You can inspect a project's compiler caches and test builds without treating its source files the same way.

## Start with the decision

A large directory may have been large for months. Swamp shows size alongside growth so you can find what changed, open the affected project, and inspect the responsible checkout, worktree, or artifact.

Build details group things by their use and removal consequences:

- **Compiler caches:** a place to start if you accept a slower next build.
- **Compiled tests and examples:** rebuild before running them again.
- **Build-script output:** scripts run again; their tools and inputs may be needed.
- **Dependencies and shared stores:** distinguish project-local output from storage used by other projects.

Age helps prioritize review. It does not prove that a build is obsolete. Allocated bytes do not promise how much space deletion will free.

Storage outside a checkout matters too. Swamp discovers developer-tool homes and agent storage, including Claude Code, Codex, Oh My Pi, and OpenCode. Where evidence supports it, these units link back to projects. Missing or ambiguous ownership stays visible rather than becoming a guessed assignment. See the checked [agent-storage](docs/agent-storage.md) and [build-artifact](docs/build-artifacts.md) coverage tables.

## Install

On Apple silicon macOS:

```bash
brew install open-horizon-labs/tap/swamp
swamp --version
```

Update with `brew upgrade swamp`. If you have a manual installation too, check `type -a swamp` to see which copy runs.

On Linux x86_64, download and verify the latest release:

```bash
curl -fLO https://github.com/open-horizon-labs/swamp/releases/latest/download/swamp-x86_64-unknown-linux-gnu.tar.gz
curl -fLO https://github.com/open-horizon-labs/swamp/releases/latest/download/swamp-x86_64-unknown-linux-gnu.tar.gz.sha256
sha256sum -c swamp-x86_64-unknown-linux-gnu.tar.gz.sha256 &&
tar -xzf swamp-x86_64-unknown-linux-gnu.tar.gz &&
mkdir -p ~/.local/bin &&
install -m 755 swamp-x86_64-unknown-linux-gnu/swamp ~/.local/bin/
~/.local/bin/swamp --version
```

Linux releases target generic x86-64 with glibc, built on Ubuntu 24.04. See [platform requirements](docs/platform.md#release-archives-and-what-they-require) and the [installation guide](docs/usage.md#installing-a-release).

### Build from source

Use the repository's pinned Rust toolchain:

```bash
git clone https://github.com/open-horizon-labs/swamp
cd swamp
cargo build --release --locked -p swamp
mkdir -p ~/.local/bin
install -m 755 target/release/swamp ~/.local/bin/
~/.local/bin/swamp --version
```

## Use it

Check the scope first, then collect an observation and open the UI:

```bash
swamp scope
swamp observe --since 24h
swamp ui
```

`swamp ui` opens on the last stored report at once and scans only when none exists; the schedule keeps it fresh and `R` refreshes on demand. It never watches the filesystem, so the header's `observed 4m ago` is the true age of what you see. Idle, it draws nothing.

Without explicit roots, Swamp uses built-in locations, enabled tool-location detectors, and your configured additions and exclusions. On macOS this includes both `~/Library/Caches` and the XDG cache root (`$XDG_CACHE_HOME`, default `~/.cache`). Tell it where your source lives with `swamp config add-root <path>`; it never guesses. Homebrew is counted by default only for developer tooling, with everything else under its prefix as one `Homebrew (other)` line. `swamp scope` explains what is included, missing, excluded, or disabled; [scope configuration](docs/usage.md#scope-and-coverage) controls it.

To work with a specific set of directories, pass the same roots to observation and reporting:

```bash
swamp observe ~/src ~/work --since 24h
swamp report ~/src ~/work --view grown --json
swamp ui ~/src ~/work
```

Start with the question you want to answer:

| Question | Where to go |
|---|---|
| Which project owns the storage, and what grew? | `1` Projects; Enter opens the selected project's folders and build groups. |
| Which tool storage should I review first? | `2` Tools opens Reclaim, largest first, with removal costs. Press `v` for Docker, Tool storage, then Agent storage. |
| Where did the rest of the disk go? | `3` Disk opens Disk usage; `v` shows Coverage gaps. |

`Tab` / `Shift-Tab` change sections; `v` changes views within a section. Arrow keys, PgUp/PgDn and Home/End navigate. `?` opens scrollable help, and the bottom row names the keys available where you are. See the [full controls](docs/usage.md#terminal-controls).

`Change 7d` names the period behind the numbers. Press `w` to choose a different period with the arrow keys, then Enter to apply or Esc to cancel. Swamp recalculates from stored observations; it does not scan your projects. If history is shorter than requested, the heading shows the available span, such as `Change ~5d`. Clearing the filter keeps the comparison period.

Press `0` to clear the initial `growth > 100MB in 7d` filter and see projects that have not grown. `/` opens the filter form; `:` edits its expression. An invalid expression stays open for correction, and Esc restores the previous filter. Saved filter and sort choices take precedence on later runs.

Tables shorten names and paths to make comparison easier; selected details and removal review retain exact paths and supporting facts. Reclaim keeps recovery costs beside sizes on narrow screens. An unread or unmeasured size is labelled, and a lower bound uses `≥`; neither becomes zero. Empty columns disappear, while a measured zero stays visible.

History starts with your first observation. To collect it while the UI is closed:

```bash
swamp schedule --every 15m
```

This uses a per-user LaunchAgent on macOS or a systemd user timer on Linux. It observes storage and can refresh GitHub context. If the store has no current format marker, normal observation automatically discards recognized incompatible derived caches and rescans; compatible history is retained. Housekeeping is limited to known Swamp-owned state and runs under the observation writer lock. On Linux, `--collector` also enables continuous change tracking between scheduled observations. `swamp schedule` shows status; `swamp schedule --off` removes the schedule.

## Review before removing

Git status, unpushed commits, cached PR information, modification age, and removal consequences sit alongside usage. These are evidence for a decision, not a universal “safe to delete” verdict.

Space marks a path or cleanup group without removing it. The marked count, selected size and Trash/permanent-removal destinations stay visible as you navigate. Backspace opens review; `A` reviews all eligible rows in the current view. A build profile selects its supported cleanup groups, not the entire profile directory. Other rows can select whole checkouts or worktrees.

Review shows the selection, restore cost, sourced last use and consequences such as open files or model layers staying behind. Repeated facts appear once. Scroll through the summary before Enter can confirm. Press `l` for every path and size, original warnings and supporting sources; inspecting these details is optional, and Enter is disabled there. Esc returns to the summary or cancels it. `b` opens blocked reasons (`d` from review), with `r` to check again. Successful moves disappear from all current views immediately; shared model layers remain. The result stays on screen until your next key.

For mise installs and simulator runtimes, Backspace on an unmarked manager row with no pending marks opens the manager's list. Enter reviews its command; `Y` runs that command permanently, without Trash recovery. Space on the same row selects its folder for the Trash flow instead.

Filesystem removals move paths to Trash. **Space is not reclaimed until Trash is emptied.** Docker image and volume removals use the daemon and are not recoverable through Trash. Anything you can see as a real folder or file can be marked, including shared stores and paths swamp has no cleanup rule for: review keeps unknown restore costs visible, with coverage and selection details under `l`. Swamp refuses a move if the path is not a real deletable folder or file, the OS denies it, the plan changed since you marked it, the ledger cannot be written or your own `swamp protect` entry covers it. See [cleanup and recovery](docs/usage.md#cleanup-and-recovery).

There is no CLI deletion command or MCP server. Cleanup is a human-confirmed TUI action; Reclaim, Tool storage and Disk moves re-check that the marked entry is unchanged at Enter, other moves do not re-derive facts between marking and confirmation.

## Use it from an agent

The CLI and [installable skill](skills/swamp/SKILL.md) let an agent gather evidence and recommend cleanup without loading a separate tool server.

```bash
npx skills add open-horizon-labs/swamp --skill swamp
```

Choose your agent; add `--global` for use across projects. This installs the skill,
not the executable. Its [installation reference](skills/swamp/references/install.md)
helps the agent install the right binary for macOS or Linux when needed.

```bash
swamp observe --since 24h
swamp report --view grown --json
```

`observe` scans and writes the store. `report` reads stored observations without recursively walking directories or launching subprocesses. JSON views expose pagination and coverage; bounded build-unit pages retain full family summaries. `inspect-cargo` provides separate, on-demand inspection when you need more detail about an existing Cargo profile.

The whole CLI is not read-only: observation, scheduling, configuration, and protection commands change their respective state. It has no command that deletes the reported storage. See the [agent interface](docs/usage.md#agent-interface) and [trust model](skills/swamp/references/trust-model.md).

## How updates stay small

Swamp folds large artifacts into directory summaries instead of keeping a permanent row for every file. After the initial walk, filesystem events identify changed containers; unchanged measurements are reused. History stores previous values as reverse deltas in compressed Parquet.

macOS FSEvents can replay changes while Swamp was closed. Linux inotify requires the opt-in collector (`swamp collect`); an uncovered interval or lost events triggers a full walk. Ordinary refreshes do not revisit every root to deduplicate hardlinks. They retain the last unique-byte estimate, explicitly marked for reconciliation; `swamp observe --full` refreshes it.

These choices make repeated observation practical without promising a fixed latency or a forensic inventory. The [architecture guide](docs/architecture.md) explains the storage model, adapter boundaries, enrichment, and remaining costs.

## Documentation

- [Why swamp exists](docs/WHY.md): the problem, the design rules and what 0.8.0 adds, each with its evidence.
- [Usage](docs/usage.md): commands, keys, configuration, build details, and recovery.
- [Architecture](docs/architecture.md): observation, folded measurement, history, and extension contracts.
- [Platforms](docs/platform.md): macOS and Linux support and limitations.
- [Build artifacts](docs/build-artifacts.md) and [agent storage](docs/agent-storage.md): checked adapter coverage.
- [Changelog](CHANGELOG.md): release changes.
- [Product](PRODUCT.md), [terminal design](DESIGN.md), and [contributing](CONTRIBUTING.md): intent and development contracts.
