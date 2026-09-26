// repo: github.com/RooCodeInc/Roo-Code (Apache-2.0)
// commit: b867ec9145750d0ae1ff7f02d35406e9bf2a0b16  path: src/utils/storage.ts
// retrieved: 2026-09-22  lines: 50-58

/**
 * Gets the storage directory path for a task
 */
export async function getTaskDirectoryPath(globalStoragePath: string, taskId: string): Promise<string> {
	const basePath = await getStorageBasePath(globalStoragePath)
	const taskDir = path.join(basePath, "tasks", taskId)
	await fs.mkdir(taskDir, { recursive: true })
	return taskDir
}
