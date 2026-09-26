// repo: github.com/cline/cline (Apache-2.0)
// commit: 254f40c4b592d1e662b84f2ba06fe45dca77cab3  path: apps/vscode/src/sdk/legacy-state-reader.ts
// retrieved: 2026-09-22  lines: 22-30, 42-44, 53-74, 149-153

/**
 * Resolve the Cline data directory.
 * Priority: CLINE_DATA_DIR env > CLINE_DIR env + "/data" > ~/.cline/data
 */
export function resolveDataDir(override?: string): string {
	// Delegates to the same resolver createStorageContext uses so the two
	// stores can never drift apart again (ENG-2332).
	return override || resolveDataDirFromEnv()
}
// ...
/** Path to taskHistory.json (stored in state/ subdirectory) */
function taskHistoryPath(dataDir?: string): string {
	return path.join(resolveDataDir(dataDir), "state", "taskHistory.json")
// ...
export function taskDirPath(taskId: string, dataDir?: string): string {
	return path.join(resolveDataDir(dataDir), "tasks", taskId)
}

/** Path to api_conversation_history.json for a task */
function apiConversationHistoryPath(taskId: string, dataDir?: string): string {
	return path.join(taskDirPath(taskId, dataDir), "api_conversation_history.json")
}

/** Path to ui_messages.json for a task */
function uiMessagesPath(taskId: string, dataDir?: string): string {
	return path.join(taskDirPath(taskId, dataDir), "ui_messages.json")
}

/** Path to context_history.json for a task */
function contextHistoryPath(taskId: string, dataDir?: string): string {
	return path.join(taskDirPath(taskId, dataDir), "context_history.json")
}

/** Path to task_metadata.json for a task */
function taskMetadataPath(taskId: string, dataDir?: string): string {
	return path.join(taskDirPath(taskId, dataDir), "task_metadata.json")
// ...
 * Read taskHistory.json from the state directory.
 * Returns an empty array if the file is missing or corrupt.
 */
export function readTaskHistory(dataDir?: string): HistoryItem[] {
	return readJsonFile<HistoryItem[]>(taskHistoryPath(dataDir), [])
