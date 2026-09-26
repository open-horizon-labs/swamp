// repo: github.com/sst/opencode
// commit: fe3f3a41f79ad292cc3c7c629567385a20ec5130 (2026-09-21T22:51:18Z)  path: packages/opencode/src/storage/storage.ts
// retrieved: 2026-09-22
// --- lines 62-64: every storage key becomes a .json FILE ---

function file(dir: string, key: string[]) {
  return path.join(dir, ...key) + ".json"
// --- lines 182-196: Storage.migration.2 writes session_diff/<id>.json ---
  Effect.fn("Storage.migration.2")(function* (dir: string, fs: FSUtil.Interface) {
    for (const item of yield* fs.glob("session/*/*.json", {
      cwd: dir,
      absolute: true,
    })) {
      const raw = yield* fs.readJson(item)
      const session = decodeSummary(raw, { onExcessProperty: "preserve" })
      if (Option.isNone(session)) continue
      const diffs = session.value.summary.diffs
      yield* fs.writeWithDirs(
        path.join(dir, "session_diff", session.value.id + ".json"),
        JSON.stringify(diffs, null, 2),
      )
      yield* fs.writeWithDirs(
        path.join(dir, "session", session.value.projectID, session.value.id + ".json"),
// --- lines 222-224: storage root = <data>/storage ---
    const state = yield* Effect.cached(
      Effect.gen(function* () {
        const dir = path.join(Global.Path.data, "storage")
// --- lines 254-261: withResolved -> file(dir, key) ---

    const withResolved = <A, E>(
      key: string[],
      fn: (target: string, rw: TxReentrantLock.TxReentrantLock) => Effect.Effect<A, E>,
    ): Effect.Effect<A, E | FSUtil.Error> =>
      Effect.scoped(
        Effect.gen(function* () {
          const target = file((yield* state).dir, key)
