# Surface: terminal UI

Command: `swamp ui <root>`. Mode: Operate.

Users inspect disk growth by project, worktree, and artifact, then select units and review an inline confirmation before acting. Filesystem paths move to Trash; Docker images and volumes are removed through the daemon. Report results distinguish removed bytes from measured free-space change.

Use [DESIGN.md](../../DESIGN.md) for the current display contract and [usage](../../docs/usage.md#terminal-controls) for keys. Check committed frames at 80×24 and 200×60.

Preserve the common Report model, visible warnings and recovery behavior, execution checks, and coverage notes. Use a table and tree with signed growth and diverging bars. Help and the filter form use overlays; action confirmation uses a compact summary overlay with optional details retaining all paths and original evidence. Routine adapter and selection explanations belong in details; restore cost, sourced last use, shared payloads and consequential exceptions lead the summary. Unrecognized warnings stay visible. Expanding/collapsing a tree keeps selection identity and screen position.
