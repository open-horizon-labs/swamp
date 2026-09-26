repo: github.com/github/copilot-cli
commit: c13b3dcae4f1e176c5a074c0d063a6e9f258081f  path: changelog.md
retrieved: 2026-09-22

LICENSE: LICENSE.md is the proprietary "GitHub Copilot CLI License" (no derivative
works, no modified redistribution). Prose is therefore NOT vendored. Recorded below:
line numbers + the exact path/symbol strings only.

line 3083  -> `~/.copilot/session-state`        (release 0.0.342, 2025-10-15)
line 3084  -> `~/.copilot/history-session-state` (described as the LEGACY location)
line 1589  -> `~/.copilot/settings.json`, `config.json`
line 1311  -> `~/.copilot/logs/`
line 1299  -> `--resume`, `-C <dir>`  (behavioural: sessions resume in a saved
              working directory; NO field name is given)
line 3087  -> `~/.copilot/config`, `log_level`

grep counts over changelog.md (3236 lines):
  "cwd"              -> 0 occurrences as a session-state field name
  "workspaceFolder"  -> 0 occurrences
  "workingDirectory" -> 0 occurrences
