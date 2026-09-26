// repo: github.com/google-gemini/gemini-cli
// commit: d5b3e3accb26000d273abf16e0f1dd83aa5428a9 (2026-09-21T20:36:40Z)  path: packages/core/src/config/storage.ts
// retrieved: 2026-09-22
// --- lines 24-25 ---
const TMP_DIR_NAME = 'tmp';
const BIN_DIR_NAME = 'bin';
// --- lines 54-60: getGlobalGeminiDir ---
  static getGlobalGeminiDir(): string {
    const homeDir = homedir();
    if (!homeDir) {
      return path.join(os.tmpdir(), GEMINI_DIR);
    }
    return path.join(homeDir, GEMINI_DIR);
  }
// --- lines 84-107: isSandbox + getGlobalRuntimeDir (sandbox-exec -> ~/.cache/.gemini) ---
   */
  static isSandbox(): boolean {
    return !!process.env['SANDBOX'];
  }

  /**
   * Returns the directory for global runtime state (temp files, chat history, etc.).
   */
  static getGlobalRuntimeDir(): string {
    // Under macOS Seatbelt (sandbox-exec), writing to the user's home .gemini
    // directory is blocked by the seatbelt profile. Route runtime state to a
    // dedicated subdirectory within the permitted persistent cache directory
    // to ensure history and session state persist across CLI invocations.
    if (process.env['SANDBOX'] === 'sandbox-exec') {
      const homeDir = homedir();
      if (homeDir) {
        return path.join(homeDir, '.cache', GEMINI_DIR);
      }
    }

    // When running in a sandbox, the container launcher mounts an ephemeral
    // directory at the global gemini directory location (/home/node/.gemini).
    // For non-sandbox mode, runtime state and global config share the same path.
    return Storage.getGlobalGeminiDir();
// --- lines 195-201: getGlobalTempDir / getGlobalBinDir ---
  static getGlobalTempDir(): string {
    return path.join(Storage.getGlobalRuntimeDir(), TMP_DIR_NAME);
  }

  static getGlobalBinDir(): string {
    return path.join(Storage.getGlobalTempDir(), BIN_DIR_NAME);
  }
// --- lines 230-233: getProjectTempDir ---
  getProjectTempDir(): string {
    const identifier = this.getProjectIdentifier();
    const tempDir = Storage.getGlobalTempDir();
    return path.join(tempDir, identifier);
