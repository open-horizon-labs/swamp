# url: https://docs.devin.ai/desktop/devin-desktop-faq (fetched as .md)
# retrieval date: 2026-09-22 (docs page; no commit SHA)
# excerpt: lines 187-219 of the fetched markdown
### Per-user IDE data (settings, extensions, workspaces)

This is the VS Code-derived user data directory. If the new path exists, we will read from it:

| OS      | Legacy path (read)                        | New path (read + write)                |
| ------- | ----------------------------------------- | -------------------------------------- |
| macOS   | `~/Library/Application Support/Windsurf/` | `~/Library/Application Support/Devin/` |
| Windows | `%APPDATA%\Windsurf\`                     | `%APPDATA%\Devin\`                     |
| Linux   | `~/.config/Windsurf/`                     | `~/.config/Devin/`                     |

Contains: `User/settings.json`, `User/keybindings.json`, `User/snippets/`, `globalStorage/`, `Workspaces/`, `argv.json`

### Per-user extensions directory

Extensions are stored in the dot-folder derived from the product name. Legacy paths will remain read-only:

| Legacy path (read)        | New path (read + write) |
| ------------------------- | ----------------------- |
| `~/.windsurf/extensions/` | `~/.devin/extensions/`  |

### Per-user configuration directory

The primary user-level configuration directory stores user settings, MCP config, global skills, and workflows:

| Purpose          | Path                                    |
| ---------------- | --------------------------------------- |
| User settings    | `~/.codeium/user_settings.pb`           |
| MCP config       | `~/.codeium/mcp_config.json`            |
| Global workflows | `~/.codeium/windsurf/global_workflows/` |
| Global skills    | `~/.codeium/windsurf/skills/`           |
| CLI binaries     | `~/.codeium/windsurf/bin/`              |

<Note>The `~/.codeium/` directory structure is not changing in this release. These paths remain the same.</Note>
