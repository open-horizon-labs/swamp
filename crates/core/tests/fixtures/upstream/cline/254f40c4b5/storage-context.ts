// repo: github.com/cline/cline (Apache-2.0)
// commit: 254f40c4b592d1e662b84f2ba06fe45dca77cab3  path: apps/vscode/src/shared/storage/storage-context.ts
// retrieved: 2026-09-22  lines: 83-100

 * Resolve the Cline data directory from the environment:
 * CLINE_DATA_DIR (trimmed) > CLINE_DIR + "/data" > ~/.cline/data.
 *
 * Single source of truth shared by createStorageContext and the SDK adapter's
 * legacy-state-reader, matching the SDK's own resolveClineDataDir. Every
 * reader/writer of globalState.json, secrets.json, and providers.json must
 * resolve through the same rules — diverging resolvers split provider state
 * across directories, so requests can run on a provider the settings never
 * show (ENG-2332).
 */
export function resolveDataDirFromEnv(): string {
	const envDataDir = process.env.CLINE_DATA_DIR?.trim()
	if (envDataDir) {
		return envDataDir
	}
	const clineDir = process.env.CLINE_DIR?.trim() || path.join(os.homedir(), ".cline")
	return path.join(clineDir, SETTINGS_SUBFOLDER)
}
