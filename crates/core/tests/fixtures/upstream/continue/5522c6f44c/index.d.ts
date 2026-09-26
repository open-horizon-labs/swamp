// repo: github.com/continuedev/continue (Apache-2.0)
// commit: 5522c6f44ca0ac3528b37244818fbfa39b5af470  path: core/index.d.ts
// retrieved: 2026-09-22  lines: 279-298

export interface Session {
  sessionId: string;
  title: string;
  workspaceDirectory: string;
  history: ChatHistoryItem[];
  /** Optional: per-session UI mode (chat/agent/plan/background) */
  mode?: MessageModes;
  /** Optional: title of the selected chat model for this session */
  chatModelTitle?: string | null;
  /** Optional: cumulative usage and cost for all LLM API calls in this session */
  usage?: SessionUsage;
}

export interface BaseSessionMetadata {
  sessionId: string;
  title: string;
  dateCreated: string;
  workspaceDirectory: string;
  messageCount?: number;
}
