//! The one way a build adapter may read file contents
//! (`.oh/guardrails/build-adapters-read-bounded-manifests-only.md`).
//!
//! The agent side has [`crate::agents::bounded_io`] with a 64 KiB header
//! cap, because a session transcript is private and only its first line
//! is ever wanted. The build side's numbers are different for a
//! different reason: a `package.json` or a `pom.xml` is read *whole*
//! (the fields an adapter wants are scattered through it), but a
//! `node_modules` tree holds one `package.json` per installed package
//! and a monorepo lockfile can be tens of megabytes. So the cap is
//! larger, the read is still bounded, and -- the part that matters --
//! every byte is counted, so "identification read N bytes" is a number a
//! test can assert rather than a claim.
//!
//! The cap is not a parse budget. A manifest larger than the cap is a
//! *truncated read*, and a caller that cannot parse the truncated text
//! must report an explicit unknown rather than guess from the prefix.

use std::path::Path;

use crate::fs_gate::read::BoundedCap;

/// The hard ceiling on one manifest read:
/// [`BoundedCap::BUILD_MANIFEST`], read through the same gate every
/// other bounded read in core goes through. For scale: a `package.json`
/// is typically 1-4 KiB, a `pom.xml` 2-30 KiB, a Cargo `.fingerprint`
/// JSON under 1 KiB. The cap is generous for all of them and far below
/// a lockfile, which no adapter here reads.
pub const MAX_MANIFEST_BYTES: usize = BoundedCap::BUILD_MANIFEST.bytes();

/// What a bounded read produced. Truncation is a first-class outcome,
/// not a silent prefix: a caller that gets [`Manifest::truncated`] set
/// and cannot parse the text owes the user an explicit unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub text: String,
    /// The same bytes, undecoded, for a format that is not text (a
    /// binary property list). Never more than the cap.
    pub raw: Vec<u8>,
    /// The file was at least as large as the cap, so `text` may end
    /// mid-token.
    pub truncated: bool,
}

/// Reads at most `min(cap, MAX_MANIFEST_BYTES)` bytes from the start of
/// `path`, through [`crate::fs_gate::read::bounded_read`] -- the same
/// gate every other content read in core goes through.
///
/// Returns `None` when the path cannot be opened or read at all, which
/// is a different fact from an empty file and callers must keep it so.
pub fn read_manifest(path: &Path, cap: usize) -> Option<Manifest> {
    let cap = cap.min(MAX_MANIFEST_BYTES);
    // The gate's own cap is the hard ceiling; a caller asking for less
    // (a test, a format known to be small) gets the read truncated to
    // its own request on top of that.
    let read = crate::fs_gate::read::bounded_read(path, BoundedCap::BUILD_MANIFEST).ok()?;
    let mut bytes = read.bytes;
    let truncated = read.truncated || bytes.len() > cap;
    if bytes.len() > cap {
        bytes.truncate(cap);
    }
    crate::work_counters::record_header_bytes(bytes.len() as u64);
    Some(Manifest {
        truncated,
        text: String::from_utf8_lossy(&bytes).into_owned(),
        raw: bytes,
    })
}

/// The shape most callers want: the whole (bounded) text, or `None`.
/// A truncated read is deliberately *not* collapsed into `Some(prefix)`
/// here -- use [`read_manifest`] and decide what a prefix means for your
/// format.
pub fn read_whole_manifest(path: &Path, cap: usize) -> Option<String> {
    let m = read_manifest(path, cap)?;
    (!m.truncated).then_some(m.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_read_never_exceeds_the_hard_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pnpm-lock.yaml");
        fs::write(&p, vec![b'a'; MAX_MANIFEST_BYTES * 3]).unwrap();
        let got = read_manifest(&p, usize::MAX).expect("read");
        assert_eq!(
            got.text.len(),
            MAX_MANIFEST_BYTES,
            "a caller asking for everything still gets at most the cap"
        );
        assert!(got.truncated, "hitting the cap is reported, not hidden");
    }

    #[test]
    fn a_truncated_manifest_is_not_offered_as_whole_text() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("package.json");
        fs::write(&p, vec![b'{'; 64]).unwrap();
        assert!(
            read_whole_manifest(&p, 16).is_none(),
            "a prefix of a manifest is not the manifest; an adapter that got one owes an \
             explicit unknown"
        );
    }

    #[test]
    fn every_read_is_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("package.json");
        fs::write(&p, b"{\"name\":\"x\"}").unwrap();
        let (_, counted) = crate::work_counters::measured(|| read_manifest(&p, 8192));
        assert!(
            counted.header_bytes_read > 0,
            "manifest reads must be counted, or \"identification read N bytes\" is unprovable"
        );
    }

    #[test]
    fn a_missing_file_is_none_not_an_empty_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_manifest(&tmp.path().join("nope.json"), 8192).is_none());
        let empty = tmp.path().join("empty.json");
        fs::write(&empty, b"").unwrap();
        assert_eq!(
            read_manifest(&empty, 8192).map(|m| m.text),
            Some(String::new()),
            "an empty file is an empty manifest, which is not the same fact as an absent one"
        );
    }
}
