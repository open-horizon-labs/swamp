// repo: github.com/google-gemini/gemini-cli
// commit: d5b3e3accb26000d273abf16e0f1dd83aa5428a9  path: packages/core/src/config/projectRegistry.ts lines 306-318, 414-420
// retrieved: 2026-09-22
    existingMappings: Record<string, string>,
  ): Promise<string> {
    const baseName = path.basename(projectPath) || 'project';
    const slug = this.slugify(baseName);

    let counter = 0;
    const existingIds = new Set(Object.values(existingMappings));

    while (true) {
      const candidate = counter === 0 ? slug : `${slug}-${counter}`;
      counter++;

      // Check if taken in registry
// ---
  private slugify(text: string): string {
    return (
      text
        .toLowerCase()
        .replace(/[^a-z0-9]/g, '-')
        .replace(/-+/g, '-')
        .replace(/^-|-$/g, '') || 'project'
    );
  }
