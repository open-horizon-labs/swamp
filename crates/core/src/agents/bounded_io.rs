//! The one way an agent adapter may read file contents
//! (`.oh/guardrails/agent-adapters-read-bounded-headers-only.md`).
//!
//! Privacy is a hard contract in this project: nothing may read past a
//! session's first line, and no path's *contents* may reach an
//! `AgentUnit` field, a log, an error string or a test fixture. Today
//! that holds because each of fifteen adapters was written carefully.
//! One `fs::read_to_string` in a sixteenth -- the obvious way to parse a
//! small JSON config -- would pull an entire conversation transcript
//! into memory, and from there into a serde error message.
//!
//! So content access is funnelled through here, with a hard cap, and the
//! adapter guardrail forbids every other route. The cap is also what
//! makes identification cost *provable*: header reads are the per-session
//! cost, so they have to be both small and counted
//! (`crate::work_counters`, which the incremental-measurement tests read).

use std::path::Path;

/// The hard ceiling on one header read. A session header is a single
/// JSON line; 64 KiB is generous for that and far below any transcript.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Reads at most `min(max_bytes, MAX_HEADER_BYTES)` from the start of
/// `path` and returns it as lossy UTF-8.
///
/// Callers parse a *header* out of this -- the first line, or the first
/// line after a fixed-width slot. Nothing here reads to the end of a
/// file, and nothing returns a handle a caller could read further from.
pub fn read_header(path: &Path, max_bytes: usize) -> Option<String> {
    let cap = crate::fs_gate::read::BoundedCap::header_at_most(max_bytes.min(MAX_HEADER_BYTES));
    crate::fs_gate::read::bounded_read_header(path, cap)
        .ok()
        .map(|b| b.lossy())
}

pub use crate::fs_gate::read::{Scan, ScanOutcome};

/// Streams `path`'s header to `scan` a byte at a time and stops at the
/// byte `scan` marks done, under `min(max_bytes, MAX_HEADER_BYTES)`.
/// For one field that sits near the start of a record whose remainder
/// is content (a Codex `session_meta`'s `cwd`, followed in the same
/// record by the project's instructions text): nothing past the field
/// is fetched, and nothing is buffered here -- `scan` keeps only what
/// it decodes.
pub fn scan_header(
    path: &Path,
    max_bytes: usize,
    scan: &mut dyn FnMut(u8) -> Scan,
) -> Option<ScanOutcome> {
    let cap = crate::fs_gate::read::BoundedCap::header_at_most(max_bytes.min(MAX_HEADER_BYTES));
    crate::fs_gate::read::bounded_scan_header(path, cap, scan).ok()
}

/// The first line of `path`'s header, which is what most adapters
/// actually want.
pub fn read_header_line(path: &Path, max_bytes: usize) -> Option<String> {
    let text = read_header(path, max_bytes)?;
    text.lines().next().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_scan_stops_at_the_byte_the_scanner_marks_done_and_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        fs::write(&p, b"abcXdef").unwrap();
        let mut seen = Vec::new();
        let out = scan_header(&p, 64, &mut |b| {
            seen.push(b);
            if b == b'X' { Scan::Done } else { Scan::More }
        })
        .unwrap();
        assert_eq!(
            out,
            ScanOutcome {
                bytes_read: 4,
                done: true
            }
        );
        assert_eq!(seen, b"abcX");

        let mut n = 0usize;
        let out = scan_header(&p, 2, &mut |_| {
            n += 1;
            Scan::More
        })
        .unwrap();
        assert_eq!(
            out,
            ScanOutcome {
                bytes_read: 2,
                done: false
            }
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn a_read_never_exceeds_the_hard_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("transcript.jsonl");
        // A file far larger than the cap, as a real transcript is.
        fs::write(&p, vec![b'a'; MAX_HEADER_BYTES * 4]).unwrap();
        let got = read_header(&p, usize::MAX).expect("read");
        assert_eq!(
            got.len(),
            MAX_HEADER_BYTES,
            "a caller asking for everything still gets at most the cap"
        );
    }

    #[test]
    fn only_the_first_line_is_returned_as_a_header_line() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.jsonl");
        fs::write(&p, b"{\"cwd\":\"/x\"}\nSECRET-SECOND-LINE\n").unwrap();
        let line = read_header_line(&p, 8192).expect("read");
        assert!(line.contains("/x"));
        assert!(
            !line.contains("SECRET-SECOND-LINE"),
            "a header read must not carry the body: {line}"
        );
    }

    #[test]
    fn every_read_is_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.jsonl");
        fs::write(&p, b"{}").unwrap();
        let (_, counted) = crate::work_counters::measured(|| read_header(&p, 8192));
        assert!(
            counted.header_bytes_read > 0,
            "header reads must be counted, or \"zero header reads\" is unprovable"
        );
    }

    #[test]
    fn a_missing_file_is_none_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_header(&tmp.path().join("nope"), 8192).is_none());
    }
}
