<!-- repo: github.com/github/docs (CC-BY-4.0)
     commit: 72e940d15a9aff06b6e84216f3c97dac25c47d9b  path: content/copilot/reference/copilot-cli-reference/cli-config-dir-reference.md
     retrieved: 2026-09-22  lines: 21-22, 25-27, 34, 42-43, 327-329 -->


The `~/.copilot` directory contains the following top-level items.
|------|------|-------------|
| `agents/` | Directory | Personal custom agent definitions |
| `config.json` | File | Automatically managed application state (authentication, installed plugins, and other internal data) |
| `logs/` | Directory | Session log files |
| `session-state/` | Directory | Session history and workspace data |
| `command-history-state/` | Directory | Command history data |
<!-- ... -->
### `session-state/`

Contains session history data, organized by session ID in subdirectories. Each session directory stores an event log (`events.jsonl`) and workspace artifacts (plans, checkpoints, tracked files). This data enables session resume (`--resume` or `--continue`).
