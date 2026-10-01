//! Downloaded model caches, one unit per model: the Hugging Face hub
//! cache (`models--<org>--<name>`, `datasets--*`, `spaces--*`) and the
//! Ollama model store (`manifests/<host>/<ns>/<model>/<tag>` naming
//! layers in `blobs/`).
//!
//! Each repo or model:tag is a unit with its size (blobs counted once,
//! a blob two revisions or two tags share charged to exactly one of
//! them), its revisions or tag, a "what it is" line from the files
//! already on disk (`model_cards`), when its largest weight file was
//! last read (file access time, labelled), and what getting it back
//! costs: a download from where it came from, or nothing recorded.
//!
//! Every content read goes through the pass-through [`CardCache`]: a
//! revision's files and an Ollama manifest's layers are immutable for a
//! given key, so each is parsed once ever and an unchanged store costs
//! listings and `lstat`s only. Symlinks are read as text and resolved
//! lexically; one that leaves the cache is reported, never followed.
//!
//! `swamp` reports; the human removes. Nothing here offers an action.

use super::model_cards::{self, CardCache, CardEntry, CardFields};
use super::{BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, NestedUnitBuilder};
use crate::artifact::{
    AccountingBasis, ArtifactRole, ArtifactVariant, Membership, NestedArtifact, TimeSource,
};
use crate::entities::Confidence;
use crate::locations::{BuildStoreKind, Truncation};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

/// Evidence sources this adapter writes, read by the views.
pub const CARD_EVIDENCE: &str = "model-card";
pub const CARD_TEXT_EVIDENCE: &str = "model-card-text";
pub const FIELD_EVIDENCE: &str = "model-field";
pub const REVISION_EVIDENCE: &str = "model-revision";
pub const LAST_READ_EVIDENCE: &str = "model-last-read";
pub const HUB_API_EVIDENCE: &str = "model-hub-api";

/// How many snapshot sub-folder levels are read (`onnx/`, `1_Pooling/`).
const MAX_SNAPSHOT_DEPTH: usize = 3;
/// Revisions listed per repo before the rest are a count.
const MAX_REVISIONS: usize = 32;

/// Bumped when what a stored card means changes; part of every key's
/// fingerprint, so an old entry is a miss, never a wrong answer.
const CARD_FORMAT: &str = "card-1";

pub struct Adapter;

impl BuildAdapter for Adapter {
    fn id(&self) -> &'static str {
        "model-stores"
    }

    fn name(&self) -> &'static str {
        "Model caches (Hugging Face hub, Ollama)"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: true,
            actions_available: false,
        }
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[BuildStoreKind::HuggingFaceHub, BuildStoreKind::OllamaModels]
    }

    fn containers(&self, _project_root: &Path, _candidates: &[PathBuf]) -> Vec<BuildContainer> {
        Vec::new()
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        match container.store_kind {
            Some(BuildStoreKind::HuggingFaceHub) => hub(container, ctx),
            Some(BuildStoreKind::OllamaModels) => ollama(container, ctx),
            _ => Vec::new(),
        }
    }
}

fn bytes(n: u64) -> String {
    crate::render::human_bytes_pub(n)
}

/// `base` joined with a symlink's text, resolved without touching the
/// filesystem. `None` for a link that climbs above the root of `base`.
fn lexical_join(base: &Path, target: &Path) -> Option<PathBuf> {
    let mut out = if target.is_absolute() {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for c in target.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::CurDir | Component::RootDir => {}
            Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// One regular file's facts, from one `lstat`.
#[derive(Debug, Clone)]
struct FileFact {
    path: PathBuf,
    allocated: u64,
    len: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
}

impl FileFact {
    /// Whether reading this file would set its access time, the last-read
    /// fact swamp reports. macOS updates the access time on a read only
    /// when it is not newer than the last change (checked on APFS: a
    /// file read since its last write keeps its time through another
    /// read). On Linux the read opens with `O_NOATIME` and moves nothing.
    fn read_would_move_atime(&self) -> bool {
        cfg!(target_os = "macos") && self.atime <= self.mtime.max(self.ctime)
    }
}

/// Whether `p` is a real directory: present, and not a symlink.
fn real_dir(ctx: &BuildCtx, p: &Path) -> bool {
    ctx.stat(p)
        .is_some_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Whether every directory from `base` down to `target`'s parent is a
/// real directory: a link resolved as text must not pass through a
/// symlinked folder the check never saw.
fn real_dirs_between(ctx: &BuildCtx, base: &Path, target: &Path) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    if !parent.starts_with(base) {
        return false;
    }
    parent
        .ancestors()
        .take_while(|a| a.starts_with(base))
        .all(|a| real_dir(ctx, a))
}

/// `name` with the characters a terminal or a JSON reader could be
/// fooled by removed (folder names come from the disk).
fn clean(name: &str) -> String {
    model_cards::sanitize(name, 200)
}

fn file_fact(ctx: &BuildCtx, path: &Path) -> Option<FileFact> {
    use crate::fs_gate::MetadataExt;
    let m = ctx.stat(path)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return None;
    }
    Some(FileFact {
        path: path.to_path_buf(),
        allocated: m.blocks() * 512,
        len: m.len(),
        atime: m.atime().max(0) as u64,
        mtime: m.mtime().max(0) as u64,
        ctime: m.ctime().max(0) as u64,
    })
}

fn truncated(t: Truncation) -> bool {
    !matches!(t, Truncation::Complete)
}

/// The access-time fact of the largest weight file, with swamp's own
/// header read set aside: when the file's access time is exactly the
/// one swamp's read left behind, the time before that read is the
/// fact.
fn last_read(fact: &FileFact, card: Option<&CardEntry>, file_name: &str) -> (u64, String) {
    let blob = fact.path.to_string_lossy();
    if let Some(c) = card
        && c.fields.get("read_blob").map(String::as_str) == Some(blob.as_ref())
        && c.fields.get("read_atime_after") == Some(&fact.atime.to_string())
        && let Some(before) = c
            .fields
            .get("read_atime_before")
            .and_then(|b| b.parse::<u64>().ok())
        // A read that did not move the access time left nothing to set aside.
        && before != fact.atime
    {
        return (
            before,
            format!("file access time of {file_name}, before swamp's own header read"),
        );
    }
    (fact.atime, format!("file access time of {file_name}"))
}

fn add_card_evidence(mut b: NestedUnitBuilder, fields: &CardFields) -> NestedUnitBuilder {
    if let Some(line) = model_cards::summary_line(fields) {
        b = b.evidence(CARD_EVIDENCE, line, Confidence::High);
    }
    if let Some(text) = fields.get("card_text") {
        b = b.evidence(CARD_TEXT_EVIDENCE, text.clone(), Confidence::High);
    }
    for (k, v) in fields {
        if k == "card_text" || k.starts_with("read_") || k.starts_with("api_") {
            continue;
        }
        b = b.evidence(FIELD_EVIDENCE, format!("{k}: {v}"), Confidence::High);
    }
    b
}

// ---------------------------------------------------------------------
// Hugging Face hub cache
// ---------------------------------------------------------------------

/// The repo kind a hub folder name says, and the repo id.
pub fn hub_repo_of(folder: &str) -> Option<(&'static str, String)> {
    let (kind, rest) = [
        ("model", "models--"),
        ("dataset", "datasets--"),
        ("space", "spaces--"),
    ]
    .iter()
    .find_map(|(k, p)| folder.strip_prefix(p).map(|r| (*k, r)))?;
    if rest.is_empty() {
        return None;
    }
    Some((kind, rest.replacen("--", "/", 1)))
}

/// What one repo folder holds.
#[derive(Default)]
struct RepoScan {
    /// Blob name -> the regular file holding it (in the repo's own
    /// `blobs/`, or the hub's shared `blobs/` through a link).
    blobs: BTreeMap<String, FileFact>,
    /// Blob names whose file is in the hub's shared folder.
    shared: BTreeSet<String>,
    incomplete: (usize, u64),
    refs: Vec<(String, String)>,
    /// Revision -> (file path inside the snapshot -> blob name).
    snapshots: BTreeMap<String, BTreeMap<String, String>>,
    /// Revision -> its snapshot folder's mtime.
    snapshot_mtime: BTreeMap<String, u64>,
    dangling: Vec<String>,
    outside: Vec<String>,
    /// Folders inside the repo that are links: never followed.
    linked: Vec<String>,
    partial_listing: bool,
}

fn scan_repo(ctx: &BuildCtx, hub_root: &Path, repo: &Path) -> RepoScan {
    let mut s = RepoScan::default();
    let blobs_dir = repo.join("blobs");
    let shared_dir = hub_root.join("blobs");
    for sub in ["blobs", "refs", "snapshots"] {
        let p = repo.join(sub);
        if ctx.stat(&p).is_some_and(|m| m.file_type().is_symlink()) {
            s.linked.push(format!("{sub}/"));
        }
    }
    let listable = |p: &Path| real_dir(ctx, p);
    let (entries, t) = if listable(&blobs_dir) {
        ctx.list_links(&blobs_dir)
    } else {
        (Vec::new(), Truncation::Complete)
    };
    s.partial_listing |= truncated(t);
    for e in entries {
        let path = blobs_dir.join(&e.name);
        if e.is_dir {
            continue;
        }
        if e.is_symlink {
            let Some(target) = ctx.link_text(&path) else {
                s.dangling.push(format!("blobs/{}", e.name));
                continue;
            };
            match lexical_join(&blobs_dir, &target) {
                // Into the hub's shared folder, through real folders only.
                Some(t)
                    if t.starts_with(&shared_dir) && real_dirs_between(ctx, &shared_dir, &t) =>
                {
                    match file_fact(ctx, &t) {
                        Some(f) => {
                            s.shared.insert(e.name.clone());
                            s.blobs.insert(e.name, f);
                        }
                        None => s.dangling.push(format!("blobs/{}", e.name)),
                    }
                }
                // To a sibling in this repo's own blobs/: the same bytes
                // the walk already counted in this folder, never again.
                Some(t) if t.parent() == Some(blobs_dir.as_path()) => match file_fact(ctx, &t) {
                    Some(f) => {
                        s.blobs.insert(e.name, f);
                    }
                    None => s.dangling.push(format!("blobs/{}", e.name)),
                },
                Some(t) if t.starts_with(&shared_dir) => {
                    s.linked
                        .push(format!("blobs/{} (through a linked folder)", e.name));
                }
                _ => s.outside.push(format!("blobs/{}", e.name)),
            }
            continue;
        }
        if let Some(f) = file_fact(ctx, &path) {
            if e.name.ends_with(".incomplete") {
                s.incomplete.0 += 1;
                s.incomplete.1 += f.allocated;
            } else {
                s.blobs.insert(e.name, f);
            }
        }
    }
    let (refs, t) = if listable(&repo.join("refs")) {
        ctx.list_links(&repo.join("refs"))
    } else {
        (Vec::new(), Truncation::Complete)
    };
    s.partial_listing |= truncated(t);
    for r in refs.iter().filter(|r| !r.is_dir && !r.is_symlink) {
        let p = repo.join("refs").join(&r.name);
        if let Some(rev) = ref_cached(ctx, hub_root, &p) {
            s.refs.push((r.name.clone(), rev));
        }
    }
    let (revs, t) = if listable(&repo.join("snapshots")) {
        ctx.list_links(&repo.join("snapshots"))
    } else {
        (Vec::new(), Truncation::Complete)
    };
    s.partial_listing |= truncated(t);
    // Which revisions are read when there are more than the bound: those
    // a ref names first, then the newest by the folder's own mtime; never
    // directory or lexical order.
    let named: BTreeSet<&str> = s.refs.iter().map(|(_, r)| r.as_str()).collect();
    let mut order: Vec<(&crate::locations::LinkEntry, bool, u64)> = revs
        .iter()
        .filter(|r| r.is_dir)
        .map(|r| {
            use crate::fs_gate::MetadataExt;
            let mtime = ctx
                .stat(&repo.join("snapshots").join(&r.name))
                .map_or(0, |m| m.mtime().max(0) as u64);
            (r, named.contains(r.name.as_str()), mtime)
        })
        .collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    for (rev, _, mtime) in order.iter().take(MAX_REVISIONS) {
        s.snapshot_mtime.insert(rev.name.clone(), *mtime);
        let mut files = BTreeMap::new();
        let mut queue = vec![(repo.join("snapshots").join(&rev.name), String::new(), 0)];
        while let Some((dir, prefix, depth)) = queue.pop() {
            let (entries, t) = ctx.list_links(&dir);
            s.partial_listing |= truncated(t);
            for e in entries {
                let rel = format!("{prefix}{}", e.name);
                let path = dir.join(&e.name);
                if e.is_dir {
                    if depth + 1 < MAX_SNAPSHOT_DEPTH {
                        queue.push((path, format!("{rel}/"), depth + 1));
                    }
                    continue;
                }
                if !e.is_symlink {
                    continue;
                }
                let Some(target) = ctx.link_text(&path) else {
                    s.dangling.push(rel);
                    continue;
                };
                match lexical_join(&dir, &target) {
                    Some(t) if t.parent() == Some(blobs_dir.as_path()) => {
                        let name = t
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if s.blobs.contains_key(&name) {
                            files.insert(rel, name);
                        } else {
                            s.dangling.push(rel);
                        }
                    }
                    _ => s.outside.push(rel),
                }
            }
        }
        s.snapshots.insert(rev.name.clone(), files);
    }
    if revs.iter().filter(|r| r.is_dir).count() > MAX_REVISIONS {
        s.partial_listing = true;
    }
    s
}

/// A ref file's revision (40 hex characters), through the cache keyed on
/// the file's size and modification time: a ref moves by being
/// rewritten, which changes its mtime.
fn ref_cached(ctx: &BuildCtx, hub_root: &Path, path: &Path) -> Option<String> {
    use crate::fs_gate::MetadataExt;
    let m = ctx.stat(path)?;
    let key = format!("hf|{}|ref:{}", hub_root.display(), path.display());
    let fingerprint = format!("{CARD_FORMAT}|{}:{}:{}", m.len(), m.mtime(), m.mtime_nsec());
    let cards = ctx.cards();
    if let Some(hit) = cards.lookup(&key, &fingerprint) {
        return hit.fields.get("rev").cloned();
    }
    // Ref reads are 40 bytes and never wait for the budget: a repo's
    // revision is part of its identity, not an enrichment.
    let bytes = ctx.header(path, 64)?;
    let text = String::from_utf8_lossy(&bytes);
    let rev = text.trim();
    let mut fields = CardFields::new();
    if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        fields.insert("rev".into(), rev.to_string());
    }
    cards.insert(
        &key,
        CardEntry {
            fingerprint,
            at: ctx.observed_at,
            fields: fields.clone(),
        },
    );
    fields.get("rev").cloned()
}

/// The safetensors files that make up one copy of the weights, and a
/// note when the snapshot holds more than one copy. Shards
/// `<p>-0000i-of-0000N.safetensors` are one set; every other file is its
/// own. A transformers shard set (`model-…-of-…`) wins, then
/// `model.safetensors`, then the largest set; the others are never added
/// to it (Mistral's `consolidated.safetensors` beside its shards,
/// diffusers' fp16 copies).
fn choose_weight_set<'a>(
    files: &'a BTreeMap<String, String>,
    len_of: &dyn Fn(&str) -> u64,
) -> (Vec<(&'a String, &'a String)>, Option<String>) {
    let mut sets: BTreeMap<String, Vec<(&'a String, &'a String)>> = BTreeMap::new();
    for (n, b) in files.iter().filter(|(n, _)| n.ends_with(".safetensors")) {
        let stem = &n[..n.len() - ".safetensors".len()];
        let key = match stem.rsplit_once("-of-") {
            Some((head, total))
                if total.len() == 5
                    && total.bytes().all(|c| c.is_ascii_digit())
                    && head.len() > 6
                    && head[head.len() - 6..].starts_with('-')
                    && head[head.len() - 5..].bytes().all(|c| c.is_ascii_digit()) =>
            {
                format!("{}-*-of-{total}", &head[..head.len() - 6])
            }
            _ => n.clone(),
        };
        sets.entry(key).or_default().push((n, b));
    }
    if sets.len() <= 1 {
        return (sets.into_values().next().unwrap_or_default(), None);
    }
    let n = sets.len();
    let pick = sets
        .keys()
        .find(|k| k.starts_with("model-*-of-"))
        .or_else(|| sets.keys().find(|k| *k == "model.safetensors"))
        .cloned()
        .or_else(|| {
            sets.iter()
                .max_by_key(|(_, v)| v.iter().map(|(_, b)| len_of(b)).sum::<u64>())
                .map(|(k, _)| k.clone())
        })
        .unwrap_or_default();
    let note =
        format!("counted from {pick}, one of {n} weight sets here (the others are not added)");
    (sets.remove(&pick).unwrap_or_default(), Some(note))
}

fn is_tokenizer(name: &str) -> bool {
    matches!(
        name.rsplit('/').next().unwrap_or(name),
        "tokenizer.json" | "tokenizer.model" | "tokenizer_config.json" | "vocab.json"
    )
}

/// The card fields of one revision, through the cache.
fn hub_card(
    ctx: &BuildCtx,
    key: &str,
    files: &BTreeMap<String, String>,
    scan: &RepoScan,
) -> Option<CardEntry> {
    let cards: &CardCache = ctx.cards();
    let len_of = |blob: &str| scan.blobs.get(blob).map_or(0, |f| f.len);
    let index = files.get("model.safetensors.index.json").cloned();
    let (weights, set_note) = choose_weight_set(files, &len_of);
    let gguf = files
        .iter()
        .filter(|(n, _)| n.ends_with(".gguf"))
        .max_by_key(|(_, b)| len_of(b));
    let mut consulted: Vec<(&String, &String)> = files
        .iter()
        .filter(|(n, _)| {
            matches!(
                n.as_str(),
                "README.md" | "config.json" | "model.safetensors.index.json"
            )
        })
        .collect();
    consulted.extend(weights.iter().copied());
    consulted.extend(gguf);
    let index_pair = index
        .as_ref()
        .map(|b| ("model.safetensors.index.json".to_string(), b.clone()));
    let largest_blob = weights
        .iter()
        .map(|(_, b)| *b)
        .max_by_key(|b| len_of(b))
        .or(gguf.map(|(_, b)| b))
        .and_then(|b| scan.blobs.get(b));
    let defer = largest_blob.is_some_and(FileFact::read_would_move_atime);
    let fingerprint = format!(
        "{CARD_FORMAT}|{}|tok={}",
        consulted
            .iter()
            .map(|(n, b)| format!("{n}={b}:{}", len_of(b)))
            .collect::<Vec<_>>()
            .join(","),
        files.keys().any(|n| is_tokenizer(n))
    );
    if let Some(hit) = cards.lookup(key, &fingerprint) {
        // A count put off because the read would have set the access
        // time is read once that is no longer so.
        if !(hit.fields.contains_key("params_deferred") && !defer) {
            return Some(hit);
        }
    }
    // A revision with no file to read costs nothing to describe.
    if !consulted.is_empty() && !cards.try_spend() {
        return None;
    }
    let mut fields = CardFields::new();
    let read = |blob: &str| {
        scan.blobs
            .get(blob)
            .and_then(|f| ctx.header(&f.path, model_cards::MAX_CARD_READ))
    };
    // A safetensors header: the 64 KiB a header usually fits in, and the
    // full bound only when its length prefix says it needs more.
    let read_weights = |blob: &str| -> Option<Vec<u8>> {
        let f = scan.blobs.get(blob)?;
        let first = ctx.header(&f.path, 64 * 1024)?;
        let claimed = first
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))?;
        if claimed.saturating_add(8) <= first.len() as u64 || first.len() < 64 * 1024 {
            return Some(first);
        }
        if claimed.saturating_add(8) > model_cards::MAX_CARD_READ as u64 {
            return Some(first);
        }
        ctx.header(&f.path, claimed as usize + 8)
    };
    if let Some(b) = files.get("README.md")
        && let Some(bytes) = read(b)
    {
        let text = String::from_utf8_lossy(&bytes);
        fields.extend(model_cards::front_matter(&text));
        if let Some(p) = model_cards::first_paragraph(&text) {
            fields.insert("card_text".into(), p);
        }
    }
    if let Some(b) = files.get("config.json")
        && let Some(bytes) = read(b)
    {
        fields.extend(model_cards::config_fields(&String::from_utf8_lossy(&bytes)));
    }
    if files.keys().any(|n| is_tokenizer(n)) {
        fields.insert("tokenizer".into(), "present".into());
    }
    // The largest weight file's access time, before and after swamp's
    // own read, so a later pass can tell that read from a real use.
    let largest = largest_blob;
    if defer {
        fields.insert(
            "params_deferred".into(),
            "not read yet: reading the weight file now would set its access time, the last-read fact shown here; it is read once the file has been opened since its last change".into(),
        );
    } else if let Some(f) = largest {
        fields.insert("read_blob".into(), f.path.to_string_lossy().into_owned());
        fields.insert("read_atime_before".into(), f.atime.to_string());
    }
    // The shard set an index names, when there is one.
    let indexed: Option<BTreeSet<String>> = index_pair.as_ref().and_then(|(_, b)| {
        let bytes = read(b)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(
            v.get("weight_map")?
                .as_object()?
                .values()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
        )
    });
    let mut weights = weights;
    if let Some(set) = &indexed {
        weights = files.iter().filter(|(n, _)| set.contains(*n)).collect();
        if weights.len() < set.len() {
            fields.insert(
                "params_unread".into(),
                format!(
                    "{} of the {} weight files model.safetensors.index.json names are here",
                    weights.len(),
                    set.len()
                ),
            );
            weights.clear();
        }
    }
    // A shard set with shards missing gives a lower bound, not a count.
    let expected = weights.first().and_then(|(n, _)| {
        let stem = n.strip_suffix(".safetensors")?;
        stem.rsplit_once("-of-")?.1.parse::<usize>().ok()
    });
    if indexed.is_none()
        && let Some(e) = expected
        && e != weights.len()
    {
        fields.insert(
            "params_unread".into(),
            format!("{} of {e} shards are here", weights.len()),
        );
        weights.clear();
    }
    let too_many = weights.len() > model_cards::MAX_WEIGHT_HEADERS;
    if too_many {
        fields.insert(
            "params_unread".into(),
            format!(
                "{} weight files, more than the {} headers read per revision",
                weights.len(),
                model_cards::MAX_WEIGHT_HEADERS
            ),
        );
    }
    if let Some(n) = &set_note
        && indexed.is_none()
    {
        fields.insert("params_note".into(), n.clone());
    }
    let mut total: Option<u64> = Some(0);
    let mut dtypes: BTreeSet<String> = BTreeSet::new();
    let weights: Vec<(&String, &String)> = if defer || too_many {
        Vec::new()
    } else {
        weights
    };
    for (name, blob) in &weights {
        match read_weights(blob).map(|b| model_cards::safetensors_header(&b)) {
            Some(model_cards::WeightHeader::Read { params, dtypes: d }) => {
                total = total.and_then(|t| t.checked_add(params));
                dtypes.extend(d);
            }
            Some(model_cards::WeightHeader::TooLarge { claimed }) => {
                total = None;
                fields.insert(
                    "params_unread".into(),
                    format!("{name}: header claims {claimed} bytes, over the 1 MiB read bound"),
                );
            }
            Some(model_cards::WeightHeader::Malformed(why)) => {
                total = None;
                fields.insert("params_unread".into(), format!("{name}: {why}"));
            }
            None => {
                total = None;
                fields.insert("params_unread".into(), format!("{name}: could not be read"));
            }
        }
    }
    if !weights.is_empty()
        && let Some(t) = total
    {
        fields.insert("params".into(), t.to_string());
        fields.insert(
            "params_source".into(),
            match (&indexed, &set_note) {
                (Some(_), _) => format!(
                    "exact, from the {} safetensors headers model.safetensors.index.json names",
                    weights.len()
                ),
                _ => format!("exact, from {} safetensors header(s)", weights.len()),
            },
        );
        if !dtypes.is_empty() {
            fields.insert(
                "dtypes".into(),
                dtypes.into_iter().collect::<Vec<_>>().join("+"),
            );
        }
    }
    if let Some((_, b)) = gguf.filter(|_| !defer)
        && let Some(g) = read(b).and_then(|bytes| model_cards::gguf_header(&bytes))
    {
        if let Some(a) = g.architecture {
            fields.insert("gguf_architecture".into(), a);
        }
        if let Some(n) = g.name {
            fields.insert("gguf_name".into(), n);
        }
        if let Some(s) = g.size_label {
            fields.insert("gguf_size_label".into(), s);
        }
        if let Some(q) = g.quantization {
            fields.insert("quantization".into(), q);
        }
        if let (Some(p), false) = (g.params, fields.contains_key("params")) {
            fields.insert("params".into(), p.to_string());
            fields.insert(
                "params_source".into(),
                "exact, from the GGUF tensor infos".into(),
            );
        }
    }
    if let Some(f) = largest.filter(|_| !defer)
        && let Some(after) = file_fact(ctx, &f.path)
    {
        fields.insert("read_atime_after".into(), after.atime.to_string());
    }
    let entry = CardEntry {
        fingerprint,
        at: ctx.observed_at,
        fields,
    };
    cards.insert(key, entry.clone());
    Some(entry)
}

/// The key a hub repo revision's card is stored under; the store root
/// leads it so entries of a store this pass did not reach are kept.
pub fn hub_card_key(store: &Path, folder: &str, rev: &str) -> String {
    format!("hf|{}|{folder}@{rev}", store.display())
}

fn hub(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let root = &container.path;
    let (entries, t) = ctx.list_links(root);
    let mut root_unit = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Container)
        .supported_with_reason(
            "Hugging Face hub cache layout: models--, datasets--, spaces-- repo folders of \
             blobs/, refs/ and snapshots/<revision>/ links (huggingface_hub cache guide)",
        )
        .membership(Membership::Unknown)
        .consequence("each repo is downloaded again from huggingface.co when a program asks for it")
        .no_action_because("the hub cache is shared by every program that uses huggingface_hub");
    if truncated(t) {
        root_unit =
            root_unit.limit("the hub folder listing was cut short: repos past it are not listed");
    }
    let mut units = vec![root_unit.build()];
    let mut scans: Vec<(String, &'static str, String, RepoScan)> = Vec::new();
    for e in entries.iter().filter(|e| e.is_dir && !e.is_symlink) {
        if let Some((kind, id)) = hub_repo_of(&e.name).map(|(k, id)| (k, clean(&id))) {
            let scan = scan_repo(ctx, root, &root.join(&e.name));
            scans.push((e.name.clone(), kind, id, scan));
        }
    }
    // A blob in the hub's shared folder is charged to the first repo
    // (by folder name) that links to it, so the units sum to the store.
    let mut charged_to: HashMap<PathBuf, String> = HashMap::new();
    for (folder, _, _, scan) in &scans {
        for name in &scan.shared {
            if let Some(f) = scan.blobs.get(name) {
                charged_to
                    .entry(f.path.clone())
                    .or_insert_with(|| folder.clone());
            }
        }
    }
    let mut shared_charged: u64 = 0;
    let mut newest_shared_read: u64 = 0;
    for (folder, kind, repo_id, scan) in &scans {
        let dir = root.join(folder);
        let own = ctx.folded().get(&dir);
        let mut repo_bytes = own.map_or(0, |d| d.allocated_total);
        let mut in_shared: u64 = 0;
        let mut also_charged_elsewhere: u64 = 0;
        for name in &scan.shared {
            let Some(f) = scan.blobs.get(name) else {
                continue;
            };
            if charged_to.get(&f.path) == Some(folder) {
                repo_bytes += f.allocated;
                in_shared += f.allocated;
                shared_charged += f.allocated;
                newest_shared_read = newest_shared_read.max(f.atime);
            } else {
                also_charged_elsewhere += f.allocated;
            }
        }
        let main = scan
            .refs
            .iter()
            .find(|(r, _)| r == "main")
            .map(|(_, v)| v.clone());
        let rev = main
            .clone()
            .filter(|m| scan.snapshots.contains_key(m))
            .or_else(|| {
                scan.snapshot_mtime
                    .iter()
                    .max_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)))
                    .map(|(r, _)| r.clone())
            });
        let short = |r: &str| r.chars().take(8).collect::<String>();
        let downloadable = !scan.refs.is_empty()
            || scan
                .snapshots
                .keys()
                .any(|r| r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()));
        let consequence = if downloadable {
            let at = rev
                .clone()
                .or(main.clone())
                .map(|r| short(&r))
                .unwrap_or_default();
            let what = if *kind == "model" {
                repo_id.clone()
            } else {
                format!("{kind} {repo_id}")
            };
            format!(
                "downloaded again from huggingface.co ({what}@{at}) when needed; size {}",
                bytes(repo_bytes)
            )
        } else {
            "cannot be regenerated (no source recorded)".to_string()
        };
        let mut b = NestedUnitBuilder::new(container, ArtifactRole::SharedStoreEntry, dir.clone())
            .is_dir(true)
            .supported_with_reason(format!("Hugging Face hub {kind} repo folder"))
            .membership(Membership::Unknown)
            .bytes_on_basis(repo_bytes, AccountingBasis::Allocated)
            .variant(ArtifactVariant {
                package: Some(repo_id.clone()),
                version: rev.as_deref().map(short),
                configuration: Some((*kind).to_string()),
                ..Default::default()
            })
            .consequence(consequence)
            .no_action_because("a hub repo folder is read by whichever program downloaded it");
        let complete = own.is_some_and(|d| d.complete) && !scan.partial_listing;
        b = b.complete(complete);
        if own.is_none() {
            b = b.limit("this folder was not measured by the walk this pass");
        }
        if let Some(newest) = scan.blobs.values().map(|f| f.mtime).max() {
            b = b.modified(newest, TimeSource::FileModification);
        }
        // Revisions and which ref points where.
        let refs_text = if scan.refs.is_empty() {
            "no refs".to_string()
        } else {
            scan.refs
                .iter()
                .map(|(r, v)| format!("{r} -> {}", short(v)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let revs_text = if scan.snapshots.is_empty() {
            "no snapshot".to_string()
        } else {
            format!(
                "{} revision(s): {}",
                scan.snapshots.len(),
                scan.snapshots
                    .keys()
                    .map(|r| short(r))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let files: usize = rev
            .as_ref()
            .and_then(|r| scan.snapshots.get(r))
            .map_or(0, BTreeMap::len);
        b = b.evidence(
            REVISION_EVIDENCE,
            format!(
                "{refs_text}; {revs_text}; {files} file(s) in the shown revision; {} blob(s)",
                scan.blobs.len()
            ),
            Confidence::High,
        );
        if scan.snapshots.is_empty() && !scan.refs.is_empty() {
            b = b.limit(
                "no snapshot: a ref is recorded but no file of that revision is here (a download that did not finish, or one whose files were removed)",
            );
        }
        if scan.incomplete.0 > 0 {
            b = b.limit(format!(
                "incomplete download: {} .incomplete file(s), {}",
                scan.incomplete.0,
                bytes(scan.incomplete.1)
            ));
        }
        if !scan.dangling.is_empty() {
            b = b.limit(format!(
                "{} snapshot link(s) point at a blob that is not there (first: {})",
                scan.dangling.len(),
                scan.dangling[0]
            ));
        }
        if !scan.outside.is_empty() {
            b = b.limit(format!(
                "{} link(s) point outside this repo's blobs/ and the hub's shared blobs/: not followed, not counted (first: {})",
                scan.outside.len(),
                scan.outside[0]
            ));
        }
        if in_shared > 0 {
            b = b.limit(format!(
                "moving this folder frees about {}; {} stays in the hub's shared blobs/",
                bytes(repo_bytes.saturating_sub(in_shared)),
                bytes(in_shared)
            ));
        }
        for l in &scan.linked {
            b = b.limit(format!("{l} is a link, not followed"));
        }
        if also_charged_elsewhere > 0 {
            b = b.limit(format!(
                "also links to {} of shared blobs counted under another repo",
                bytes(also_charged_elsewhere)
            ));
        }
        if scan.partial_listing {
            b = b.limit("a listing inside this repo was cut short: counts are a lower bound");
        }
        // The card, through the pass-through cache.
        if let Some(r) = &rev
            && let Some(files) = scan.snapshots.get(r)
        {
            let key = hub_card_key(root, folder, r);
            match hub_card(ctx, &key, files, scan) {
                Some(card) => {
                    b = add_card_evidence(b, &card.fields);
                    if let Some(f) = card.fields.get("read_blob").and_then(|p| {
                        scan.blobs
                            .values()
                            .find(|f| f.path.to_string_lossy() == p.as_str())
                    }) {
                        let name = files
                            .iter()
                            .find(|(_, blob)| {
                                scan.blobs.get(*blob).is_some_and(|x| x.path == f.path)
                            })
                            .map(|(n, _)| n.clone())
                            .unwrap_or_default();
                        let (at, label) = last_read(f, Some(&card), &name);
                        b = b.evidence(
                            LAST_READ_EVIDENCE,
                            format!("{at}|{label}"),
                            Confidence::Medium,
                        );
                    }
                }
                None => {
                    b = b.limit(format!(
                        "what it is: not yet read (this pass's {} new card reads were used; the next observe reads it)",
                        model_cards::NEW_PARSES_PER_PASS
                    ));
                }
            }
        }
        if !b.has_evidence(LAST_READ_EVIDENCE)
            && let Some(f) = scan.blobs.values().max_by_key(|f| f.len)
        {
            b = b.evidence(
                LAST_READ_EVIDENCE,
                format!("{}|file access time of its largest blob", f.atime),
                Confidence::Medium,
            );
        }
        units.push(b.build());
    }
    // The hub's shared blob folder: whatever is not charged to a repo.
    let shared_dir = root.join("blobs");
    if let Some(d) = ctx.folded().get(&shared_dir) {
        let rest = d.allocated_total.saturating_sub(shared_charged);
        let referenced: BTreeSet<&PathBuf> = charged_to.keys().collect();
        let (mut n, mut unref_bytes, mut partial) = (0usize, 0u64, false);
        let (subs, t) = if real_dir(ctx, &shared_dir) {
            ctx.list_links(&shared_dir)
        } else {
            (Vec::new(), Truncation::Complete)
        };
        partial |= truncated(t);
        let linked_subs = subs.iter().filter(|s| s.is_symlink).count();
        for s in subs.iter().filter(|s| s.is_dir && !s.is_symlink) {
            let (files, t) = ctx.list_links(&shared_dir.join(&s.name));
            partial |= truncated(t);
            for f in files.iter().filter(|f| !f.is_dir && !f.is_symlink) {
                let p = shared_dir.join(&s.name).join(&f.name);
                if !referenced.contains(&p)
                    && let Some(fact) = file_fact(ctx, &p)
                {
                    n += 1;
                    unref_bytes += fact.allocated;
                }
            }
        }
        let mut b = NestedUnitBuilder::new(container, ArtifactRole::Residual, shared_dir.clone())
            .is_dir(true)
            .supported_with_reason("the hub's shared blobs/ folder that repo blobs/ entries link into")
            .evidence(
                LAST_READ_EVIDENCE,
                format!(
                    "{}|newest last read of the repos whose blobs are here",
                    newest_shared_read
                ),
                Confidence::Medium,
            )
            .bytes_on_basis(rest, AccountingBasis::Allocated)
            .complete(d.complete && !partial)
            .consequence(
                "repo folders above link into this folder; moving it leaves their links pointing at nothing until each is downloaded again",
            )
            .no_action_because("repo folders in this cache link into it");
        if linked_subs > 0 {
            b = b.limit(format!(
                "{linked_subs} entr(ies) here are links: not followed, not counted"
            ));
        }
        b = if n == 0 {
            b.limit("every file here is linked from a repo above (counted there)")
        } else {
            b.limit(format!(
                "{n} file(s), {}, not referenced by any repo's blobs/",
                bytes(unref_bytes)
            ))
        };
        units.push(b.build());
    }
    units
}

// ---------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------

/// `model:tag` as `ollama pull` spells it: the default registry and
/// namespace are left out, as Ollama itself does.
pub fn ollama_name(host: &str, ns: &str, model: &str, tag: &str) -> String {
    match (host, ns) {
        ("registry.ollama.ai", "library") => format!("{model}:{tag}"),
        ("registry.ollama.ai", ns) => format!("{ns}/{model}:{tag}"),
        (host, ns) => format!("{host}/{ns}/{model}:{tag}"),
    }
}

fn blob_name(digest: &str) -> String {
    digest.replacen(':', "-", 1)
}

const MODEL_MEDIA: &str = "application/vnd.ollama.image.model";

struct Tag {
    path: PathBuf,
    name: String,
    /// Config digest, then layer digests, with sizes as the manifest
    /// states them.
    layers: Vec<(String, String, u64)>,
    config: Option<String>,
}

fn ollama_manifest_cached(
    ctx: &BuildCtx,
    store: &Path,
    path: &Path,
) -> Result<Vec<(String, String, u64)>, String> {
    use crate::fs_gate::MetadataExt;
    let cards = ctx.cards();
    let Some(m) = ctx.stat(path) else {
        return Err("the manifest could not be read".into());
    };
    let key = format!("ollama|{}|{}", store.display(), path.display());
    let fingerprint = format!("{CARD_FORMAT}|{}:{}:{}", m.len(), m.mtime(), m.mtime_nsec());
    let decode = |f: &CardFields| -> Vec<(String, String, u64)> {
        f.iter()
            .filter(|(k, _)| k.as_str() != "error")
            .map(|(_, v)| v)
            .filter_map(|v| {
                let mut it = v.splitn(3, '|');
                Some((
                    it.next()?.to_string(),
                    it.next()?.to_string(),
                    it.next()?.parse().ok()?,
                ))
            })
            .collect()
    };
    if let Some(hit) = cards.lookup(&key, &fingerprint) {
        // A manifest that could not be parsed is remembered as such for
        // its size and mtime: it does not take a read every pass.
        if let Some(e) = hit.fields.get("error") {
            return Err(e.clone());
        }
        return Ok(decode(&hit.fields));
    }
    if !cards.try_spend() {
        return Err(
            "not yet read (this pass's new reads were used; the next observe reads it)".into(),
        );
    }
    let parsed = ctx
        .header(path, 256 * 1024)
        .ok_or("the manifest could not be read")
        .and_then(|b| {
            model_cards::ollama_manifest(&String::from_utf8_lossy(&b))
                .ok_or("the manifest is not one this reader understands")
        });
    let (config, layers) = match parsed {
        Ok(p) => p,
        Err(why) => {
            let mut fields = CardFields::new();
            fields.insert("error".into(), why.to_string());
            cards.insert(
                &key,
                CardEntry {
                    fingerprint,
                    at: ctx.observed_at,
                    fields,
                },
            );
            return Err(why.to_string());
        }
    };
    let mut fields = CardFields::new();
    for (i, l) in std::iter::once(&config).chain(layers.iter()).enumerate() {
        fields.insert(
            format!("{i:04}"),
            format!("{}|{}|{}", l.media_type, l.digest, l.size),
        );
    }
    let all = decode(&fields);
    cards.insert(
        &key,
        CardEntry {
            fingerprint,
            at: ctx.observed_at,
            fields,
        },
    );
    Ok(all)
}

fn ollama_config_cached(ctx: &BuildCtx, store: &Path, digest: &str) -> Option<CardFields> {
    let cards = ctx.cards();
    let key = format!("ollama-config|{}|{digest}", store.display());
    let fingerprint = CARD_FORMAT.to_string();
    if let Some(hit) = cards.lookup(&key, &fingerprint) {
        return Some(hit.fields);
    }
    if !cards.try_spend() {
        return None;
    }
    let text = ctx.header(&store.join("blobs").join(blob_name(digest)), 256 * 1024)?;
    let fields = model_cards::ollama_config(&String::from_utf8_lossy(&text));
    cards.insert(
        &key,
        CardEntry {
            fingerprint,
            at: ctx.observed_at,
            fields: fields.clone(),
        },
    );
    Some(fields)
}

fn ollama(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let root = &container.path;
    let mut root_unit = NestedUnitBuilder::container_root(container, ctx, ArtifactRole::Container)
        .supported_with_reason(
            "Ollama model store layout: manifests/<registry>/<namespace>/<model>/<tag> naming \
             sha256 layers in blobs/ (Ollama FAQ)",
        )
        .membership(Membership::Unknown)
        .consequence("each model is downloaded again with `ollama pull` when the registry has it")
        .no_action_because("Ollama reads every model's layers from this store");
    // manifests/<host>/<ns>/<model>/<tag>
    let mut tags: Vec<Tag> = Vec::new();
    let mut unread: Vec<(PathBuf, String, String)> = Vec::new();
    let mut partial = false;
    let manifests = root.join("manifests");
    let dirs = |p: &Path, partial: &mut bool| -> Vec<String> {
        let (e, t) = ctx.list_links(p);
        *partial |= truncated(t);
        e.into_iter()
            .filter(|e| e.is_dir && !e.is_symlink)
            .map(|e| e.name)
            .collect()
    };
    let manifests_real = real_dir(ctx, &manifests);
    if !manifests_real && ctx.stat(&manifests).is_some() {
        root_unit = root_unit.limit("manifests/ is a link, not followed");
    }
    let hosts = if manifests_real {
        dirs(&manifests, &mut partial)
    } else {
        Vec::new()
    };
    for host in hosts {
        for ns in dirs(&manifests.join(&host), &mut partial) {
            for model in dirs(&manifests.join(&host).join(&ns), &mut partial) {
                let mdir = manifests.join(&host).join(&ns).join(&model);
                let (files, t) = ctx.list_links(&mdir);
                partial |= truncated(t);
                for f in files.iter().filter(|f| !f.is_dir && !f.is_symlink) {
                    let path = mdir.join(&f.name);
                    let name = clean(&ollama_name(&host, &ns, &model, &f.name));
                    match ollama_manifest_cached(ctx, root, &path) {
                        Ok(layers) => {
                            let config = layers.first().map(|(_, d, _)| d.clone());
                            tags.push(Tag {
                                path,
                                name,
                                layers,
                                config,
                            });
                        }
                        Err(why) => unread.push((path, name, why)),
                    }
                }
            }
        }
    }
    // Every blob on disk, one lstat each.
    let blobs_dir = root.join("blobs");
    let blobs_real = real_dir(ctx, &blobs_dir);
    if !blobs_real && ctx.stat(&blobs_dir).is_some() {
        root_unit = root_unit.limit("blobs/ is a link, not followed: no layer is counted");
    }
    let (entries, t) = if blobs_real {
        ctx.list_links(&blobs_dir)
    } else {
        (Vec::new(), Truncation::Complete)
    };
    partial |= truncated(t);
    let mut on_disk: BTreeMap<String, FileFact> = BTreeMap::new();
    let mut incomplete = (0usize, 0u64);
    for e in entries.iter().filter(|e| !e.is_dir && !e.is_symlink) {
        if let Some(f) = file_fact(ctx, &blobs_dir.join(&e.name)) {
            if e.name.ends_with("-partial") || e.name.contains("-partial-") {
                incomplete.0 += 1;
                incomplete.1 += f.allocated;
            } else {
                on_disk.insert(e.name.clone(), f);
            }
        }
    }
    if partial {
        root_unit =
            root_unit.limit("a listing in this store was cut short: models past it are not listed");
    }
    if incomplete.0 > 0 {
        root_unit = root_unit.limit(format!(
            "incomplete download: {} partial blob file(s), {}",
            incomplete.0,
            bytes(incomplete.1)
        ));
    }
    let mut units = vec![root_unit.build()];
    tags.sort_by(|a, b| a.path.cmp(&b.path));
    let mut users: HashMap<String, usize> = HashMap::new();
    for t in &tags {
        let mut seen = BTreeSet::new();
        for (_, d, _) in &t.layers {
            if seen.insert(d.clone()) {
                *users.entry(blob_name(d)).or_default() += 1;
            }
        }
    }
    let mut charged: BTreeSet<String> = BTreeSet::new();
    let mut charged_bytes: u64 = 0;
    for t in &tags {
        let (mut mine, mut shared, mut total, mut missing) = (0u64, 0u64, 0u64, 0usize);
        let mut model_blob: Option<&FileFact> = None;
        let mut seen = BTreeSet::new();
        for (media, d, _) in &t.layers {
            let name = blob_name(d);
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(f) = on_disk.get(&name) else {
                missing += 1;
                continue;
            };
            total += f.allocated;
            if users.get(&name).copied().unwrap_or(0) > 1 {
                shared += f.allocated;
            }
            if charged.insert(name.clone()) {
                mine += f.allocated;
            }
            if media == MODEL_MEDIA {
                model_blob = Some(f);
            }
        }
        charged_bytes += mine;
        let fields = t
            .config
            .as_deref()
            .and_then(|d| ollama_config_cached(ctx, root, d));
        let mut b = NestedUnitBuilder::new(container, ArtifactRole::SharedStoreEntry, t.path.clone())
            .supported_with_reason("Ollama manifest naming its layers by sha256 digest")
            .membership(Membership::Unknown)
            .bytes_on_basis(mine, AccountingBasis::Allocated)
            .complete(missing == 0 && !partial)
            .variant(ArtifactVariant {
                package: Some(t.name.clone()),
                configuration: Some("ollama model".into()),
                ..Default::default()
            })
            .consequence(format!(
                "downloaded again with `ollama pull {}` when needed, if the registry has it (a model made with `ollama create` exists only here); size {}",
                t.name,
                bytes(total)
            ))
            .no_action_because("its layers may be shared with other models in this store")
            .evidence(
                REVISION_EVIDENCE,
                format!("tag {}; {} layer(s), {} of them on disk", t.name, seen.len(), seen.len() - missing),
                Confidence::High,
            )
            .limit(format!(
                "moving this manifest frees none of its layers: the layers ({}) stay in blobs/; `ollama rm {}` removes the model and the layers no other model uses",
                bytes(total),
                t.name
            ));
        if let Some(m) = ctx.stat(&t.path) {
            use crate::fs_gate::MetadataExt;
            b = b.modified(m.mtime().max(0) as u64, TimeSource::FileModification);
        }
        if shared > 0 {
            b = b.limit(format!(
                "{} of its layers are shared with other models",
                bytes(shared)
            ));
        }
        if mine < total {
            b = b.limit(format!(
                "{} of its layers are counted under another model above",
                bytes(total - mine)
            ));
        }
        if missing > 0 {
            b = b.limit(format!(
                "{missing} layer(s) the manifest names are not in blobs/"
            ));
        }
        match &fields {
            Some(f) => b = add_card_evidence(b, f),
            None => {
                b = b.limit("what it is: not yet read (the next observe reads its config)");
            }
        }
        if let Some(f) = model_blob {
            b = b.evidence(
                LAST_READ_EVIDENCE,
                format!("{}|file access time of its model layer", f.atime),
                Confidence::Medium,
            );
        }
        units.push(b.build());
    }
    for (path, name, why) in unread {
        units.push(
            NestedUnitBuilder::new(container, ArtifactRole::SharedStoreEntry, path)
                .supported_with_reason("Ollama manifest")
                .variant(ArtifactVariant {
                    package: Some(name),
                    configuration: Some("ollama model".into()),
                    ..Default::default()
                })
                .limit(format!("manifest: {why}"))
                .no_action_because("its layers may be shared with other models in this store")
                .build(),
        );
    }
    if let Some(d) = ctx.folded().get(&blobs_dir) {
        let referenced: BTreeSet<String> = users.keys().cloned().collect();
        let loose: Vec<&FileFact> = on_disk
            .iter()
            .filter(|(n, _)| !referenced.contains(*n))
            .map(|(_, f)| f)
            .collect();
        let loose_bytes: u64 = loose.iter().map(|f| f.allocated).sum();
        let mut b = NestedUnitBuilder::new(container, ArtifactRole::Residual, blobs_dir.clone())
            .is_dir(true)
            .supported_with_reason("Ollama's content-addressed layer folder")
            .bytes_on_basis(d.allocated_total.saturating_sub(charged_bytes), AccountingBasis::Allocated)
            .complete(d.complete && !partial)
            .consequence(
                "every model above reads its layers from this folder; moving it leaves each model without its layers until `ollama pull` downloads them again",
            )
            .no_action_because("every model in this store reads its layers here");
        b = if loose.is_empty() {
            b.limit("every blob here is named by a manifest (counted under that model)")
        } else {
            b.limit(format!(
                "{} blob(s), {}, not referenced by any manifest",
                loose.len(),
                bytes(loose_bytes)
            ))
        };
        units.push(b.build());
    }
    units
}

/// The store a model-store unit was identified in: the path of its
/// container's own row. A unit belongs to that store only, never to an
/// ancestor folder that is a unit too (`~/.cache/huggingface` above the
/// hub cache).
fn store_of<'a>(u: &NestedArtifact, interiors: &'a [NestedArtifact]) -> Option<&'a Path> {
    let c = u.container_id.as_deref()?;
    interiors
        .iter()
        .find(|i| i.id == c && i.container_id.as_deref() == Some(c))
        .map(|i| i.path.as_path())
}

/// One model, as the Reclaim and External views and the JSON show it:
/// what it is, which revision or tag, its size, when its weights were
/// last read, and what getting it back costs. Built from the stored
/// units only (a pure read).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModelRow {
    pub path: String,
    /// The repo id (`org/name`) or `model:tag`.
    pub name: String,
    /// `model`, `dataset`, `space` or `ollama model`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub bytes: u64,
    /// The one-line "what it is"; absent when the files state nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// The model card's first paragraph, sanitized and bounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    /// Every field the files (or the Hub) stated, `key: value`.
    pub fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_at: Option<u64>,
    /// `Sep 29 (file access time of model.safetensors)`, or `no record`.
    pub last_read: String,
    pub regeneration: String,
    /// Revisions and refs (`main -> e613edc6; 1 revision(s)`) or the tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<String>,
    /// The Hub's answer, with its fetch date, or `off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// Facts about the bytes: shared blobs, incomplete downloads,
    /// dangling links, what moving the folder leaves behind.
    pub facts: Vec<String>,
}

/// The model rows of the store at `unit_path`, largest first.
pub fn model_rows(unit_path: &Path, interiors: &[NestedArtifact], now: u64) -> Vec<ModelRow> {
    let mut rows: Vec<ModelRow> = interiors
        .iter()
        .filter(|i| i.present && store_of(i, interiors) == Some(unit_path))
        .filter_map(|i| model_row_of(i, now))
        .collect();
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    rows
}

/// A path as a row shows it and as [`ModelRow::path`] holds it: control,
/// bidi and zero-width characters removed. Views match a folder to its
/// model through this, never through the raw bytes.
pub fn shown_path(p: &Path) -> String {
    model_cards::sanitize(&p.display().to_string(), 4096)
}

/// One model unit as a [`ModelRow`]; `None` for anything else.
pub fn model_row_of(i: &NestedArtifact, now: u64) -> Option<ModelRow> {
    if i.adapter.as_deref() != Some("model-stores") || i.role != ArtifactRole::SharedStoreEntry {
        return None;
    }
    let one = |src: &str| {
        i.producer_evidence
            .iter()
            .find(|e| e.source == src)
            .map(|e| e.detail.clone())
    };
    let (last_read_at, last_read) = match last_read_of(i) {
        Some((at, label)) => (
            Some(at),
            format!("{} ({label})", crate::last_used::format_day(at, now)),
        ),
        None => (None, "no record".to_string()),
    };
    Some(ModelRow {
        path: shown_path(&i.path),
        name: clean(i.variant.package.as_deref().unwrap_or_default()),
        kind: i.variant.configuration.clone().unwrap_or_default(),
        revision: i.variant.version.clone(),
        bytes: i.bytes,
        about: one(CARD_EVIDENCE),
        card: one(CARD_TEXT_EVIDENCE),
        fields: i
            .producer_evidence
            .iter()
            .filter(|e| e.source == FIELD_EVIDENCE)
            .map(|e| e.detail.clone())
            .collect(),
        last_read_at,
        last_read,
        regeneration: i
            .consequence
            .clone()
            .unwrap_or_else(|| "regeneration cost not established".into()),
        revisions: one(REVISION_EVIDENCE),
        hub: one(HUB_API_EVIDENCE),
        facts: i.coverage.limits.clone(),
    })
}

/// The last-read fact a unit carries, as `(epoch, label)`.
pub fn last_read_of(u: &NestedArtifact) -> Option<(u64, String)> {
    let e = u
        .producer_evidence
        .iter()
        .find(|e| e.source == LAST_READ_EVIDENCE)?;
    let (at, label) = e.detail.split_once('|')?;
    Some((at.parse().ok().filter(|t| *t > 0)?, label.to_string()))
}

/// Gives a model store's drilldown rows (one per repo folder) and the
/// store itself the last-read facts this adapter stated, under the same
/// precedence rule every other last-used value goes through. A row with
/// no model behind it keeps what it had.
pub fn attach_last_read(
    units: &mut [crate::external::ExternalUnit],
    interiors: &[NestedArtifact],
    now: u64,
) {
    for u in units.iter_mut() {
        let mine: Vec<&NestedArtifact> = interiors
            .iter()
            .filter(|i| {
                i.adapter.as_deref() == Some("model-stores")
                    && store_of(i, interiors) == Some(u.path.as_path())
            })
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut newest: Option<u64> = None;
        for i in &mine {
            let Some((at, _)) = last_read_of(i) else {
                continue;
            };
            newest = newest.max(Some(at));
            if i.path.parent() == Some(u.path.as_path())
                && let Some(name) = i.path.file_name().map(|n| n.to_string_lossy().into_owned())
                && let Some(child) = u
                    .children
                    .iter_mut()
                    .find(|c| c.kind == crate::drilldown::ChildKind::Entry && c.name == name)
            {
                child.last_used = crate::last_used::resolve_at(None, Some(at), now);
            }
        }
        if u.last_used.at.is_none() && newest.is_some() {
            u.last_used = crate::last_used::resolve_at(None, newest, now);
        }
    }
}

#[cfg(test)]
mod tests;
