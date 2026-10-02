# Project comparison period

Users could see growth without knowing its duration. The filter's period could change the header without recalculating values. Project tables now show the applied period in `Change 7d`, with `w change period` beside the title. A small picker changes the period independently of the filter and saves the choice.

The core stored-report reader accepts an optional window. It recalculates project, directory, file and nested build growth from stored observations. TUI work runs off the event thread. Pending requests retain the old heading until the matching reply arrives. Replies copy only deltas onto matching current rows and allocations, preserving removed paths, sizes and selection.

Period labels use the actual available span, ending at the latest observation. Rounded spans are marked, for example `~5d`. Exact seconds stay in the filter grammar; its read-only summary uses coarse units. Clearing the filter keeps the comparison period.

A copied real store yielded different one-hour and seven-day-request deltas for 19 of 42 projects. The seven-day request was capped at 430472 seconds of history. Work counters recorded zero project directory listings, stats or subprocesses. This is stored-read correctness evidence, not a scan timing claim.

Regression coverage includes period selection and cancellation, stale replies, removed rows, actual delta changes, bounded-history labels and terminal frames at 80×24 and 200×60. Scope-aware readers and pure summary arithmetic are explicitly allowed by the source audit; scanning entry points remain restricted.
