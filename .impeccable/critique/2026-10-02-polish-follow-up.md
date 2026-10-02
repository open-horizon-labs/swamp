# CLI and TUI polish follow-up

Method: dual-agent assessment (A: dissent_review; B: storage_code), followed by implementation and a separate read-only correctness review. Assessment A was completed before the parent read Assessment B. The original target slug is `crates-tui-src-ui-rs`; the original 32/40 score describes the pre-change interface, not a claimed post-change score.

The core problems were misleading per-root progress, an overfull Trash confirmation, too much fixed chrome on short terminals, and inconsistent human-readable values. The empty Rust detector result did not contradict these findings: they depended on runtime state, terminal width, and the user's task rather than web-style source rules.

## Changes

- Observe presents one elapsed clock and the active operation, throttled to one redraw per second. The default completion gives readable counts and coverage; `--verbose` retains precise diagnostics. Progress is cleared before the completion is printed.
- Human output uses shared SI byte formatting, grouped counts, minute-resolution UTC timestamps, whole-second durations, and coarse ages. JSON and stored numeric fields retain their precision. Totals use exact stored bytes; independently rounded visible rows need not sum exactly.
- Trash review groups destination totals and common warnings, with item-specific exceptions. Every path and size remains available in an optional scrollable inventory. The primary summary must be displayed before Enter acts; visiting every inventory line is not required. Changed plans and terminal sizes require the current summary to be displayed again. Paths retain repeated spaces and Unicode graphemes. The footer names the action and keeps cancellation available.
- At 24 terminal rows, the headline occupies two rows and retains measurement context. Help begins with common tasks. The visible view names now say Dependencies, Ecosystems, and Folder types.
- Reclaim starts with compact context and critical accounting warnings. Detailed disk breakdowns remain in the default report and Disk view. Denied overview groups read `not measured`; groups with known bytes and gaps say `at least … + not measured`.
- Hugging Face folder rows separate the selected folder's physical size from model bytes shared through the blob store. The selected-folder action does not charge shared blobs to that folder.

## Evidence and limits

Assessment B ran the Impeccable CLI detector once and returned zero findings. No ignore list existed. The native Cua Driver observation attempt failed because its daemon was not running; no browser overlay, GUI screenshot, or live accessibility result is claimed. No visualization server or desktop window was started. Review instead used the actual Ratatui TestBackend renderings at 80×24 and 200×60, narrow and long-content behavioral fixtures, and read-only CLI captures against a copied store. Temporary captures and logs are retained under `/Users/Shared/swamp-perf-sprint` as local verification evidence.

Independent code review found no new filesystem work in stored-report rendering and no loss of exact JSON accounting. The original critique backlog was automatically closed by the storage helper because the target changed; its history remains available.

Questions skipped: the user explicitly requested implementation of the critique, polish, and clarity fixes.

Final runtime checks included the real 499-action selection at 80×24 and 120×30, 61 read-only CLI captures, a full-observation PTY run, and a held-lock PTY check. Preparation fell from 15.694 seconds to 0.266 seconds in the proposal replay after removing payload content hashing; full UI marking took 0.925–0.960 seconds. The real confirmation fits at 80×24 without opening the inventory. Optional absent tool locations are summarized, and duplicate quarantine warnings are removed from default completion output.
