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
}

fn file_fact(ctx: &BuildCtx, path: &Path) -> Option<FileFact> {
    use std::os::unix::fs::MetadataExt;
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
    dangling: Vec<String>,
    outside: Vec<String>,
    partial_listing: bool,
}

fn scan_repo(ctx: &BuildCtx, hub_root: &Path, repo: &Path) -> RepoScan {
    let mut s = RepoScan::default();
    let blobs_dir = repo.join("blobs");
    let shared_dir = hub_root.join("blobs");
    let (entries, t) = ctx.list_links(&blobs_dir);
    s.partial_listing |= truncated(t);
    for e in entries {
        let path = blobs_dir.join(&e.name);
        if e.is_dir {
            continue;
        }
        if e.is_symlink {
            let Some(target) = ctx.read_link(&path) else {
                s.dangling.push(format!("blobs/{}", e.name));
                continue;
            };
            match lexical_join(&blobs_dir, &target) {
                Some(t) if t.starts_with(&shared_dir) || t.starts_with(&blobs_dir) => {
                    match file_fact(ctx, &t) {
                        Some(f) => {
                            s.shared.insert(e.name.clone());
                            s.blobs.insert(e.name, f);
                        }
                        None => s.dangling.push(format!("blobs/{}", e.name)),
                    }
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
    let (refs, t) = ctx.list_links(&repo.join("refs"));
    s.partial_listing |= truncated(t);
    for r in refs.iter().filter(|r| !r.is_dir && !r.is_symlink) {
        let p = repo.join("refs").join(&r.name);
        if let Some(rev) = ref_cached(ctx, hub_root, &p) {
            s.refs.push((r.name.clone(), rev));
        }
    }
    let (revs, t) = ctx.list_links(&repo.join("snapshots"));
    s.partial_listing |= truncated(t);
    for rev in revs.iter().filter(|r| r.is_dir).take(MAX_REVISIONS) {
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
                let Some(target) = ctx.read_link(&path) else {
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
    use std::os::unix::fs::MetadataExt;
    let m = ctx.stat(path)?;
    let key = format!("hf|{}|ref:{}", hub_root.display(), path.display());
    let fingerprint = format!("{CARD_FORMAT}|{}:{}:{}", m.len(), m.mtime(), m.mtime_nsec());
    let cards = ctx.cards();
    if let Some(hit) = cards.lookup(&key, &fingerprint) {
        return hit.fields.get("rev").cloned();
    }
    // Ref reads are 40 bytes and never wait for the budget: a repo's
    // revision is part of its identity, not an enrichment.
    let text = ctx.manifest(path).filter(|t| !t.truncated)?;
    let rev = text.text.trim();
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
    let mut weights: Vec<(&String, &String)> = files
        .iter()
        .filter(|(n, _)| n.ends_with(".safetensors"))
        .collect();
    weights.sort_by_key(|(_, b)| std::cmp::Reverse(len_of(b)));
    weights.truncate(model_cards::MAX_WEIGHT_HEADERS);
    let gguf = files
        .iter()
        .filter(|(n, _)| n.ends_with(".gguf"))
        .max_by_key(|(_, b)| len_of(b));
    let mut consulted: Vec<(&String, &String)> = files
        .iter()
        .filter(|(n, _)| *n == "README.md" || *n == "config.json")
        .collect();
    consulted.extend(weights.iter().copied());
    consulted.extend(gguf);
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
        return Some(hit);
    }
    // A revision with no file to read costs nothing to describe.
    if !consulted.is_empty() && !cards.try_spend() {
        return None;
    }
    let mut fields = CardFields::new();
    let read = |blob: &str| scan.blobs.get(blob).and_then(|f| ctx.header(&f.path));
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
    let largest = weights
        .first()
        .map(|(_, b)| *b)
        .or(gguf.map(|(_, b)| b))
        .and_then(|b| scan.blobs.get(b));
    if let Some(f) = largest {
        fields.insert("read_blob".into(), f.path.to_string_lossy().into_owned());
        fields.insert("read_atime_before".into(), f.atime.to_string());
    }
    let mut total: Option<u64> = Some(0);
    let mut dtypes: BTreeSet<String> = BTreeSet::new();
    for (name, blob) in &weights {
        match read(blob).map(|b| model_cards::safetensors_header(&b)) {
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
            format!("exact, from {} safetensors header(s)", weights.len()),
        );
        if !dtypes.is_empty() {
            fields.insert(
                "dtypes".into(),
                dtypes.into_iter().collect::<Vec<_>>().join("+"),
            );
        }
    }
    if let Some((_, b)) = gguf
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
    if let Some(f) = largest
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
        if let Some((kind, id)) = hub_repo_of(&e.name) {
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
            .or_else(|| scan.snapshots.keys().next_back().cloned());
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
                "{} of this repo's size is in the hub's shared blobs/ folder: moving this repo folder leaves those bytes there",
                bytes(in_shared)
            ));
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
        let (subs, t) = ctx.list_links(&shared_dir);
        partial |= truncated(t);
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
            .bytes_on_basis(rest, AccountingBasis::Allocated)
            .complete(d.complete && !partial)
            .consequence(
                "repo folders above link into this folder; moving it leaves their links pointing at nothing until each is downloaded again",
            )
            .no_action_because("repo folders in this cache link into it");
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
) -> Result<Vec<(String, String, u64)>, &'static str> {
    use std::os::unix::fs::MetadataExt;
    let cards = ctx.cards();
    let Some(m) = ctx.stat(path) else {
        return Err("the manifest could not be read");
    };
    let key = format!("ollama|{}|{}", store.display(), path.display());
    let fingerprint = format!("{CARD_FORMAT}|{}:{}:{}", m.len(), m.mtime(), m.mtime_nsec());
    let decode = |f: &CardFields| -> Vec<(String, String, u64)> {
        f.values()
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
        return Ok(decode(&hit.fields));
    }
    if !cards.try_spend() {
        return Err("not yet read (this pass's new reads were used; the next observe reads it)");
    }
    let Some(text) = ctx.manifest(path).filter(|m| !m.truncated) else {
        return Err("the manifest could not be read");
    };
    let Some((config, layers)) = model_cards::ollama_manifest(&text.text) else {
        return Err("the manifest is not one this reader understands");
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
    let text = ctx.manifest(&store.join("blobs").join(blob_name(digest)))?;
    let fields = model_cards::ollama_config(&text.text);
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
    let mut unread: Vec<(PathBuf, String, &'static str)> = Vec::new();
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
    for host in dirs(&manifests, &mut partial) {
        for ns in dirs(&manifests.join(&host), &mut partial) {
            for model in dirs(&manifests.join(&host).join(&ns), &mut partial) {
                let mdir = manifests.join(&host).join(&ns).join(&model);
                let (files, t) = ctx.list_links(&mdir);
                partial |= truncated(t);
                for f in files.iter().filter(|f| !f.is_dir && !f.is_symlink) {
                    let path = mdir.join(&f.name);
                    let name = ollama_name(&host, &ns, &model, &f.name);
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
    let (entries, t) = ctx.list_links(&blobs_dir);
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
                "moving this manifest frees none of its {} of layers in blobs/; `ollama rm {}` removes the model and the layers no other model uses",
                bytes(total),
                t.name
            ));
        if let Some(m) = ctx.stat(&t.path) {
            use std::os::unix::fs::MetadataExt;
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
            .filter(|i| i.adapter.as_deref() == Some("model-stores") && i.path.starts_with(&u.path))
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
