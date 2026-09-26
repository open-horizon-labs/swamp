// repo: github.com/RooCodeInc/Roo-Code (Apache-2.0)
// commit: b867ec9145750d0ae1ff7f02d35406e9bf2a0b16  path: src/core/task-persistence/TaskHistoryStore.ts
// retrieved: 2026-09-22  lines: 22-26, 146-150

 *
 * Each task's HistoryItem is stored as an individual JSON file in its
 * existing task directory (`globalStorage/tasks/<taskId>/history_item.json`).
 * A single index file (`globalStorage/tasks/_index.json`) is maintained
 * as a cache for fast list reads at startup.
// ...
	 * Get history items filtered by workspace path.
	 */
	getByWorkspace(workspace: string): HistoryItem[] {
		return this.getAll().filter((item) => item.workspace === workspace)
	}
