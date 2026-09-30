# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

<!-- Schema compatibility only: the product is a macOS/Linux terminal application. The schema has no terminal value. Use terminal frames at 80×24 and 200×60 for visual review. -->

## Stack

Rust workspace. `swamp-core` owns observation, attribution, storage, and action primitives. The CLI and ratatui/crossterm TUI consume stored report facts. `skills/swamp/` documents the CLI's agent interface; there is no MCP server.

## Users

Developers with multiple projects, clones, linked worktrees, build systems, containers, and coding agents on macOS or Linux. They need to understand growth and choose what to keep without opening each project by hand.

## Product Purpose

Help a developer decide what to keep, remove, or investigate by answering: what grew, which project it belongs to, and what removal would cost.

A larger detector catalog is useful only when it improves that decision. Swamp is a developer-storage tool, not a perfect audit of every file, writer, or access.

## Operating Context

`observe`, the TUI, and optional scheduled observations measure storage and record facts. `report` reads the last stored observation; it does not refresh it. The TUI opens on the last stored report at once and scans only when no report exists yet; the schedule keeps the report current and `R` refreshes on demand. It never watches the filesystem while open. Linux can also use an opt-in collector; macOS can replay persisted FSEvents for `observe`.

The effective scope combines defaults, enabled tool-location detectors, and configured additions/exclusions. Explicit roots replace that root selection, while configured exclusions still apply. Scope and coverage must remain visible.

## Capabilities and Constraints

- Group checkouts and linked worktrees by project, including separate clones with a matching normalized Git remote.
- Model external tool homes and agent storage without assigning ambiguous ownership to a convenient project.
- Identify build units by purpose. Show age, size, and removal consequences before statistics or implementation detail.
- Keep current measurements and reverse-delta history in the existing Parquet store. Fold artifacts; request deeper inspection on demand instead of persisting an exhaustive file index.
- Keep incremental observation local to changes. Label stale unique-byte estimates until explicit reconciliation.
- Record history coverage honestly. History begins with observation and contains sizes and metadata, not recoverable contents or writer identity.
- What you see and own, you may move to Trash: cleanup is offered for every real folder or file, and what swamp does not know about it (no rule, no record of use, regeneration cost not established) is stated on the confirm. Filesystem actions use Trash; Docker image/volume deletion has no Trash recovery.
- Keep removal human-confirmed in the TUI. There is no CLI deletion or approval command. Refusals are only: not a real deletable path, the OS, a plan that changed since you marked it, an unwritable ledger, an overlap, your own protect mark.
- Distinguish rebuilding a cache from losing a conversation, checkpoint, local source change, or other unique data.

See [usage](docs/usage.md), the [trust model](skills/swamp/references/trust-model.md), and [implementation limits](docs/architecture.md#limits-of-the-current-implementation).

## Product Principles

1. Lead with the user's decision: growth, ownership, and consequences.
2. Preserve project and worktree context through drilldown.
3. Show exact action scope and recovery behavior.
4. Distinguish measurements, cached facts, inference, and unknowns.
5. Make repeated observation cheap enough that useful history accumulates.
6. Deliver each supported adapter through shared views and actions; do not imply the entire ecosystem catalog is complete.

## Evidence on Hand

Implementation, fixtures, integration tests, installed-binary trials, and native CI are the evidence. Individual timings are not general performance guarantees. The [accuracy report](docs/accuracy.md) records the release's claim checks.
