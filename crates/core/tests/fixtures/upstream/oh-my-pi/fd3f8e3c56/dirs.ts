# repo: https://github.com/can1357/oh-my-pi (MIT)
# commit: fd3f8e3c569b511611081e16b181b740a4c98599  committed: 2026-09-22T11:22:26Z  retrieved: 2026-09-22
# path: packages/utils/src/dirs.ts  lines 21-24, 294-296, 579-602, 888-952
export const APP_NAME: string = "omp";

/** Config directory name (e.g. ".omp") */
export const CONFIG_DIR_NAME: string = ".omp";
export function getConfigDirName(): string {
	return process.env.PI_CONFIG_DIR || CONFIG_DIR_NAME;
}
}
/** Get the agent config directory (~/.omp/agent). */
export function getAgentDir(): string {
	return dirs.agentDir;
}

/** Get the project-local config directory (.omp). */
export function getProjectAgentDir(cwd: string = getProjectDir()): string {
	return path.join(cwd, CONFIG_DIR_NAME);
}

// =============================================================================
// Config-root subdirectories (~/.omp/*)
// =============================================================================

/** Get the reports directory (~/.omp/reports). */
export function getReportsDir(): string {
	return dirs.rootSubdir("reports", "state");
}

/** Get the logs directory (~/.omp/logs). */
export function getLogsDir(): string {
	return dirs.rootSubdir("logs", "state");
}

/** Get the sessions directory (~/.omp/agent/sessions). */
export function getSessionsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "sessions", "data");
}

/** Get the content-addressed blob store directory (~/.omp/agent/blobs). */
export function getBlobsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "blobs", "data");
}

/** Get the custom themes directory (~/.omp/agent/themes). */
export function getCustomThemesDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "themes");
}

/** Get the tools directory (~/.omp/agent/tools). */
export function getToolsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "tools");
}

/** Get the slash commands directory (~/.omp/agent/commands). */
export function getCommandsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "commands");
}

/** Get the prompts directory (~/.omp/agent/prompts). */
export function getPromptsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "prompts");
}

/** Get the user-level Python modules directory (~/.omp/agent/modules). */
export function getAgentModulesDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "modules");
}

/** Get the memories directory (~/.omp/agent/memories). */
export function getMemoriesDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "memories", "state");
}

/** Get the terminal sessions directory (~/.omp/agent/terminal-sessions). */
export function getTerminalSessionsDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "terminal-sessions", "state");
}

/**
 * Get the persistent registry of custom session files
 * (~/.omp/agent/custom-session-files). Each `--session-dir`/`--session`
 * transcript is recorded here as one marker file so storage GC can scan its
 * exact path after its terminal breadcrumb is overwritten by a later session.
 */
export function getCustomSessionFilesDir(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "custom-session-files", "state");
}

/** Get the crash log path (~/.omp/agent/omp-crash.log). */
export function getCrashLogPath(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, "omp-crash.log", "state");
}

/** Get the debug log path (~/.omp/agent/omp-debug.log). */
export function getDebugLogPath(agentDir?: string): string {
	return dirs.agentSubdir(agentDir, `${APP_NAME}-debug.log`, "state");
}
