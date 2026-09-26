# Finding and restoring what the TUI trashed

There is no `propose`/`approve`/`execute`/`grant`/`cleanup-check`
command any more, and no CLI command deletes anything. Deletion happens
only in the TUI (Space marks, Backspace shows current facts, Enter
moves the marked paths to the Trash) or by a human running a shell
command directly. This reference is for the read-only half of that
lifecycle: how to find what went where, and how to get it back.

## Where things went

Every Trash move appends one line to `~/.local/share/swamp/ledger.jsonl`
(a `LogFile`; override with `$SWAMP_LEDGER_PATH` for tests/CI, never for
a real move):

```json
{"id":"...","verb":"Delete","entity_id":"...","evidence":{"label":"...","bytes":123,"observed_at":...,"warnings_shown":[...]},"grant_id":"human-marked","actor":"human:tui","outcome":"completed","recovery_location":"/Users/you/.Trash/target-1700000000","measured_free_space_delta":null,"observed_path_state":"trashed","recorded_at":...}
```

`recovery_location` is where it went; `None` there means it was removed
permanently (a Docker image or volume -- Docker has no Trash, so this
is the one case with nothing to restore). `grant_id` is a historical
field name kept for ledger compatibility; it carries no authorization
any more, just the constant `human-marked`.

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
