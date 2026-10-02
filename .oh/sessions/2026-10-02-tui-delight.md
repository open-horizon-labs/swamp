# Delight through control and continuity

The user asked for an Impeccable delight pass considering every TUI action, selecting the most aligned improvements and executing them. The terminal remains an Operate surface. Delight here means keeping your place and your selections while examining thousands of folders, and being able to correct a mistake without starting over.

## Action inventory and decisions

| Action family | Decision |
|---|---|
| Arrows, paging, Home/End | Add quiet position in the full current list, using the rows already built for the draw. |
| Enter/Right, Left, Esc | Keep existing tree traversal and remembered position; show a useful expand/collapse cue where it helps. |
| Tab/Shift-Tab, section digits, v | Keep existing behavior; preserve marked-items summary across views. |
| Space, A, Backspace | Keep review and execution rules; keep the pending selection count and measured size visible after the transient result clears. Distinguish Trash from permanent Docker removal. |
| Confirm, optional path inventory, cancellation | Keep existing protected review flow and accurate cancellation/result messages. No new confirmation, animation, or celebratory copy. |
| Filter form, raw expression, clear | Keep syntax and keys. Invalid Enter stays in the raw editor with the previous filter/selection active; correction or Esc recovers directly. |
| Sort/reverse | Keep stable selected identity and existing sort label; position feedback supplies orientation without a toast on every action. |
| R refresh, waiting, operation cancellation | Keep truthful progress and cancellation semantics; no staged delays or fake completion. |
| Blocked reasons and recheck | Retain blocked count and recovery access alongside pending selections; failures outrank idle feedback. |
| Cargo inspection and manager-owned removal | Preserve their separate modes; contextual cue distinguishes manager listing from the marked-set review. |
| Keep executables | Existing persistent-setting confirmation is useful; leave it intact. |
| Help and quit | Preserve familiar keys and modal precedence; no idle feedback through overlays. |

## Execution boundary

Three small systems: persistent selection orientation, row position/context, and direct filter-error recovery. Use the existing two status rows, shared count/byte formatters and incumbent terminal attributes. No new keys, dependencies, scanner work, storage format, action authority or destruction behavior. The top panel and table geometry remain fixed.

Impeccable context was loaded earlier in this session and was not rerun. The current DESIGN.md and terminal surface brief are authoritative over the cached sidecar; existing sidecar/build-path drift remains outside this task. Delight, Operate and craft-floor references were read. `ba` and `sg` are unavailable in this shell; an independent agent reviewed the action inventory and risks instead.

## Risk retirement

| Risk | Tempting wrong patch | Evidence required | Status |
|---|---|---|---|
| Pending size reads as freed space | Celebrate a selected byte total | Mixed Trash/Docker frame, measured/selected size wording, no freed-space claim | Retired: Mixed Trash/Docker frames at 40, 80 and 200 columns; the sum says selected, and immediate mark feedback now names permanent Docker removal. |
| Marks seem to disappear on navigation | Leave aggregate feedback only in a result toast | Move, page, filter and switch views with retained marks; clear last mark | Retired: Interaction frames cover paging, End, resizing, applied filters, empty results and view changes; clearing the marks removes the summary. |
| Blocked rows disappear behind encouragement | Render selection summary ahead of failure | Refusal/result/operation precedence and blocked count in idle state | Retired: Blocked count and reasons remain visible at 40 columns; operation, result, confirmation and modal tests retain their precedence. |
| Position uses only visible rows | Count the viewport | Off-screen selection, paging, resizing and full row count assertions | Retired: Paging and End assert the full 84-row fixture count at both widths; real replay names all 14,119 tree and 6,311 agent rows. |
| Extra row construction slows every draw | Call app.rows() from feedback | Pass current draw rows; copied real-store before/after replay with no probes | Retired: The current draw passes its existing rows to feedback. Copied real-store replay records zero directory listings, stats and subprocesses. |
| Filter recovery silently changes the active view | Apply invalid text, reset cursor or persist it | Invalid Enter, correction and Esc tests preserve accepted filter, cursor and marks | Retired: Invalid Enter/correction/Esc tests retain filter, marks and cursor; no invalid persistence starts. Long Unicode drafts retain their insertion caret. |
| Footer cues trigger wrong removal route | Label manager listing as Trash action | Manager row / ordinary row / existing marks / modal cases | Retired: Manager/ordinary/marked/empty-list assertions follow existing dispatch; generic footer and status cues agree. No removal rule changed. |
| Personality obscures work | Add repeated toasts, idle motion or overlays | One batched compact/wide inspection, one correction batch, one confirmation | Retired: Independent action/source review, one compact/wide first inspection and one final confirmation; only the identified caret/error-label and project-cue corrections followed. |

No outstanding ambiguity needs a user answer: the user explicitly delegated the choice of aligned touches and their implementation. The original goal remains user control and legibility. Stop if the pass requires changing action rules, inventing evidence, or adding recurring live work.

## Implementation and review

The existing two status rows now carry pending selection counts, selected stored bytes and separate Trash/permanent Docker counts. Blocked reasons retain a route back to the list. Position counts the current full list, with no extra row construction. Context hints and the key legend agree with manager routing, including the existing refusal when other items are marked. A filtered-empty view still permits review of existing marks. Quiet empty state stays quiet; no “nothing marked” filler or idle motion was added.

Rejected raw filter input stays editable, preserves accepted filter/cursor/marks, and is never persisted. Esc restores the accepted text. The error is shown in the status area, independently of draft length or a previous refusal. Long drafts scroll by grapheme so the insertion caret and typed end remain visible. Editing keys appear once in the footer.

Luna agents implemented the application and renderer changes; the root integrated and tested them. An independent reviewer found the initial manager-footer ambiguity and confirmed the corrections. The first batched fixture/real-store review identified the hidden raw-filter caret, duplicated error label and an incorrect project Enter cue; the bounded correction fixed these. The final confirmation accepted compact/wide action states, manager routing and project/tree cues. No further visual refinement followed.

Initial verification caught a test-fixture visibility error and two obsolete wording assertions, then caught the manager-footer fallback and a mistaken test assumption that a project predicate filters a drilled tree. The latter fixture now uses the filtering Build outputs view; existing filter semantics were preserved.

## Measurements

Same copied observation store, same debug replay harness, 25 draws per view on this Mac. The store was read only. These local observations are not release timing guarantees. The large-list work remains in row construction; this pass adds no filesystem or subprocess work.

| View | Rows | Before median | After median |
|---|---:|---:|---:|
| Projects | 42 | 1.34 ms | 1.40 ms |
| Project folders | 14,119 | 158 ms | 157 ms |
| Agent storage | 6,311 | 7.02 ms | 6.69 ms |
| Reclaim | 89 | 1.68 ms | 1.71 ms |

All replayed draws recorded zero directory listings, file stats and subprocesses. The small differences are within ordinary run-to-run variation, not evidence of a speedup. Captures cover all thirteen views at 80×24 and 200×60. Dedicated selection/removal frames also cover 40 columns. TestBackend buffers establish cell layout and interaction state, not every terminal font/theme’s contrast.

Evidence lives in `/Users/Shared/swamp-tui-delight`: `render-before.log`, `render-after.log`, `real-first/`, `correction-frames.log`, `final-tests.log`, `final-checks.log`, and `release-build.log`. The manual Impeccable detector returned `[]` for Rust source targets; terminal buffers and behavior tests are the applicable visual evidence. No scanner/storage change, real data removal, merge, release tag or public announcement is part of this pass.

Final normal TUI run: 321 passed, zero failed, two opt-in tests ignored by default. The copied-store render test was run explicitly and passed. Goldens were checked without regeneration in the final run. Workspace formatting, Clippy and source/static gates are recorded separately; non-TUI implementation and tests are unchanged locally.

The final `scripts/check.sh` run passed formatting, workspace Clippy, source audits, release-graph checks, named targets, gate-script tests and static checks with only the already-run test phase skipped. No unresolved implementation or visual blockers remain.
