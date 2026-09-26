// repo: github.com/continuedev/continue (Apache-2.0)
// commit: 5522c6f44ca0ac3528b37244818fbfa39b5af470  path: core/util/paths.ts
// retrieved: 2026-09-22  lines: 27-35, 69-72, 78-80, 102-108

const CONTINUE_GLOBAL_DIR = (() => {
  const configPath = process.env.CONTINUE_GLOBAL_DIR;
  if (configPath) {
    // Convert relative path to absolute paths based on current working directory
    return path.isAbsolute(configPath)
      ? configPath
      : path.resolve(process.cwd(), configPath);
  }
  return path.join(os.homedir(), ".continue");
// ...
export function getContinueGlobalPath(): string {
  // This is ~/.continue on mac/linux
  const continuePath = CONTINUE_GLOBAL_DIR;
  if (!fs.existsSync(continuePath)) {
// ...
export function getSessionsFolderPath(): string {
  const sessionsPath = path.join(getContinueGlobalPath(), "sessions");
  if (!fs.existsSync(sessionsPath)) {
// ...
export function getSessionFilePath(sessionId: string): string {
  return path.join(getSessionsFolderPath(), `${sessionId}.json`);
}

export function getSessionsListPath(): string {
  const filepath = path.join(getSessionsFolderPath(), "sessions.json");
  if (!fs.existsSync(filepath)) {
