# url: https://code.claude.com/docs/en/claude-directory (fetched as .md)
# retrieval date: 2026-09-22 (docs page; no commit SHA available)
# excerpt: lines 1435, 1542, 1661 of the fetched markdown
On Windows, `~/.claude` resolves to `%USERPROFILE%\.claude`. If you set [`CLAUDE_CONFIG_DIR`](/docs/en/env-vars), every `~/.claude` path on this page lives under that directory instead.

| `todos/`, `statsig/`, `logs/`                                                                                                   | Legacy directories from older versions. No longer written. The sweep removes their contents and then the empty directory.                                                                                                                                                                                                               |

| `~/.claude/todos/`, `~/.claude/statsig/`, `~/.claude/logs/`, `~/.claude/image-cache/`                                | Nothing. Legacy directories not written by current versions.                                                                                                   |
