# repo: https://github.com/earendil-works/pi (MIT)
# commit: d201760ffee16564aa8d9a759e0c85b70db33674  committed: 2026-09-22T11:56:53Z  retrieved: 2026-09-22
# path: packages/coding-agent/docs/sessions.md  lines 1-8 + 166
# Sessions

Pi saves conversations as sessions so you can continue work, branch from earlier turns, and revisit previous paths.

## Session Storage

Sessions auto-save to `~/.pi/agent/sessions/`, organized by working directory. Each session is a JSONL file with a tree structure.

...
When pi exits because of an uncaught exception or a fatal runtime error, it stores the error message and stack trace in `~/.pi/agent/crashes.json` (the newest five). The next interactive start shows a warning once; running `/bug` attaches the stored crashes to `diagnostics.json` and removes the file after the report is uploaded or exported. Resume the crashed session with `pi -r` first if you want the transcript in the report.
