//! Neutral JSONL session-header *mechanics* shared by the Pi-family
//! adapters (`crate::agents::pi`, `crate::agents::oh_my_pi`).
//!
//! Oh My Pi is a fork of Pi, so the two tools' session files have
//! overlapping — but *not* identical — on-disk shapes: Oh My Pi's
//! documented format puts a fixed-width title slot ahead of the JSON
//! header line, Pi's own README documents the header at byte offset 0.
//!
//! Neither adapter may reach into the other to borrow that parsing
//! (`.oh/guardrails/agent-adapters-are-pluggable.md`: an adapter names
//! no other adapter, so a change to one tool's format can never silently
//! change another tool's identification). This module is therefore
//! *tool-agnostic*: it knows how to find a header line at a byte offset
//! and how to pull the declared `cwd`/`additionalDirectories` out of it,
//! and nothing about which tool documents which shape. Each adapter
//! passes the layouts **its own** tool documents, and a file matching
//! none of them stays unknown-format — never a guess, and never
//! "whatever the sibling tool does".
//!
//! This module is not an adapter: it declares no `Adapter` type, carries
//! no tool id, and builds no unit. It does no directory traversal and
//! reads nothing itself — every read here goes through
//! [`super::IdentifyCtx::derived`], so an unchanged session costs zero
//! header bytes on a second pass.

use super::IdentifyCtx;
use std::path::Path;

/// How many bytes past any title slot a header read may consume. Header
/// reads are bounded by construction: an adapter never reads past its
/// session's first line, and never reads message bodies at all.
pub const HEADER_READ_BYTES: usize = 8192;

/// The fixed-width title slot the `AfterTitleSlot` layout skips.
pub const TITLE_SLOT_BYTES: usize = 256;

/// Separates the fields of a cached derived value. A declared path can
/// contain almost anything, but never a C0 control byte, so this can
/// never collide with the data it joins.
const FIELD_SEP: char = '\u{1}';

/// One on-disk header layout an adapter is willing to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderLayout {
    /// The JSON header is the file's first line, at byte offset 0.
    OffsetZero,
    /// The JSON header follows a fixed-width title slot.
    AfterTitleSlot,
}

impl HeaderLayout {
    pub fn describe(self) -> &'static str {
        match self {
            Self::OffsetZero => "a JSON header at byte offset 0",
            Self::AfterTitleSlot => "a JSON header after a 256-byte title slot",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::OffsetZero => "0",
            Self::AfterTitleSlot => "t",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "0" => Some(Self::OffsetZero),
            "t" => Some(Self::AfterTitleSlot),
            _ => None,
        }
    }

    /// The header line this layout expects inside one bounded read's
    /// text, or `None` when the read is too short to contain one.
    ///
    /// The byte offset is applied to the text's own bytes and nudged
    /// forward to the next character boundary, so a slot holding
    /// multi-byte characters shifts the header line rather than
    /// discarding it.
    pub fn header_line(self, text: &str) -> Option<&str> {
        match self {
            Self::OffsetZero => text.lines().next(),
            Self::AfterTitleSlot => {
                if text.len() <= TITLE_SLOT_BYTES {
                    return None;
                }
                let mut at = TITLE_SLOT_BYTES;
                while at < text.len() && !text.is_char_boundary(at) {
                    at += 1;
                }
                text.get(at..)?.lines().next()
            }
        }
    }
}

/// What one bounded header read yielded. Contents beyond these declared
/// fields are never retained: nothing here returns message text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionHeader {
    pub cwd: Option<String>,
    pub additional_directories: Vec<String>,
    /// Which of the caller's accepted layouts actually matched.
    pub layout: Option<HeaderLayout>,
}

impl SessionHeader {
    pub fn is_empty(&self) -> bool {
        self.cwd.is_none() && self.additional_directories.is_empty()
    }
}

/// The declared-directory pull: `cwd` plus `additionalDirectories` out
/// of one JSON object line. Tool-agnostic — it names no tool and no
/// layout, only the field names both Pi-family formats document.
pub fn parse_line(line: &str) -> Option<SessionHeader> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let cwd = value
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let additional_directories = value
        .get("additionalDirectories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if cwd.is_none() && additional_directories.is_empty() {
        return None;
    }
    Some(SessionHeader {
        cwd,
        additional_directories,
        layout: None,
    })
}

/// Parses one bounded read's `text` against each of `layouts`, in the
/// caller's own order. The adapter decides which layouts its tool
/// documents; this function only knows how to look.
pub fn parse_header(text: &str, layouts: &[HeaderLayout]) -> SessionHeader {
    for layout in layouts {
        let Some(line) = layout.header_line(text) else {
            continue;
        };
        if let Some(mut header) = parse_line(line) {
            header.layout = Some(*layout);
            return header;
        }
    }
    SessionHeader::default()
}

/// A [`SessionHeader`] as one cacheable string, or `None` for "this
/// file declares nothing" (which [`super::IdentifyCtx::derived`] caches
/// too, so an absent field is not re-read forever).
pub fn encode_header(header: &SessionHeader) -> Option<String> {
    if header.is_empty() {
        return None;
    }
    let mut fields = vec![
        header
            .layout
            .map(HeaderLayout::tag)
            .unwrap_or("?")
            .to_string(),
        header.cwd.clone().unwrap_or_default(),
    ];
    fields.extend(header.additional_directories.iter().cloned());
    Some(fields.join(&FIELD_SEP.to_string()))
}

/// The inverse of [`encode_header`]. An empty or malformed value decodes
/// to an empty header rather than a panic: a cached string is data.
pub fn decode_header(encoded: &str) -> SessionHeader {
    let mut fields = encoded.split(FIELD_SEP);
    let layout = fields.next().and_then(HeaderLayout::from_tag);
    let cwd = fields.next().filter(|s| !s.is_empty()).map(str::to_string);
    let additional_directories: Vec<String> = fields
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    SessionHeader {
        cwd,
        additional_directories,
        layout,
    }
}

/// One bounded, **cached** header read for `path`, tried against exactly
/// the layouts the calling adapter's own tool documents.
///
/// `adapter_id`/`kind` are the caller's own, so two adapters (or two
/// derivations of the same adapter) never share a cache entry, and an
/// unchanged session file is read at most once per
/// `(size, mtime, adapter version)`.
pub fn derived_header(
    ctx: &IdentifyCtx,
    adapter_id: &str,
    kind: &str,
    path: &Path,
    max_bytes: usize,
    layouts: &[HeaderLayout],
) -> SessionHeader {
    ctx.derived(adapter_id, kind, path, max_bytes, &|text| {
        encode_header(&parse_header(text, layouts))
    })
    .map(|encoded| decode_header(&encoded))
    .unwrap_or_default()
}

/// The honest `Unresolved` reason when no accepted layout matched:
/// names every layout that was checked, so "we could not tell" never
/// reads like "there is nothing there".
pub fn no_layout_matched_reason(layouts: &[HeaderLayout]) -> String {
    let checked: Vec<&str> = layouts.iter().map(|l| l.describe()).collect();
    format!(
        "no cwd field found; checked {} (unknown-format for this session)",
        checked.join(" then ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::IdentificationCache;
    use std::fs;

    fn tmp_with(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        fs::write(&p, bytes).unwrap();
        (d, p)
    }

    fn read(path: &Path, layouts: &[HeaderLayout]) -> SessionHeader {
        let cache = IdentificationCache::disabled();
        let ctx = IdentifyCtx::new(1, &cache);
        derived_header(
            &ctx,
            "t",
            "header",
            path,
            TITLE_SLOT_BYTES + HEADER_READ_BYTES,
            layouts,
        )
    }

    #[test]
    fn offset_zero_layout_is_read_when_the_adapter_accepts_it() {
        let (_d, p) = tmp_with(br#"{"cwd":"/tmp/x"}"#);
        let h = read(&p, &[HeaderLayout::OffsetZero]);
        assert_eq!(h.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(h.layout, Some(HeaderLayout::OffsetZero));
    }

    #[test]
    fn an_adapter_that_only_accepts_the_title_slot_layout_does_not_read_offset_zero() {
        // The tempting shortcut this rejects: "both tools are similar,
        // so try everything". An adapter gets exactly the layouts its
        // own tool documents; anything else stays unknown-format.
        let (_d, p) = tmp_with(br#"{"cwd":"/tmp/x"}"#);
        let h = read(&p, &[HeaderLayout::AfterTitleSlot]);
        assert!(h.is_empty());
        assert_eq!(h.layout, None);
    }

    #[test]
    fn an_adapter_that_only_accepts_offset_zero_does_not_read_a_title_slot_file() {
        // The mirror image, and the one that matters for section 13: an
        // adapter accepting only the offset-zero shape must find nothing
        // in a file whose first line is a *different* tool's fixed-width
        // title record, rather than silently understanding it.
        let mut bytes = vec![b' '; TITLE_SLOT_BYTES];
        let title = br#"{"type":"title"}"#;
        bytes[..title.len()].copy_from_slice(title);
        bytes[TITLE_SLOT_BYTES - 1] = b'\n';
        bytes.extend_from_slice(br#"{"cwd":"/tmp/y"}"#);
        let (_d, p) = tmp_with(&bytes);
        let h = read(&p, &[HeaderLayout::OffsetZero]);
        assert!(h.is_empty(), "{h:?}");
    }

    #[test]
    fn title_slot_layout_skips_the_slot() {
        let mut bytes = vec![b' '; TITLE_SLOT_BYTES];
        bytes.extend_from_slice(br#"{"cwd":"/tmp/y","additionalDirectories":["/tmp/z"]}"#);
        let (_d, p) = tmp_with(&bytes);
        let h = read(&p, &[HeaderLayout::AfterTitleSlot]);
        assert_eq!(h.cwd.as_deref(), Some("/tmp/y"));
        assert_eq!(h.additional_directories, vec!["/tmp/z".to_string()]);
    }

    #[test]
    fn nothing_parseable_names_every_layout_it_checked() {
        let (_d, p) = tmp_with(b"not json at all");
        let h = read(
            &p,
            &[HeaderLayout::OffsetZero, HeaderLayout::AfterTitleSlot],
        );
        assert!(h.is_empty());
        let reason =
            no_layout_matched_reason(&[HeaderLayout::OffsetZero, HeaderLayout::AfterTitleSlot]);
        assert!(reason.contains("byte offset 0"));
        assert!(reason.contains("title slot"));
        assert!(reason.contains("unknown-format"));
    }

    #[test]
    fn a_header_round_trips_through_the_cache_encoding() {
        let header = SessionHeader {
            cwd: Some("/tmp/a".to_string()),
            additional_directories: vec!["/tmp/b".to_string(), "/tmp/c".to_string()],
            layout: Some(HeaderLayout::AfterTitleSlot),
        };
        let encoded = encode_header(&header).expect("declares something");
        assert_eq!(decode_header(&encoded), header);
        assert_eq!(encode_header(&SessionHeader::default()), None);
        assert!(decode_header("").is_empty());
    }

    #[test]
    fn a_body_past_the_header_line_is_never_returned() {
        let canary = "CANARY-PI-FAMILY-DO-NOT-LEAK-4a10";
        let (_d, p) =
            tmp_with(format!("{{\"cwd\":\"/tmp/x\"}}\n{{\"content\":\"{canary}\"}}\n").as_bytes());
        let h = read(&p, &[HeaderLayout::OffsetZero]);
        assert!(!format!("{h:?}").contains(canary));
    }

    #[test]
    fn an_unchanged_file_is_read_once_when_the_cache_is_enabled() {
        let (_d, p) = tmp_with(br#"{"cwd":"/tmp/x"}"#);
        let store = tempfile::tempdir().unwrap();
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        let layouts = [HeaderLayout::OffsetZero];
        let _ = derived_header(&ctx, "t", "header", &p, HEADER_READ_BYTES, &layouts);
        let (again, counted) = crate::work_counters::measured(|| {
            derived_header(&ctx, "t", "header", &p, HEADER_READ_BYTES, &layouts)
        });
        assert_eq!(again.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(
            counted.header_bytes_read, 0,
            "an unchanged session must cost zero header bytes on a second pass"
        );
    }
}
