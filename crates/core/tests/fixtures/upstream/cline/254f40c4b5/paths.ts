// repo: github.com/cline/cline (Apache-2.0)
// commit: 254f40c4b592d1e662b84f2ba06fe45dca77cab3  path: sdk/packages/shared/src/storage/paths.ts
// retrieved: 2026-09-22  lines: 34-35, 128-136, 151-160, 179-186

const DEPRECATED_CONFIG_DIR = ".clinerules";
const CLINE_CONFIG_DIR = ".cline";
// ...
let CLINE_DIR: string | undefined;
let CLINE_DIR_SET_EXPLICITLY = false;

export function setClineDir(dir: string): void {
	const trimmed = dir.trim();
	if (!trimmed) {
		return;
	}
	CLINE_DIR = trimmed;
// ...
export function resolveClineDir(): string {
	if (CLINE_DIR) {
		return CLINE_DIR;
	}
	const envDir = process.env.CLINE_DIR?.trim();
	if (envDir) {
		return envDir;
	}
	return join(HOME_DIR, ".cline");
}
// ...
export function resolveClineDataDir(): string {
	const explicitDir = process.env.CLINE_DATA_DIR?.trim();
	if (explicitDir) {
		return explicitDir;
	}
	return join(resolveClineDir(), "data");
}

