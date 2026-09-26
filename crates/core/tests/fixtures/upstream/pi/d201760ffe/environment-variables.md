# repo: https://github.com/earendil-works/pi (MIT)
# commit: d201760ffee16564aa8d9a759e0c85b70db33674  committed: 2026-09-22T11:56:53Z  retrieved: 2026-09-22
# path: packages/coding-agent/docs/environment-variables.md  lines 76-83

These variables are read by Pi itself:

| Variable | Description |
|----------|-------------|
| `PI_CODING_AGENT_DIR` | Override the config directory; default is `~/.pi/agent` |
| `PI_CODING_AGENT_SESSION_DIR` | Override session storage; overridden by `--session-dir` |
| `PI_PACKAGE_DIR` | Override the package directory, useful for Nix/Guix store paths |
