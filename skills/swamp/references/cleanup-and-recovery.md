# Finding and restoring what the TUI trashed

There is no `propose`/`approve`/`execute`/`grant`/`cleanup-check` command, and no CLI deletion command. In the TUI, Space marks, Backspace opens review, and Enter moves filesystem selections to Trash after the summary has been displayed. `l` opens the optional exact path/member inventory; Esc returns to the summary or cancels from there. Docker and manager removals are permanent and have no Trash recovery. See [the trust model](trust-model.md) for those controls. This reference explains how to find a Trash move and restore it.

## Where things went

The action ledger is `ledger.parquet` in the swamp store (normally `~/.local/share/swamp`), with the reviewed facts in `ledger_facts.parquet`. A `started` record precedes the move; its final outcome records the original path, recovery location, selected bytes and time. These are Parquet tables, not a JSON-lines log.

`recovery_location` records where a Trash move went. Permanent Docker and manager removals have no Trash recovery; check the action and outcome before looking for a restore path. `grant_id` is a historical field name kept for ledger compatibility; it carries no authorization, just the constant `human-marked`.

For a Cargo group or an agent-storage session, several original paths
moved together into one **envelope** (a directory under the Trash root
holding each member plus a `restore.json` manifest: original path,
where it landed inside the envelope, byte count, and `"moved"` /
`"pending"` status per member -- `"pending"` only if a later member's
move failed partway through).

## Getting it back

- **macOS**: the Trash is `~/.Trash`, a plain rename target. Move the
  item (or the envelope's members, using `restore.json` to match each
  one back to its original path) back where it came from. For a
  removed linked worktree, also run `git worktree repair` in the
  checkout afterward (swamp does not re-run this for you).
- **Linux**: swamp lays out the freedesktop Trash spec
  (`$XDG_DATA_HOME/Trash`, defaulting to `~/.local/share/Trash`):
  `files/<name>` next to `info/<name>.trashinfo` (the original path and
  deletion time, in the spec's own format). Any spec-compliant desktop
  file manager, or the `trash` crate's `os_limited::restore_all`, reads
  and restores it correctly -- swamp's own move is exactly what such a
  reader expects, nothing bespoke.

Bytes moved to Trash stay on the same volume until the Trash itself is
emptied -- "trashed" and "freed" are different facts; do not conflate
them.
