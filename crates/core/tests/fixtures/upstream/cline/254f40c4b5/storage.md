<!-- repo: github.com/cline/cline (Apache-2.0)
     commit: 254f40c4b592d1e662b84f2ba06fe45dca77cab3  path: .clinerules/storage.md
     retrieved: 2026-09-22  lines: 1-3, 52-64 -->

# Storage Architecture

Global settings, secrets and workspace state are stored in **file-backed JSON stores** under `~/.cline/data/`. This is the shared storage layer used by VSCode, CLI, and JetBrains.
<!-- ... lines 4-51 omitted ... -->
## File Layout

```
~/.cline/
  data/
    globalState.json          # Global settings & state
    secrets.json              # API keys (mode 0o600)
    tasks/
      taskHistory.json        # Task history (separate file)
    workspaces/
      <hash>/
        workspaceState.json   # Per-workspace toggles
```
