// repo: github.com/cline/cline (Apache-2.0)
// commit: 254f40c4b592d1e662b84f2ba06fe45dca77cab3  path: apps/vscode/src/shared/HistoryItem.ts
// retrieved: 2026-09-22  lines: 1-16

export type HistoryItem = {
	id: string
	ulid?: string // ULID for better tracking and metrics
	ts: number
	task: string
	tokensIn: number
	tokensOut: number
	cacheWrites?: number
	cacheReads?: number
	totalCost: number

	size?: number
	cwdOnTaskInitialization?: string
	conversationHistoryDeletedRange?: [number, number]
	isFavorited?: boolean

