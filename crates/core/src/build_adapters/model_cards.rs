//! What a downloaded model *is*, from files already on disk: a model
//! card's YAML front matter, `config.json`, a `*.safetensors` header, a
//! `.gguf` header, an Ollama config blob. Pure parsing over bytes a
//! bounded read already produced; nothing here opens a file.
//!
//! Never invent: a field the files do not state is absent, not guessed.
//! Every parser takes hostile input (a length prefix claiming 2^63
//! bytes, a shape whose product overflows, YAML that is not YAML) and
//! answers with an explicit unknown instead of a panic or a guess.
//!
//! [`CardCache`] is the pass-through cache every enrichment goes
//! through: keyed by content identity (a revision hash, a blob digest),
//! so a parse happens once per revision ever, and a pass with nothing
//! new does no content read at all.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The most of one file a card parse reads: a safetensors or GGUF
/// header, a README, a config. 1 MiB (`BoundedCap::MANIFEST`).
pub const MAX_CARD_READ: usize = 1024 * 1024;
/// New parses (a revision's card, a manifest) one identification pass
/// may do; the rest wait for the next pass, labelled "not yet read".
pub const NEW_PARSES_PER_PASS: usize = 64;
/// Weight-file headers read per revision (shards of one model).
pub const MAX_WEIGHT_HEADERS: usize = 16;
/// The longest model-card paragraph kept, in characters.
pub const MAX_CARD_TEXT_CHARS: usize = 400;
/// The longest single front-matter value kept, in characters.
const MAX_FIELD_CHARS: usize = 120;

/// One model's facts, as `(field, value)` pairs in a fixed vocabulary.
/// A `BTreeMap` so the stored and rendered order never depends on
/// parse order.
pub type CardFields = BTreeMap<String, String>;

// ---------------------------------------------------------------------
// Text hygiene
// ---------------------------------------------------------------------

/// Text from a file a stranger wrote, made safe to show: control
/// characters, bidirectional overrides and zero-width characters
/// removed, whitespace collapsed, dashes swapped for ASCII, bounded to
/// `max` characters.
pub fn sanitize(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_space = true;
    for c in text.chars() {
        let c = match c {
            '\u{2014}' | '\u{2013}' => '-',
            '\t' | '\n' | '\r' => ' ',
            c if c.is_control() => continue,
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{061C}'
            | '\u{FEFF}' => continue,
            c => c,
        };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
        if out.chars().count() >= max {
            break;
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------
// README: front matter and first paragraph
// ---------------------------------------------------------------------

/// The front-matter keys kept, and the field each becomes.
const FRONT_MATTER_KEYS: &[&str] = &[
    "pipeline_tag",
    "library_name",
    "license",
    "base_model",
    "tags",
    "language",
];

/// The README's YAML front matter (the `---` block a model card opens
/// with), for the keys in [`FRONT_MATTER_KEYS`] only. A minimal reader:
/// `key: value`, `key: [a, b]` and `key:` followed by `- item` lines.
/// Anything else is skipped, never guessed at; a block with no closing
/// `---` yields nothing.
pub fn front_matter(readme: &str) -> CardFields {
    let mut out = CardFields::new();
    let text = readme.trim_start_matches('\u{FEFF}');
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return out;
    };
    let Some(end) = rest.find("\n---") else {
        return out;
    };
    let block = &rest[..end];
    let mut current: Option<&str> = None;
    let mut list: Vec<String> = Vec::new();
    let flush = |key: Option<&str>, list: &mut Vec<String>, out: &mut CardFields| {
        if let Some(k) = key
            && !list.is_empty()
        {
            out.insert(k.to_string(), sanitize(&list.join(", "), MAX_FIELD_CHARS));
        }
        list.clear();
    };
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(item) = trimmed.strip_prefix("- ")
            && current.is_some()
            && line.starts_with([' ', '-'])
        {
            list.push(unquote(item).to_string());
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            // Nested mapping lines under a key this reader does not keep.
            continue;
        }
        flush(current, &mut list, &mut out);
        current = None;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let Some(&known) = FRONT_MATTER_KEYS.iter().find(|k| **k == key) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            current = Some(known);
        } else if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            let items: Vec<&str> = inner
                .split(',')
                .map(|s| unquote(s.trim()))
                .filter(|s| !s.is_empty())
                .collect();
            if !items.is_empty() {
                out.insert(
                    known.to_string(),
                    sanitize(&items.join(", "), MAX_FIELD_CHARS),
                );
            }
        } else {
            let v = unquote(value);
            if !v.is_empty() {
                out.insert(known.to_string(), sanitize(v, MAX_FIELD_CHARS));
            }
        }
    }
    flush(current, &mut list, &mut out);
    out
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(s)
}

/// The model card's first paragraph of prose: after the front matter,
/// skipping headings, HTML, badges, tables and code fences. Sanitized
/// and bounded; `None` when there is no prose.
pub fn first_paragraph(readme: &str) -> Option<String> {
    let mut body = readme.trim_start_matches('\u{FEFF}');
    if let Some(rest) = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
    {
        body = match rest.find("\n---") {
            Some(end) => rest[end + 4..].trim_start_matches(['-', '\r', '\n']),
            None => "",
        };
    }
    let mut para: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        let skip = t.starts_with('#')
            || t.starts_with('<')
            || t.starts_with("[![")
            || t.starts_with("![")
            || t.starts_with('|')
            || t.starts_with("---")
            || t.starts_with('>');
        if t.is_empty() || skip {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(t);
    }
    let text = sanitize(&para.join(" "), MAX_CARD_TEXT_CHARS);
    (!text.is_empty()).then_some(text)
}

// ---------------------------------------------------------------------
// config.json
// ---------------------------------------------------------------------

/// `config.json`'s identifying fields: `model_type`, `architectures`,
/// `torch_dtype`, and a parameter count only when the file states one
/// (`num_parameters`). Malformed JSON yields nothing.
pub fn config_fields(text: &str) -> CardFields {
    let mut out = CardFields::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|x| sanitize(x, MAX_FIELD_CHARS))
    };
    if let Some(t) = s("model_type").filter(|t| !t.is_empty()) {
        out.insert("model_type".into(), t);
    }
    if let Some(a) = v.get("architectures").and_then(|a| a.as_array()) {
        let names: Vec<String> = a
            .iter()
            .filter_map(|x| x.as_str())
            .take(4)
            .map(|x| sanitize(x, MAX_FIELD_CHARS))
            .collect();
        if !names.is_empty() {
            out.insert("architectures".into(), names.join(", "));
        }
    }
    if let Some(t) = s("torch_dtype")
        .or_else(|| s("dtype"))
        .filter(|t| !t.is_empty())
    {
        out.insert("torch_dtype".into(), t);
    }
    if let Some(n) = v.get("num_parameters").and_then(|x| x.as_u64()) {
        out.insert("params_stated".into(), n.to_string());
    }
    if let Some(n) = v.get("hidden_size").and_then(|x| x.as_u64()) {
        out.insert("hidden_size".into(), n.to_string());
    }
    out
}

// ---------------------------------------------------------------------
// safetensors
// ---------------------------------------------------------------------

/// What a safetensors header said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeightHeader {
    /// Exact parameter count (the product of every tensor's shape) and
    /// the dtypes present.
    Read { params: u64, dtypes: Vec<String> },
    /// The header claims more bytes than the bounded read allows.
    TooLarge { claimed: u64 },
    /// Not a header this parser can read: wrong magic, truncated, not
    /// JSON, an overflowing shape.
    Malformed(&'static str),
}

/// Parses a safetensors header from the first bytes of the file: an
/// 8-byte little-endian length, then that many bytes of JSON mapping
/// tensor names to `{dtype, shape, data_offsets}`. No tensor is read.
pub fn safetensors_header(bytes: &[u8]) -> WeightHeader {
    let Some(len) = bytes.get(..8) else {
        return WeightHeader::Malformed("shorter than the 8-byte length prefix");
    };
    let claimed = u64::from_le_bytes(len.try_into().unwrap_or([0; 8]));
    if claimed > (MAX_CARD_READ - 8) as u64 {
        return WeightHeader::TooLarge { claimed };
    }
    let end = 8 + claimed as usize;
    let Some(json) = bytes.get(8..end) else {
        return WeightHeader::Malformed("the file ends before the header does");
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else {
        return WeightHeader::Malformed("the header is not JSON");
    };
    let Some(map) = v.as_object() else {
        return WeightHeader::Malformed("the header is not a JSON object");
    };
    let mut params: u64 = 0;
    let mut dtypes: Vec<String> = Vec::new();
    for (name, t) in map {
        if name == "__metadata__" {
            continue;
        }
        let Some(shape) = t.get("shape").and_then(|s| s.as_array()) else {
            return WeightHeader::Malformed("a tensor has no shape");
        };
        let mut n: u64 = 1;
        for d in shape {
            let Some(d) = d.as_u64() else {
                return WeightHeader::Malformed("a shape holds a non-integer");
            };
            let Some(m) = n.checked_mul(d) else {
                return WeightHeader::Malformed("a shape's product overflows");
            };
            n = m;
        }
        let Some(p) = params.checked_add(n) else {
            return WeightHeader::Malformed("the parameter total overflows");
        };
        params = p;
        if let Some(dt) = t.get("dtype").and_then(|d| d.as_str()) {
            let dt = sanitize(dt, 16).to_ascii_lowercase();
            if !dtypes.contains(&dt) {
                dtypes.push(dt);
            }
        }
    }
    dtypes.sort();
    WeightHeader::Read { params, dtypes }
}

// ---------------------------------------------------------------------
// GGUF
// ---------------------------------------------------------------------

/// What a GGUF header said, as far as the bounded read reached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GgufFacts {
    pub architecture: Option<String>,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub quantization: Option<String>,
    /// Exact, from the tensor infos, only when every one fit in the read.
    pub params: Option<u64>,
}

/// llama.cpp's `general.file_type` numbering (`LLAMA_FTYPE_*`), for the
/// values with a stable name. Anything else is shown as its number.
fn ftype_name(n: u32) -> String {
    match n {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        32 => "BF16",
        _ => return format!("file type {n}"),
    }
    .to_string()
}

struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u64()?;
        if n > self.b.len() as u64 {
            return None;
        }
        self.take(n as usize)
    }
    /// Skips one value of GGUF type `t`; `None` past the buffer.
    fn skip_value(&mut self, t: u32, depth: u8) -> Option<()> {
        let fixed = |t: u32| -> Option<usize> {
            Some(match t {
                0 | 1 | 7 => 1,
                2 | 3 => 2,
                4..=6 => 4,
                10..=12 => 8,
                _ => return None,
            })
        };
        match t {
            8 => {
                self.string()?;
            }
            9 => {
                if depth > 2 {
                    return None;
                }
                let inner = self.u32()?;
                let n = self.u64()?;
                if let Some(size) = fixed(inner) {
                    let total = (n as usize).checked_mul(size)?;
                    self.take(total)?;
                } else {
                    if n > self.b.len() as u64 {
                        return None;
                    }
                    for _ in 0..n {
                        self.skip_value(inner, depth + 1)?;
                    }
                }
            }
            t => {
                self.take(fixed(t)?)?;
            }
        }
        Some(())
    }
}

/// Parses as much of a GGUF header as `bytes` holds. `None` when the
/// magic is wrong; fields past the end of the bounded read stay absent.
pub fn gguf_header(bytes: &[u8]) -> Option<GgufFacts> {
    if bytes.get(..4)? != b"GGUF" {
        return None;
    }
    let mut c = Cursor { b: bytes, at: 4 };
    let version = c.u32()?;
    if !(2..=3).contains(&version) {
        return None;
    }
    let mut facts = GgufFacts::default();
    let tensors = c.u64()?;
    let kvs = c.u64()?;
    let mut complete = true;
    for _ in 0..kvs.min(1 << 20) {
        let Some(key) = c.string() else {
            complete = false;
            break;
        };
        let key = String::from_utf8_lossy(key).into_owned();
        let Some(t) = c.u32() else {
            complete = false;
            break;
        };
        let start = c.at;
        let string_of = |c: &mut Cursor| -> Option<String> {
            c.string()
                .map(|s| sanitize(&String::from_utf8_lossy(s), MAX_FIELD_CHARS))
        };
        match (key.as_str(), t) {
            ("general.architecture", 8) => facts.architecture = string_of(&mut c),
            ("general.name", 8) => facts.name = string_of(&mut c),
            ("general.size_label", 8) => facts.size_label = string_of(&mut c),
            ("general.file_type", 4) => facts.quantization = c.u32().map(ftype_name),
            _ => {
                c.at = start;
                if c.skip_value(t, 0).is_none() {
                    complete = false;
                    break;
                }
            }
        }
    }
    if complete && kvs < (1 << 20) {
        // Tensor infos: name, n_dims (u32), dims (u64 each), type (u32),
        // offset (u64). The parameter count is exact only when every
        // one fits in the read.
        let mut params: u64 = 0;
        let mut all = true;
        for _ in 0..tensors.min(1 << 20) {
            let ok = (|| -> Option<u64> {
                c.string()?;
                let dims = c.u32()?;
                if dims > 8 {
                    return None;
                }
                let mut n: u64 = 1;
                for _ in 0..dims {
                    n = n.checked_mul(c.u64()?)?;
                }
                c.u32()?;
                c.u64()?;
                Some(n)
            })();
            match ok.and_then(|n| params.checked_add(n)) {
                Some(p) => params = p,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all && tensors > 0 && tensors < (1 << 20) {
            facts.params = Some(params);
        }
    }
    Some(facts)
}

// ---------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------

/// One layer an Ollama manifest names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaLayer {
    pub media_type: String,
    /// `sha256:<hex>`, exactly as the manifest spells it.
    pub digest: String,
    pub size: u64,
}

/// The manifest's config and layers. `None` for anything that is not a
/// manifest of this shape, or a digest that is not `sha256:<64 hex>`
/// (a digest is turned into a blob file name, so it is validated).
pub fn ollama_manifest(text: &str) -> Option<(OllamaLayer, Vec<OllamaLayer>)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let layer = |l: &serde_json::Value| -> Option<OllamaLayer> {
        let digest = l.get("digest")?.as_str()?;
        let hex = digest.strip_prefix("sha256:")?;
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(OllamaLayer {
            media_type: sanitize(l.get("mediaType")?.as_str()?, MAX_FIELD_CHARS),
            digest: digest.to_string(),
            size: l.get("size")?.as_u64()?,
        })
    };
    let config = layer(v.get("config")?)?;
    let layers = v
        .get("layers")?
        .as_array()?
        .iter()
        .map(layer)
        .collect::<Option<Vec<_>>>()?;
    Some((config, layers))
}

/// An Ollama config blob's identifying fields: `model_format`,
/// `model_family`, `model_type` (Ollama's parameter-size label) and
/// `file_type` (its quantization label).
pub fn ollama_config(text: &str) -> CardFields {
    let mut out = CardFields::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    for (from, to) in [
        ("model_format", "format"),
        ("model_family", "family"),
        ("model_type", "parameter_size"),
        ("file_type", "quantization"),
    ] {
        if let Some(s) = v.get(from).and_then(|x| x.as_str()) {
            let s = sanitize(s, MAX_FIELD_CHARS);
            if !s.is_empty() {
                out.insert(to.to_string(), s);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// The line
// ---------------------------------------------------------------------

/// An exact parameter count, rounded for a one-line summary: `8.0B`,
/// `241.7M`. The exact count stays in the `params` field.
pub fn params_short(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.1}K", f / 1e3)
    } else {
        n.to_string()
    }
}

/// The one-line "what it is", from the fields present and nothing else:
/// `text-generation · llama · 8.0B params · bf16 · license llama3 · base
/// meta-llama/Llama-3.1-8B`. `None` when no field is present.
pub fn summary_line(f: &CardFields) -> Option<String> {
    let get = |k: &str| f.get(k).filter(|v| !v.is_empty()).cloned();
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = get("pipeline_tag") {
        parts.push(p);
    }
    if let Some(a) = get("model_type")
        .or_else(|| get("gguf_architecture"))
        .or_else(|| get("family"))
    {
        parts.push(a);
    } else if let Some(a) = get("architectures") {
        parts.push(a);
    }
    if let Some(n) = get("params").and_then(|p| p.parse::<u64>().ok()) {
        parts.push(format!("{} params", params_short(n)));
    } else if let Some(s) = get("parameter_size").or_else(|| get("gguf_size_label")) {
        parts.push(format!("{s} params"));
    }
    if let Some(q) = get("quantization") {
        parts.push(q);
    } else if let Some(d) = get("torch_dtype").or_else(|| get("dtypes")) {
        parts.push(d);
    }
    if let Some(l) = get("license") {
        parts.push(format!("license {l}"));
    }
    if let Some(b) = get("base_model") {
        parts.push(format!("base {b}"));
    }
    if parts.is_empty() {
        parts.push(get("library_name")?);
    }
    Some(parts.join(" · "))
}

// ---------------------------------------------------------------------
// The pass-through cache
// ---------------------------------------------------------------------

/// One stored enrichment: the content fingerprint it was produced
/// from, when, and the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardEntry {
    pub fingerprint: String,
    pub at: u64,
    pub fields: CardFields,
}

/// Every enrichment read goes through here. A lookup with a matching
/// content fingerprint is a hit and costs nothing; a miss may parse
/// only while the pass's budget lasts.
#[derive(Debug)]
pub struct CardCache {
    entries: RefCell<HashMap<String, CardEntry>>,
    touched: RefCell<HashSet<String>>,
    budget: Cell<usize>,
}

impl Default for CardCache {
    fn default() -> Self {
        Self::from_entries(HashMap::new(), NEW_PARSES_PER_PASS)
    }
}

impl CardCache {
    pub fn from_entries(entries: HashMap<String, CardEntry>, budget: usize) -> Self {
        Self {
            entries: RefCell::new(entries),
            touched: RefCell::new(HashSet::new()),
            budget: Cell::new(budget),
        }
    }

    /// The stored fields for `key` when they were produced from exactly
    /// `fingerprint`. Marks the entry as still wanted.
    pub fn lookup(&self, key: &str, fingerprint: &str) -> Option<CardEntry> {
        let entries = self.entries.borrow();
        let e = entries.get(key)?;
        if e.fingerprint != fingerprint {
            return None;
        }
        self.touched.borrow_mut().insert(key.to_string());
        Some(e.clone())
    }

    /// Any stored entry for `key`, whatever it was produced from, and
    /// without marking it wanted: for a value that carries forward (the
    /// access time swamp's own read left behind).
    pub fn peek(&self, key: &str) -> Option<CardEntry> {
        self.entries.borrow().get(key).cloned()
    }

    /// Takes one unit of this pass's parse budget; `false` once spent.
    pub fn try_spend(&self) -> bool {
        let left = self.budget.get();
        if left == 0 {
            return false;
        }
        self.budget.set(left - 1);
        true
    }

    pub fn insert(&self, key: &str, entry: CardEntry) {
        self.touched.borrow_mut().insert(key.to_string());
        self.entries.borrow_mut().insert(key.to_string(), entry);
    }

    /// What to store after the pass: every entry this pass used, plus
    /// every entry under a store this pass did not identify (`keep`
    /// says which keys those are). An entry for a revision or manifest
    /// that is gone is dropped here.
    pub fn retained(&self, keep: &dyn Fn(&str) -> bool) -> HashMap<String, CardEntry> {
        let touched = self.touched.borrow();
        self.entries
            .borrow()
            .iter()
            .filter(|(k, _)| touched.contains(*k) || keep(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(header: &str) -> Vec<u8> {
        let mut b = (header.len() as u64).to_le_bytes().to_vec();
        b.extend_from_slice(header.as_bytes());
        b
    }

    #[test]
    fn safetensors_counts_every_tensor_exactly() {
        let h = r#"{"__metadata__":{"format":"pt"},"a":{"dtype":"BF16","shape":[4096,4096],"data_offsets":[0,1]},"b":{"dtype":"F32","shape":[4096],"data_offsets":[1,2]}}"#;
        assert_eq!(
            safetensors_header(&st(h)),
            WeightHeader::Read {
                params: 4096 * 4096 + 4096,
                dtypes: vec!["bf16".into(), "f32".into()]
            }
        );
    }

    /// Tempting wrong patch: trusting the length prefix and allocating
    /// or slicing `8 + claimed` bytes. A 2^63 claim must be reported,
    /// not attempted.
    #[test]
    fn a_malicious_length_prefix_is_reported_not_followed() {
        let mut b = u64::MAX.to_le_bytes().to_vec();
        b.extend_from_slice(b"{}");
        assert_eq!(
            safetensors_header(&b),
            WeightHeader::TooLarge { claimed: u64::MAX }
        );
        let mut b = 500u64.to_le_bytes().to_vec();
        b.extend_from_slice(b"{\"a\":");
        assert!(matches!(safetensors_header(&b), WeightHeader::Malformed(_)));
        assert!(matches!(
            safetensors_header(b"abc"),
            WeightHeader::Malformed(_)
        ));
    }

    /// Tempting wrong patch: `shape.iter().product()` with plain `*`,
    /// which panics in debug and wraps in release.
    #[test]
    fn an_overflowing_shape_is_malformed_not_wrapped() {
        let h = format!(r#"{{"a":{{"dtype":"F32","shape":[{},{}]}}}}"#, u64::MAX, 4);
        assert!(matches!(
            safetensors_header(&st(&h)),
            WeightHeader::Malformed(_)
        ));
    }

    #[test]
    fn front_matter_keeps_listed_keys_and_skips_the_rest() {
        let r = "---\nlicense: llama3\npipeline_tag: text-generation\nbase_model:\n  - meta-llama/Llama-3.1-8B\nwidget:\n  - text: hi\ntags: [a, \"b\"]\n---\n# Title\n\nAn 8B model for chat.\nSecond line.\n\nMore.";
        let f = front_matter(r);
        assert_eq!(f["license"], "llama3");
        assert_eq!(f["pipeline_tag"], "text-generation");
        assert_eq!(f["base_model"], "meta-llama/Llama-3.1-8B");
        assert_eq!(f["tags"], "a, b");
        assert!(!f.contains_key("widget"));
        assert_eq!(
            first_paragraph(r).unwrap(),
            "An 8B model for chat. Second line."
        );
    }

    /// Tempting wrong patch: parsing whatever follows `---` up to EOF
    /// when the block never closes, and treating prose as fields.
    #[test]
    fn malformed_front_matter_yields_nothing() {
        assert!(front_matter("---\nlicense: mit\nno closing").is_empty());
        assert!(front_matter("license: mit\n").is_empty());
        assert!(front_matter("---\n: : :\n\t- x\n---\n").is_empty());
        assert!(config_fields("{not json").is_empty());
        assert!(ollama_config("[]").is_empty());
    }

    /// Tempting wrong patch: showing the README's text verbatim. A
    /// right-to-left override or an escape sequence would reorder or
    /// recolor the terminal.
    #[test]
    fn card_text_is_stripped_of_control_and_bidi_characters() {
        let r = "Evil\u{202E}txt.exe\u{1b}[31m red\u{200B} \u{2014} done";
        let p = first_paragraph(r).unwrap();
        assert_eq!(p, "Eviltxt.exe[31m red - done");
        assert!(p.chars().all(|c| !c.is_control()));
        let long = "x".repeat(5000);
        assert_eq!(
            first_paragraph(&long).unwrap().chars().count(),
            MAX_CARD_TEXT_CHARS
        );
    }

    fn gguf(kvs: &[(&str, u32, Vec<u8>)], tensors: &[(&str, &[u64])]) -> Vec<u8> {
        let mut b = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend((tensors.len() as u64).to_le_bytes());
        b.extend((kvs.len() as u64).to_le_bytes());
        for (k, t, v) in kvs {
            b.extend((k.len() as u64).to_le_bytes());
            b.extend(k.as_bytes());
            b.extend(t.to_le_bytes());
            b.extend(v);
        }
        for (name, dims) in tensors {
            b.extend((name.len() as u64).to_le_bytes());
            b.extend(name.as_bytes());
            b.extend((dims.len() as u32).to_le_bytes());
            for d in *dims {
                b.extend(d.to_le_bytes());
            }
            b.extend(0u32.to_le_bytes());
            b.extend(0u64.to_le_bytes());
        }
        b
    }

    fn gstr(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u64).to_le_bytes().to_vec();
        v.extend(s.as_bytes());
        v
    }

    #[test]
    fn gguf_reads_architecture_name_quantization_and_params() {
        let mut arr = 8u32.to_le_bytes().to_vec();
        arr.extend(2u64.to_le_bytes());
        arr.extend(gstr("a"));
        arr.extend(gstr("b"));
        let b = gguf(
            &[
                ("general.architecture", 8, gstr("llama")),
                ("tokenizer.ggml.tokens", 9, arr),
                ("general.name", 8, gstr("Tiny")),
                ("general.file_type", 4, 15u32.to_le_bytes().to_vec()),
            ],
            &[("w", &[10, 20]), ("b", &[20])],
        );
        let f = gguf_header(&b).unwrap();
        assert_eq!(f.architecture.as_deref(), Some("llama"));
        assert_eq!(f.name.as_deref(), Some("Tiny"));
        assert_eq!(f.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(f.params, Some(220));
    }

    /// Tempting wrong patch: reporting the partial tensor sum when the
    /// bounded read ends inside the tensor infos.
    #[test]
    fn a_gguf_cut_short_keeps_what_it_read_and_no_param_guess() {
        let b = gguf(
            &[("general.architecture", 8, gstr("qwen3"))],
            &[("w", &[10, 20]), ("b", &[20])],
        );
        let f = gguf_header(&b[..b.len() - 6]).unwrap();
        assert_eq!(f.architecture.as_deref(), Some("qwen3"));
        assert_eq!(f.params, None);
        // A string length claiming the universe.
        let mut evil = b"GGUF".to_vec();
        evil.extend(3u32.to_le_bytes());
        evil.extend(1u64.to_le_bytes());
        evil.extend(1u64.to_le_bytes());
        evil.extend(u64::MAX.to_le_bytes());
        let f = gguf_header(&evil).unwrap();
        assert_eq!(f, GgufFacts::default());
        assert!(gguf_header(b"GGML....").is_none());
    }

    #[test]
    fn summary_line_uses_present_fields_only() {
        let mut f = CardFields::new();
        assert_eq!(summary_line(&f), None);
        f.insert("pipeline_tag".into(), "text-generation".into());
        f.insert("model_type".into(), "llama".into());
        f.insert("params".into(), "8030261248".into());
        f.insert("torch_dtype".into(), "bf16".into());
        f.insert("license".into(), "llama3".into());
        f.insert("base_model".into(), "meta-llama/Llama-3.1-8B".into());
        assert_eq!(
            summary_line(&f).unwrap(),
            "text-generation · llama · 8.0B params · bf16 · license llama3 · base meta-llama/Llama-3.1-8B"
        );
    }

    #[test]
    fn ollama_manifest_rejects_a_digest_that_could_name_another_file() {
        let ok = r#"{"config":{"mediaType":"c","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":2},"layers":[]}"#;
        assert!(ollama_manifest(ok).is_some());
        let evil = ok.replace(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:../../../etc/passwd",
        );
        assert!(ollama_manifest(&evil).is_none());
        assert!(ollama_manifest("{").is_none());
    }

    #[test]
    fn the_cache_hits_only_on_the_same_fingerprint_and_prunes_what_was_not_used() {
        let c = CardCache::from_entries(HashMap::new(), 1);
        let e = CardEntry {
            fingerprint: "f1".into(),
            at: 5,
            fields: CardFields::new(),
        };
        assert!(c.try_spend());
        assert!(!c.try_spend(), "the per-pass budget is a cap");
        c.insert("k", e.clone());
        let fresh = CardCache::from_entries(c.retained(&|_| false), 0);
        assert!(fresh.lookup("k", "f2").is_none());
        assert!(fresh.lookup("k", "f1").is_some());
        let mut stored = HashMap::new();
        stored.insert("gone".to_string(), e.clone());
        stored.insert("other-store".to_string(), e);
        let c = CardCache::from_entries(stored, 0);
        let kept = c.retained(&|k| k.starts_with("other"));
        assert!(kept.contains_key("other-store"));
        assert!(!kept.contains_key("gone"));
    }
}
