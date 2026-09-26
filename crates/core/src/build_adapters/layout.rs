//! Neutral parsing shared by several build adapters: names, `key=value`
//! properties, property lists, timestamps.
//!
//! Like [`super::jvm_common`], an allow-listed helper of
//! `.oh/guardrails/build-adapters-are-pluggable.md`: nothing here knows
//! which adapter called it, and nothing here reads a file. Every
//! function is arithmetic over bytes an adapter already read through
//! [`super::bounded_io`] or names it already has from the folded rows.

use std::collections::HashMap;
use std::path::Path;

/// A path's final component, or `""`.
pub fn name_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
}

/// `KEY=value` (or `key = value`, or `key: value` when `colon`) lines,
/// comments and blank lines skipped. For `pyvenv.cfg`, Android's
/// `source.properties` and `config.ini`, Go's `trim.txt` neighbours.
pub fn properties(text: &str, colon: bool) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let split = line
            .split_once('=')
            .or_else(|| colon.then(|| line.split_once(':')).flatten());
        if let Some((k, v)) = split {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// RFC 822-style headers (`Name: x`, `Version: 1.0`) up to the first
/// blank line: a Python `PKG-INFO`/`METADATA`.
pub fn headers(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        }
    }
    out
}

/// Whether a token starts like a version (`1`, `v2.3`, `3.11.4`).
pub fn versionish(s: &str) -> bool {
    let s = s.strip_prefix('v').unwrap_or(s);
    s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Splits `name-1.2.3` at the last `-` followed by a version-looking
/// token. `None` when there is no such split: an identity is never
/// invented from a name with no version in it.
pub fn split_name_version(s: &str) -> Option<(&str, &str)> {
    let (name, version) = s.rsplit_once('-')?;
    (!name.is_empty() && versionish(version)).then_some((name, version))
}

/// `2024-01-15T10:32:00Z` / `2024-01-15T10:32:00.123456789+02:00` /
/// `2024-01-15 10:32:00 +0000 UTC` to Unix seconds. `None` for anything
/// else: a timestamp a manager reported in a shape this does not read is
/// an unknown time, never a guessed one.
pub fn rfc3339_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, min, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Offset: `Z`, `+hh:mm`, `-hh:mm`, `+hhmm` (after any fraction).
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let rest = rest.trim_start();
    let offset_secs: i64 = match rest.chars().next() {
        Some('+') | Some('-') => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            let hh: i64 = digits.get(0..2)?.parse().ok()?;
            let mm: i64 = digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
            sign * (hh * 3600 + mm * 60)
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let t = days * 86_400 + hour * 3600 + min * 60 + sec - offset_secs;
    (t >= 0).then_some(t as u64)
}

// ---------------------------------------------------------------------
// Property lists
// ---------------------------------------------------------------------

/// A scalar a property list holds at `path` (a chain of dictionary
/// keys): a string, or an integer rendered in decimal. Reads both the
/// XML and the binary (`bplist00`) encodings, because Xcode writes both.
/// `None` for a missing key, a non-scalar, or bytes that are not a
/// property list -- never a partial guess.
pub fn plist_scalar(raw: &[u8], path: &[&str]) -> Option<String> {
    if raw.starts_with(b"bplist00") {
        return bplist::scalar(raw, path);
    }
    xml_plist_scalar(&String::from_utf8_lossy(raw), path)
}

/// The XML encoding, by a small tokenizer over the elements a plist
/// uses. Nested dictionaries are followed key by key.
fn xml_plist_scalar(text: &str, path: &[&str]) -> Option<String> {
    let start = text.find("<dict>")?;
    let mut rest = &text[start + "<dict>".len()..];
    for (depth, key) in path.iter().enumerate() {
        let last = depth + 1 == path.len();
        // Walk this dict's keys at its own nesting level.
        let mut level = 0usize;
        let mut cursor = rest;
        loop {
            let open = cursor.find('<')?;
            cursor = &cursor[open..];
            if cursor.starts_with("</dict>") {
                if level == 0 {
                    return None;
                }
                level -= 1;
                cursor = &cursor["</dict>".len()..];
                continue;
            }
            if cursor.starts_with("<dict>") {
                level += 1;
                cursor = &cursor["<dict>".len()..];
                continue;
            }
            if cursor.starts_with("<key>") && level == 0 {
                let end = cursor.find("</key>")?;
                let k = &cursor["<key>".len()..end];
                cursor = &cursor[end + "</key>".len()..];
                if unescape(k) != *key {
                    continue;
                }
                let v = cursor.trim_start();
                if last {
                    for tag in ["string", "integer", "real", "date"] {
                        let o = format!("<{tag}>");
                        let c = format!("</{tag}>");
                        if let Some(body) = v.strip_prefix(o.as_str()) {
                            let e = body.find(c.as_str())?;
                            return Some(unescape(&body[..e]));
                        }
                    }
                    if v.starts_with("<true/>") {
                        return Some("true".into());
                    }
                    if v.starts_with("<false/>") {
                        return Some("false".into());
                    }
                    return None;
                }
                let inner = v.strip_prefix("<dict>")?;
                rest = inner;
                break;
            }
            // Any other tag: skip past it.
            let close = cursor.find('>')?;
            cursor = &cursor[close + 1..];
        }
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

mod bplist {
    //! The binary property list encoding, as far as a dictionary of
    //! scalars needs: the trailer, the offset table, dictionaries,
    //! ASCII/UTF-16 strings and integers. Anything else is `None`.

    struct Doc<'a> {
        raw: &'a [u8],
        offsets: Vec<usize>,
        ref_size: usize,
    }

    fn be(bytes: &[u8]) -> Option<u64> {
        if bytes.len() > 8 {
            return None;
        }
        Some(bytes.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b)))
    }

    impl<'a> Doc<'a> {
        fn parse(raw: &'a [u8]) -> Option<(Self, usize)> {
            if raw.len() < 40 {
                return None;
            }
            let t = &raw[raw.len() - 32..];
            let off_size = usize::from(t[6]);
            let ref_size = usize::from(t[7]);
            let count = be(&t[8..16])? as usize;
            let top = be(&t[16..24])? as usize;
            let table = be(&t[24..32])? as usize;
            if off_size == 0 || ref_size == 0 || count > 1_000_000 {
                return None;
            }
            let mut offsets = Vec::with_capacity(count);
            for i in 0..count {
                let at = table.checked_add(i.checked_mul(off_size)?)?;
                offsets.push(be(raw.get(at..at + off_size)?)? as usize);
            }
            Some((
                Self {
                    raw,
                    offsets,
                    ref_size,
                },
                top,
            ))
        }

        /// Marker, and (for sized objects) the length and where the
        /// payload starts.
        fn head(&self, obj: usize) -> Option<(u8, usize, usize)> {
            let at = *self.offsets.get(obj)?;
            let marker = *self.raw.get(at)?;
            let low = usize::from(marker & 0x0f);
            if low != 0x0f {
                return Some((marker >> 4, low, at + 1));
            }
            let int_marker = *self.raw.get(at + 1)?;
            if int_marker >> 4 != 0x1 {
                return None;
            }
            let n = 1usize << (int_marker & 0x0f);
            let len = be(self.raw.get(at + 2..at + 2 + n)?)? as usize;
            Some((marker >> 4, len, at + 2 + n))
        }

        fn string(&self, obj: usize) -> Option<String> {
            let (kind, len, start) = self.head(obj)?;
            match kind {
                0x5 => {
                    Some(String::from_utf8_lossy(self.raw.get(start..start + len)?).into_owned())
                }
                0x6 => {
                    let bytes = self.raw.get(start..start + len * 2)?;
                    let units: Vec<u16> = bytes
                        .chunks(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16(&units).ok()
                }
                0x1 => {
                    let at = *self.offsets.get(obj)?;
                    let n = 1usize << (self.raw.get(at)? & 0x0f);
                    Some(be(self.raw.get(at + 1..at + 1 + n)?)?.to_string())
                }
                0x0 => {
                    let at = *self.offsets.get(obj)?;
                    match self.raw.get(at)? {
                        0x08 => Some("false".into()),
                        0x09 => Some("true".into()),
                        _ => None,
                    }
                }
                _ => None,
            }
        }

        fn dict_get(&self, obj: usize, key: &str) -> Option<usize> {
            let (kind, count, start) = self.head(obj)?;
            if kind != 0xd {
                return None;
            }
            let r = self.ref_size;
            for i in 0..count {
                let k = be(self.raw.get(start + i * r..start + (i + 1) * r)?)? as usize;
                if self.string(k).as_deref() == Some(key) {
                    let vat = start + (count + i) * r;
                    return Some(be(self.raw.get(vat..vat + r)?)? as usize);
                }
            }
            None
        }
    }

    pub(super) fn scalar(raw: &[u8], path: &[&str]) -> Option<String> {
        let (doc, mut obj) = Doc::parse(raw)?;
        for key in path {
            obj = doc.dict_get(obj, key)?;
        }
        doc.string(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_xml_plist_answers_top_level_and_nested_keys() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>LastAccessedDate</key><date>2024-01-15T10:32:00Z</date>
  <key>ApplicationProperties</key><dict>
     <key>CFBundleShortVersionString</key><string>2.4</string>
  </dict>
  <key>WorkspacePath</key><string>/Users/dev/src/App/App.xcworkspace</string>
</dict></plist>"#;
        assert_eq!(
            plist_scalar(xml, &["WorkspacePath"]).as_deref(),
            Some("/Users/dev/src/App/App.xcworkspace")
        );
        assert_eq!(
            plist_scalar(
                xml,
                &["ApplicationProperties", "CFBundleShortVersionString"]
            )
            .as_deref(),
            Some("2.4")
        );
        assert_eq!(plist_scalar(xml, &["Missing"]), None);
        assert_eq!(
            plist_scalar(xml, &["CFBundleShortVersionString"]),
            None,
            "a nested key is not a top-level key"
        );
    }

    /// A hand-assembled `bplist00`: `{ "WorkspacePath": "/w/A.xcodeproj", "state": 3 }`.
    fn bplist_fixture() -> Vec<u8> {
        let mut v = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        // 0: dict with 2 entries, refs 1,2 (keys) 3,4 (values)
        offsets.push(v.len());
        v.extend([0xd2, 1, 2, 3, 4]);
        // 1: "WorkspacePath"
        offsets.push(v.len());
        v.push(0x50 | 13);
        v.extend(b"WorkspacePath");
        // 2: "state"
        offsets.push(v.len());
        v.push(0x55);
        v.extend(b"state");
        // 3: "/w/A.xcodeproj" (14 bytes)
        offsets.push(v.len());
        v.push(0x5e);
        v.extend(b"/w/A.xcodeproj");
        // 4: int 3
        offsets.push(v.len());
        v.extend([0x10, 3]);
        let table = v.len();
        for o in &offsets {
            v.push(*o as u8);
        }
        let mut trailer = vec![0u8; 6];
        trailer.push(1); // offset size
        trailer.push(1); // ref size
        trailer.extend((offsets.len() as u64).to_be_bytes());
        trailer.extend(0u64.to_be_bytes());
        trailer.extend((table as u64).to_be_bytes());
        v.extend(trailer);
        v
    }

    #[test]
    fn a_binary_plist_answers_the_same_questions() {
        let b = bplist_fixture();
        assert_eq!(
            plist_scalar(&b, &["WorkspacePath"]).as_deref(),
            Some("/w/A.xcodeproj")
        );
        assert_eq!(plist_scalar(&b, &["state"]).as_deref(), Some("3"));
        assert_eq!(plist_scalar(&b, &["nope"]), None);
        assert_eq!(
            plist_scalar(&b[..20], &["WorkspacePath"]),
            None,
            "a truncated plist is unreadable, not partially read"
        );
    }

    #[test]
    fn timestamps_parse_only_in_shapes_that_are_read() {
        assert_eq!(rfc3339_secs("1970-01-01T00:01:00Z"), Some(60));
        assert_eq!(
            rfc3339_secs("2024-01-15T10:32:00.123456789+01:00"),
            rfc3339_secs("2024-01-15T09:32:00Z")
        );
        assert_eq!(
            rfc3339_secs("2024-01-15 10:32:00 +0000 UTC"),
            rfc3339_secs("2024-01-15T10:32:00Z")
        );
        assert_eq!(rfc3339_secs("3 weeks ago"), None);
        assert_eq!(rfc3339_secs(""), None);
    }

    #[test]
    fn a_name_without_a_version_is_not_split() {
        assert_eq!(
            split_name_version("requests-2.31.0"),
            Some(("requests", "2.31.0"))
        );
        assert_eq!(split_name_version("left-pad"), None);
        assert_eq!(
            split_name_version("python-dateutil-2.9.0"),
            Some(("python-dateutil", "2.9.0"))
        );
    }
}
