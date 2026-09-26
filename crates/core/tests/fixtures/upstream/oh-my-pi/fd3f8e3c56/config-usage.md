# repo: https://github.com/can1357/oh-my-pi (MIT)
# commit: fd3f8e3c569b511611081e16b181b740a4c98599  committed: 2026-09-22T11:22:26Z  retrieved: 2026-09-22
# path: docs/config-usage.md  lines 62-86
- OMP native: `~/<PI_CONFIG_DIR>/agent` (normally `~/.omp/agent`; a named profile changes this as described below)
- `~/.claude`
- `~/.codex`
- `~/.gemini`

Project-level bases:

- `<cwd>/.omp`
- `<cwd>/.claude`
- `<cwd>/.codex`
- `<cwd>/.gemini`

`CONFIG_DIR_NAME` is `.omp` (`packages/utils/src/dirs.ts`). `PI_CONFIG_DIR` changes the OMP user root used by the generic helpers. `PI_CODING_AGENT_DIR` is different: for the default profile it changes `getAgentDir()` consumers such as native discovery, settings, and runtime state, but it does **not** change the generic `getConfigDirs()` / `findConfigFile()` OMP base. Named profiles ignore `PI_CODING_AGENT_DIR`.

## Profiles

A named profile (`omp --profile <name>`, `OMP_PROFILE`, or the legacy fallback `PI_PROFILE`) relocates the OMP user base. `OMP_PROFILE` wins when it is defined, including when it is explicitly empty; `default`, empty, or whitespace selects the default profile. When a profile is active, every OMP-native user-level path written here as `~/.omp/agent/...` normally resolves to `~/.omp/profiles/<name>/agent/...`. `--alias <command>` does not select a profile by itself: paired with `--profile`, it creates a shell shortcut for that profile.

The relocation is uniform across the native provider (`builtin.ts`) and the generic `config.ts` helpers, so it covers slash commands, rules, prompts, instructions, hooks, tools, extensions, settings, skills, and MCP, plus the top-level `SYSTEM.md` / `RULES.md` / `AGENTS.md` files and runtime state (sessions, blobs, `agent.db`). A profile sees only its own OMP config, never the default profile's agent config.

Keybindings are the one exception: a named profile merges the default profile's `~/.omp/agent/keybindings.*` under its own `~/.omp/profiles/<name>/agent/keybindings.*`, with the profile file overriding per binding ([#4867](https://github.com/can1357/oh-my-pi/issues/4867)). Keybindings describe the terminal/keyboard in front of the user, which doesn't change with the active profile, so user-level remaps keep working in every profile unless the profile explicitly overrides them. The inherited file is read-only for the profile process — legacy-format migration of the default profile's file only happens when the default profile itself runs.

On macOS and Linux, an existing `$XDG_DATA_HOME/omp`, `$XDG_STATE_HOME/omp`, or `$XDG_CACHE_HOME/omp` can relocate the corresponding data, state, or cache paths. For a named profile, OMP uses an XDG category only when that category already contains `omp/profiles/<name>`; otherwise that category remains under `~/.omp/profiles/<name>`. Run `omp config init-xdg` before relying on XDG paths.

The other source bases are not profile-scoped and load identically under every profile: the external-tool bases (`~/.claude`, `~/.codex`, `~/.gemini`) belong to those tools, and the project-level bases (`<cwd>/.omp`, `<cwd>/.claude`, ...) are keyed to the working directory. Throughout this document, read `~/.omp/agent` as shorthand for the active profile's agent directory unless an environment override or XDG path is being discussed.
