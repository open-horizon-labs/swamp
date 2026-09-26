//! Docker facts: images, build cache entries, and volumes, read either
//! from a mocked `docker system df -v --format json` file (tests, or the
//! CLI's `--docker-facts <file>`) or from a live, bounded call to the
//! daemon. The daemon is never allowed to hang or fail the report: an
//! unreachable/slow/erroring daemon becomes `DockerFacts::unavailable`,
//! never an `Err`.
//!
//! Beyond object identity/sizes, this module extracts the per-object
//! detail #33 asks for: `created_at`, compose service (read straight off
//! the object's own labels, no separate field needed), layer digests ->
//! `shared_with` (other images sharing >=1 layer), containers
//! referencing an image/volume (name, state, finished_at) from `docker
//! ps -a --format json` plus a batched `docker inspect` for the
//! container detail `ps` doesn't carry, a dangling flag, and build-cache
//! `last_used`/`usage_count`/`in_use`/`shared`. Every additional call
//! uses the same bounded timeout and best-effort-only failure policy as
//! the existing image-inspect call: a failure there degrades that one
//! piece of detail, never the whole report.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOCKER_TIMEOUT: Duration = Duration::from_secs(5);

/// One container's reference to an image or volume, as shown on that
/// object's row. Never a verdict: "state" and "finished_at" are facts
/// ("exited"/"running"/... and a timestamp or `None`), not a judgment
/// about whether the object is safe to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContainerRef {
    pub name: String,
    pub state: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerImageFact {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub labels: HashMap<String, String>,
    pub shared_bytes: u64,
    pub unique_bytes: u64,
    pub created_at: Option<String>,
    /// `RootFS.Layers` digests from `docker image inspect`; empty when
    /// inspect failed/timed out or the image has none recorded.
    pub layers: Vec<String>,
    /// Other images' references (repo:tag, or id when untagged) sharing
    /// at least one layer digest with this one. Computed once every
    /// image's layers are known; never includes this image itself.
    pub shared_with: Vec<String>,
    pub containers: Vec<ContainerRef>,
    /// No `RepoTags` at all (a build stage's intermediate image, or the
    /// previous image behind a moved tag) -- a fact read straight off
    /// the object, not an inference.
    pub dangling: bool,
}

/// One BuildKit build-cache record, as the daemon reported it.
///
/// Every field is the daemon's own fact, kept in the daemon's terms:
/// `created_at`/`last_used` are the daemon's records (RFC 3339 strings,
/// unparsed here), `bytes` is its logical size for this record alone --
/// a record's parents are separate records with their own sizes, never
/// included -- and `in_use`/`shared`/`reclaimable` are what the daemon
/// said, not inferences. `None` means the daemon (or the CLI version
/// asked) did not report that field, which is different from `false`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerCacheFact {
    pub id: String,
    pub bytes: u64,
    pub last_used: Option<String>,
    pub usage_count: Option<u64>,
    pub in_use: bool,
    pub shared: bool,
    /// BuildKit's record type: `regular`, `internal`, `frontend`,
    /// `source.local`, `source.git.checkout`, `exec.cachemount`, ...
    #[serde(default)]
    pub cache_type: Option<String>,
    /// The daemon's description (`[build 2/5] RUN apt-get ...`,
    /// `local source for context`). Producer evidence, never parsed for
    /// identity.
    #[serde(default)]
    pub description: Option<String>,
    /// Parent record ids. A child's size never includes a parent's.
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// `docker buildx du` only: whether the daemon would reclaim it.
    #[serde(default)]
    pub reclaimable: Option<bool>,
    /// `docker buildx du` only: whether the record is a mutable snapshot.
    #[serde(default)]
    pub mutable: Option<bool>,
    /// The buildx builder whose BuildKit instance holds this record;
    /// `None` for the daemon's own (default) builder, as `docker system
    /// df` reports it.
    #[serde(default)]
    pub builder: Option<String>,
}

/// One buildx builder, from `docker buildx ls`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DockerBuilderFact {
    pub name: String,
    pub driver: Option<String>,
    pub status: Option<String>,
}

/// What this daemon and CLI could answer, stated rather than inferred
/// from an empty list (#71: "unavailable daemon/version capability
/// explicit").
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerCapabilities {
    /// The daemon's API version (`docker version`'s `Server.ApiVersion`),
    /// when it answered.
    pub api_version: Option<String>,
    /// Why per-builder BuildKit records are not listed, when they are
    /// not: buildx missing, `buildx ls` failed, a builder's `du` timed
    /// out. Empty when every builder answered.
    pub buildx_limits: Vec<String>,
}

/// The oldest daemon API that reports build-cache records in `docker
/// system df -v` (API 1.39, Docker 18.09). An older daemon's records are
/// listed, if at all, without the detail this layer identifies by.
pub const MIN_BUILD_CACHE_API: (u32, u32) = (1, 39);

impl DockerCapabilities {
    /// Whether the daemon's API is known to predate build-cache record
    /// detail. Unknown (no version reported) is not "unsupported".
    pub fn build_cache_api_unsupported(&self) -> bool {
        let Some(v) = &self.api_version else {
            return false;
        };
        let mut parts = v.trim().split('.');
        let major: Option<u32> = parts.next().and_then(|p| p.parse().ok());
        let minor: Option<u32> = parts.next().and_then(|p| p.parse().ok());
        match (major, minor) {
            (Some(a), Some(b)) => (a, b) < MIN_BUILD_CACHE_API,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerVolumeFact {
    pub name: String,
    pub labels: HashMap<String, String>,
    pub bytes: u64,
    pub created_at: Option<String>,
    pub driver: Option<String>,
    pub containers: Vec<ContainerRef>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DockerFacts {
    pub images: Vec<DockerImageFact>,
    pub build_cache: Vec<DockerCacheFact>,
    pub volumes: Vec<DockerVolumeFact>,
    /// Set when the daemon/file could not be read: the reason text is
    /// surfaced as a report-level coverage note, never an error.
    pub unavailable: Option<String>,
    /// Builders `docker buildx ls` listed. Empty when buildx is absent or
    /// was not asked; the daemon's own builder is always implied.
    #[serde(default)]
    pub builders: Vec<DockerBuilderFact>,
    #[serde(default)]
    pub capabilities: DockerCapabilities,
    /// When these facts were fetched from the daemon, in seconds. `None`
    /// for a facts file, whose capture time swamp does not know.
    #[serde(default)]
    pub captured_at: Option<u64>,
}

/// The name `docker system df`'s own build cache is reported under.
pub const DEFAULT_BUILDER: &str = "default";

impl DockerFacts {
    /// Every builder a record or `buildx ls` names, the daemon's own
    /// first. A builder with no records is still a builder: its empty
    /// cache is a fact, not an absence.
    pub fn builder_names(&self) -> Vec<String> {
        let mut out = vec![DEFAULT_BUILDER.to_string()];
        for name in self
            .builders
            .iter()
            .map(|b| b.name.clone())
            .chain(self.build_cache.iter().filter_map(|r| r.builder.clone()))
        {
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// The records one builder holds.
    pub fn records_of(&self, builder: &str) -> Vec<&DockerCacheFact> {
        self.build_cache
            .iter()
            .filter(|r| r.builder.as_deref().unwrap_or(DEFAULT_BUILDER) == builder)
            .collect()
    }
}

/// One container as read from `docker ps -a --format json` plus a
/// best-effort `docker inspect` for the fields `ps` doesn't carry
/// (precise `FinishedAt`, volume mount names). Not part of the public
/// `DockerFacts` shape -- it exists only to drive `containers` on the
/// image/volume facts above.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ContainerFact {
    name: String,
    /// The `Image` field from `ps`: usually `repo:tag`, sometimes a bare
    /// image id -- matched against both on the image side.
    image_ref: String,
    state: String,
    finished_at: Option<String>,
    /// Volume names this container mounts (from `docker inspect`'s
    /// `Mounts[].Name`, `Type == "volume"` only).
    volume_names: Vec<String>,
}

fn value_str(v: &serde_json::Value) -> String {
    v.as_str().map(|s| s.to_string()).unwrap_or_default()
}

fn opt_value_str(v: &serde_json::Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// Parses a comma-joined `k=v,k=v` label string, the shape both the
/// fixture and `docker ... --format json` use for `Labels`.
fn parse_labels(raw: &str) -> HashMap<String, String> {
    raw.split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Parses a docker size: a plain byte count (test fixtures) or a human
/// string such as `"153MB"`, `"83.9kB"`, `"0B"` (the live CLI's own
/// `--format json` output uses decimal, not binary, units).
fn parse_size(raw: &str) -> u64 {
    let raw = raw.trim();
    if let Ok(n) = raw.parse::<u64>() {
        return n;
    }
    let Some(idx) = raw.find(|c: char| c.is_alphabetic()) else {
        return 0;
    };
    let (num, unit) = raw.split_at(idx);
    let Ok(num) = num.trim().parse::<f64>() else {
        return 0;
    };
    let mult: f64 = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "TB" => 1_000_000_000_000.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (num * mult).round() as u64
}

fn parse_value(value: &serde_json::Value) -> DockerFacts {
    let mut facts = DockerFacts::default();

    if let Some(images) = value.get("Images").and_then(|v| v.as_array()) {
        for image in images {
            let labels = image
                .get("Labels")
                .map(value_str)
                .map(|s| parse_labels(&s))
                .unwrap_or_default();
            let mut repo_tags = Vec::new();
            if let Some(tags) = image.get("RepoTags").and_then(|v| v.as_array()) {
                for t in tags {
                    if let Some(s) = t.as_str() {
                        repo_tags.push(s.to_string());
                    }
                }
            } else {
                let repo = image.get("Repository").map(value_str).unwrap_or_default();
                let tag = image.get("Tag").map(value_str).unwrap_or_default();
                if !repo.is_empty() && repo != "<none>" {
                    repo_tags.push(if tag.is_empty() || tag == "<none>" {
                        repo
                    } else {
                        format!("{repo}:{tag}")
                    });
                }
            }
            let id = image.get("ID").map(value_str).unwrap_or_default();
            let shared_bytes = image
                .get("SharedSize")
                .map(value_str)
                .map(|s| parse_size(&s))
                .unwrap_or(0);
            let unique_bytes = image
                .get("UniqueSize")
                .map(value_str)
                .map(|s| parse_size(&s))
                .unwrap_or_else(|| {
                    image
                        .get("Size")
                        .map(value_str)
                        .map(|s| parse_size(&s))
                        .unwrap_or(0)
                });
            let created_at = image.get("CreatedAt").and_then(opt_value_str);
            let dangling = repo_tags.is_empty();
            facts.images.push(DockerImageFact {
                id,
                repo_tags,
                labels,
                shared_bytes,
                unique_bytes,
                created_at,
                layers: Vec::new(),
                shared_with: Vec::new(),
                containers: Vec::new(),
                dangling,
            });
        }
    }

    if let Some(entries) = value.get("BuildCache").and_then(|v| v.as_array()) {
        for entry in entries {
            let id = entry.get("ID").map(value_str).unwrap_or_default();
            let bytes = entry
                .get("Size")
                .map(value_str)
                .map(|s| parse_size(&s))
                .unwrap_or(0);
            let last_used = entry.get("LastUsedAt").and_then(opt_value_str);
            let usage_count = entry.get("UsageCount").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let in_use = entry
                .get("InUse")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let shared = entry
                .get("Shared")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            facts.build_cache.push(DockerCacheFact {
                id,
                bytes,
                last_used,
                usage_count,
                in_use,
                shared,
                cache_type: entry.get("CacheType").and_then(opt_value_str),
                description: entry.get("Description").and_then(opt_value_str),
                parents: parents_of(entry),
                created_at: entry.get("CreatedAt").and_then(opt_value_str),
                reclaimable: None,
                mutable: None,
                builder: None,
            });
        }
    }

    if let Some(v) = value.get("Version") {
        facts.capabilities.api_version = api_version_of(v);
    }
    if let Some(rows) = value.get("Builders").and_then(|v| v.as_array()) {
        facts.builders = rows.iter().filter_map(parse_builder).collect();
    }
    if let Some(entries) = value.get("BuildxDu").and_then(|v| v.as_array()) {
        for entry in entries {
            let builder = entry.get("Builder").map(value_str).unwrap_or_default();
            if builder.is_empty() {
                continue;
            }
            if let Some(reason) = entry.get("Unavailable").and_then(opt_value_str) {
                facts
                    .capabilities
                    .buildx_limits
                    .push(format!("builder `{builder}`: {reason}"));
                continue;
            }
            let records = match (entry.get("Verbose"), entry.get("Records")) {
                (Some(text), _) => parse_buildx_du_verbose(&value_str(text)),
                (None, Some(rows)) => rows
                    .as_array()
                    .map(|a| a.iter().map(parse_buildx_record).collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            merge_builder_records(&mut facts, &builder, records);
        }
    }
    if let Some(limits) = value.get("BuildxLimits").and_then(|v| v.as_array()) {
        facts
            .capabilities
            .buildx_limits
            .extend(limits.iter().filter_map(opt_value_str));
    }

    if let Some(entries) = value.get("Volumes").and_then(|v| v.as_array()) {
        for entry in entries {
            let name = entry.get("Name").map(value_str).unwrap_or_default();
            let labels = entry
                .get("Labels")
                .map(value_str)
                .map(|s| parse_labels(&s))
                .unwrap_or_default();
            let bytes = entry
                .get("Size")
                .map(value_str)
                .map(|s| parse_size(&s))
                .unwrap_or(0);
            let driver = entry.get("Driver").and_then(opt_value_str);
            let created_at = entry.get("CreatedAt").and_then(opt_value_str);
            facts.volumes.push(DockerVolumeFact {
                name,
                labels,
                bytes,
                created_at,
                driver,
                containers: Vec::new(),
            });
        }
    }

    if let Some(entries) = value.get("ImageInspect").and_then(|v| v.as_array()) {
        merge_image_inspect(&mut facts.images, entries);
    }

    if let Some(entries) = value.get("VolumeInspect").and_then(|v| v.as_array()) {
        merge_volume_inspect(&mut facts.volumes, entries);
    }

    if let Some(entries) = value.get("Containers").and_then(|v| v.as_array()) {
        let containers: Vec<ContainerFact> = entries.iter().map(parse_container_inspect).collect();
        join_containers(&mut facts, &containers);
    }

    compute_shared_with(&mut facts.images);

    facts
}

/// `docker system df -v --format json` image rows carry no `Labels` at
/// all (confirmed against a live daemon: its image objects have exactly
/// `[Containers, CreatedAt, CreatedSince, Digest, ID, Repository,
/// SharedSize, Size, Tag, UniqueSize]`). Labels only exist in `docker
/// image inspect` output, under `Config.Labels`; layer digests live
/// under `RootFS.Layers`. This merges an `ImageInspect`-shaped array
/// (`[{"Id"|"ID", "RepoTags", "Config": {"Labels": {...}}, "RootFS":
/// {"Layers": [...]}}, ...]`, the same shape `docker image inspect
/// --format json` returns) into the df-sourced image facts, matching by
/// image ID first and falling back to a shared repo:tag.
/// `Parents` (an array, API >= 1.42) or the deprecated single `Parent`.
fn parents_of(entry: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = entry.get("Parents").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|p| p.as_str())
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
    }
    match entry.get("Parents").or_else(|| entry.get("Parent")) {
        Some(v) => value_str(v)
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect(),
        None => Vec::new(),
    }
}

fn api_version_of(v: &serde_json::Value) -> Option<String> {
    v.get("Server")
        .and_then(|s| s.get("ApiVersion"))
        .and_then(opt_value_str)
}

fn parse_builder(row: &serde_json::Value) -> Option<DockerBuilderFact> {
    let name = row.get("Name").and_then(opt_value_str)?;
    let status = row
        .get("Nodes")
        .and_then(|n| n.as_array())
        .and_then(|n| n.first())
        .and_then(|n| n.get("Status"))
        .and_then(opt_value_str)
        .or_else(|| row.get("Status").and_then(opt_value_str));
    Some(DockerBuilderFact {
        name,
        driver: row.get("Driver").and_then(opt_value_str),
        status,
    })
}

fn opt_bool(v: Option<&serde_json::Value>) -> Option<bool> {
    let v = v?;
    v.as_bool().or_else(|| match v.as_str()? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

/// One record of `docker buildx du --verbose --format json`.
fn parse_buildx_record(r: &serde_json::Value) -> DockerCacheFact {
    DockerCacheFact {
        id: r.get("ID").map(value_str).unwrap_or_default(),
        bytes: r
            .get("Size")
            .map(|v| v.as_u64().unwrap_or_else(|| parse_size(&value_str(v))))
            .unwrap_or(0),
        last_used: r.get("LastUsedAt").and_then(opt_value_str),
        usage_count: r.get("UsageCount").and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        }),
        in_use: opt_bool(r.get("InUse")).unwrap_or(false),
        shared: opt_bool(r.get("Shared")).unwrap_or(false),
        cache_type: r
            .get("Type")
            .or_else(|| r.get("CacheType"))
            .and_then(opt_value_str),
        description: r.get("Description").and_then(opt_value_str),
        parents: parents_of(r),
        created_at: r.get("CreatedAt").and_then(opt_value_str),
        reclaimable: opt_bool(r.get("Reclaimable")),
        mutable: opt_bool(r.get("Mutable")),
        builder: None,
    }
}

/// `docker buildx du --verbose`'s documented text shape: one block per
/// record, `Key:\tvalue` lines, blocks separated by a blank line, and a
/// trailing `Shared:`/`Private:`/`Reclaimable:`/`Total:` summary that
/// is not a record.
pub fn parse_buildx_du_verbose(text: &str) -> Vec<DockerCacheFact> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut fields: HashMap<String, String> = HashMap::new();
        for line in block.lines() {
            if let Some((k, v)) = line.split_once(':') {
                fields.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let Some(id) = fields.get("ID").filter(|v| !v.is_empty()).cloned() else {
            continue;
        };
        let flag = |k: &str| fields.get(k).and_then(|v| v.parse::<bool>().ok());
        out.push(DockerCacheFact {
            id,
            bytes: fields.get("Size").map(|s| parse_size(s)).unwrap_or(0),
            last_used: fields.get("Last used").filter(|v| !v.is_empty()).cloned(),
            usage_count: fields.get("Usage count").and_then(|v| v.parse().ok()),
            in_use: flag("In use").unwrap_or(false),
            shared: flag("Shared").unwrap_or(false),
            cache_type: fields.get("Type").filter(|v| !v.is_empty()).cloned(),
            description: fields.get("Description").filter(|v| !v.is_empty()).cloned(),
            parents: fields
                .get("Parents")
                .or_else(|| fields.get("Parent"))
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            created_at: fields.get("Created at").filter(|v| !v.is_empty()).cloned(),
            reclaimable: flag("Reclaimable"),
            mutable: flag("Mutable"),
            builder: None,
        });
    }
    out
}

/// One builder's records, attributed to it. The daemon's own builder is
/// already reported by `docker system df`; `buildx du` for it describes
/// the same records, so its extra fields are merged onto them by id
/// rather than listed twice.
fn merge_builder_records(facts: &mut DockerFacts, builder: &str, records: Vec<DockerCacheFact>) {
    let is_default = builder == DEFAULT_BUILDER
        || facts
            .builders
            .iter()
            .any(|b| b.name == builder && b.driver.as_deref() == Some("docker"));
    for mut r in records {
        if is_default
            && let Some(existing) = facts
                .build_cache
                .iter_mut()
                .find(|e| e.builder.is_none() && e.id == r.id)
        {
            existing.reclaimable = r.reclaimable.or(existing.reclaimable);
            existing.mutable = r.mutable.or(existing.mutable);
            if existing.parents.is_empty() {
                existing.parents = r.parents;
            }
            if existing.cache_type.is_none() {
                existing.cache_type = r.cache_type;
            }
            continue;
        }
        if !is_default {
            r.builder = Some(builder.to_string());
        }
        facts.build_cache.push(r);
    }
}

fn merge_image_inspect(images: &mut [DockerImageFact], inspect_entries: &[serde_json::Value]) {
    for entry in inspect_entries {
        let id = entry
            .get("Id")
            .or_else(|| entry.get("ID"))
            .map(value_str)
            .unwrap_or_default();
        let labels: HashMap<String, String> = entry
            .get("Config")
            .and_then(|c| c.get("Labels"))
            .and_then(|l| l.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let layers: Vec<String> = entry
            .get("RootFS")
            .and_then(|r| r.get("Layers"))
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| d.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let inspect_tags: Vec<String> = entry
            .get("RepoTags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        if labels.is_empty() && layers.is_empty() {
            continue;
        }

        for image in images.iter_mut() {
            let matches = (!id.is_empty() && image.id == id)
                || image.repo_tags.iter().any(|t| inspect_tags.contains(t));
            if matches {
                image.labels.extend(labels.clone());
                if image.layers.is_empty() {
                    image.layers = layers.clone();
                }
            }
        }
    }
}

/// Merges `docker volume inspect --format json` output (`[{"Name",
/// "CreatedAt", "Driver"}, ...]`) into the df-sourced volume facts, by
/// name.
fn merge_volume_inspect(volumes: &mut [DockerVolumeFact], inspect_entries: &[serde_json::Value]) {
    for entry in inspect_entries {
        let name = entry.get("Name").map(value_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let created_at = entry.get("CreatedAt").and_then(opt_value_str);
        let driver = entry.get("Driver").and_then(opt_value_str);
        for volume in volumes.iter_mut() {
            if volume.name == name {
                if volume.created_at.is_none() {
                    volume.created_at = created_at.clone();
                }
                if volume.driver.is_none() {
                    volume.driver = driver.clone();
                }
            }
        }
    }
}

/// Parses one `docker inspect` entry that is a container (has a `State`
/// object): name (leading `/` stripped, matching `docker ps`'s bare
/// name), image reference, state, precise `FinishedAt` (empty/zero-value
/// == never finished / still running, normalized to `None`), and the
/// volume names among its mounts.
fn parse_container_inspect(entry: &serde_json::Value) -> ContainerFact {
    let name = entry
        .get("Name")
        .map(value_str)
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image_ref = entry
        .get("Config")
        .and_then(|c| c.get("Image"))
        .map(value_str)
        .or_else(|| entry.get("Image").map(value_str))
        .unwrap_or_default();
    let state = entry
        .get("State")
        .and_then(|s| s.get("Status"))
        .map(value_str)
        .unwrap_or_default();
    let finished_at = entry
        .get("State")
        .and_then(|s| s.get("FinishedAt"))
        .and_then(opt_value_str)
        .filter(|s| !s.starts_with("0001-01-01"));
    let volume_names: Vec<String> = entry
        .get("Mounts")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| m.get("Type").and_then(|t| t.as_str()) == Some("volume"))
                .filter_map(|m| m.get("Name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ContainerFact {
        name,
        image_ref,
        state,
        finished_at,
        volume_names,
    }
}

/// Attaches each container as a [`ContainerRef`] to every image and
/// volume it references. An image match is by repo:tag or bare id
/// (docker ps's `Image` field is inconsistently one or the other
/// depending on whether the tag still exists); a volume match is by
/// name.
fn join_containers(facts: &mut DockerFacts, containers: &[ContainerFact]) {
    for c in containers {
        // A container the daemon gave us no name for tells the human
        // nothing, and rendered it as an empty `()` in the list of what
        // holds an image. Skip it rather than print a blank.
        if c.name.trim().is_empty() {
            continue;
        }
        let container_ref = ContainerRef {
            name: c.name.clone(),
            state: c.state.clone(),
            finished_at: c.finished_at.clone(),
        };
        for image in facts.images.iter_mut() {
            let matches = image.repo_tags.iter().any(|t| t == &c.image_ref)
                || image.id == c.image_ref
                || image
                    .id
                    .trim_start_matches("sha256:")
                    .starts_with(&c.image_ref);
            if matches {
                image.containers.push(container_ref.clone());
            }
        }
        for vol_name in &c.volume_names {
            for volume in facts.volumes.iter_mut() {
                if &volume.name == vol_name {
                    volume.containers.push(container_ref.clone());
                }
            }
        }
    }
}

/// Fills `shared_with` on every image whose layer set overlaps another
/// image's: an O(n^2) comparison over layer digest sets, fine at the
/// scale a single machine's `docker system df` returns (low hundreds of
/// images at most).
fn compute_shared_with(images: &mut [DockerImageFact]) {
    let refs: Vec<(String, std::collections::HashSet<String>)> = images
        .iter()
        .map(|i| {
            let name = i.repo_tags.first().cloned().unwrap_or_else(|| i.id.clone());
            (name, i.layers.iter().cloned().collect())
        })
        .collect();
    for (idx, image) in images.iter_mut().enumerate() {
        if image.layers.is_empty() {
            continue;
        }
        let own_layers: std::collections::HashSet<String> = image.layers.iter().cloned().collect();
        let mut shared_with = Vec::new();
        for (other_idx, (other_name, other_layers)) in refs.iter().enumerate() {
            if other_idx == idx || other_layers.is_empty() {
                continue;
            }
            if own_layers.intersection(other_layers).next().is_some() {
                shared_with.push(other_name.clone());
            }
        }
        shared_with.sort();
        image.shared_with = shared_with;
    }
}

/// Runs a `docker` subcommand and returns its parsed JSON stdout, bounded
/// by `timeout`. Returns `Err(reason)` on any failure (missing binary,
/// non-zero exit, timeout, bad JSON) -- the caller decides whether that
/// failure is fatal to the whole report or just means "no enrichment".
/// Docker's `--format json` on a list subcommand (`ps`, sometimes
/// others) emits newline-delimited JSON objects rather than one array,
/// so on a whole-output parse failure this also retries as NDJSON,
/// wrapping the parsed lines in a `Value::Array`.
/// What removing one Docker object costs, and whether we can do it at
/// all. Nothing here goes to Trash: the daemon has no such thing, so
/// every removal below is permanent, and the caller must say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// `docker image rm <id>`: the layers go, and the image comes back
    /// only by pulling or rebuilding it.
    Image { id: String },
    /// `docker volume rm <name>`: a volume's contents exist nowhere else.
    Volume { name: String },
    /// Docker exposes no per-record removal for build cache — only
    /// `docker builder prune`, which is a different unit of action.
    Refused(&'static str),
}

/// Runs a removal, permanently, in the daemon. Returns the daemon's own
/// refusal text when it declines (an image still referenced by a
/// container, a volume still mounted), because that reason is the fact
/// the human needs. The daemon's own answer is the only refusal here:
/// there is no separate recheck-and-veto step in front of it.
pub fn remove(target: &Removal) -> Result<(), String> {
    crate::fs_gate::destroy::docker_remove(target)
}

/// How many buildx builders besides the daemon's own get a `du` query.
/// Each is one bounded spawn, cached with the rest of the facts.
const MAX_BUILDX_BUILDERS: usize = 4;

/// A shorter bound for the buildx queries: an unresponsive builder is a
/// stated limit, and must not hold the whole observation.
const BUILDX_TIMEOUT: Duration = Duration::from_secs(3);

fn run_docker_json(args: &[&str], timeout: Duration) -> Result<serde_json::Value, String> {
    let out = crate::fs_gate::spawn::run(crate::fs_gate::spawn::Program::Docker, args, timeout)
        .map_err(|e| format!("docker: unavailable ({e})"))?;
    if out.timed_out {
        return Err("docker: unavailable (timed out)".to_string());
    }
    if !out.success() {
        return Err("docker: unavailable (daemon not responding)".to_string());
    }
    let stdout = out.stdout_lossy();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
        return Ok(v);
    }
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if !lines.is_empty() {
        return Ok(serde_json::Value::Array(lines));
    }
    Err("docker: unavailable (bad output)".to_string())
}

/// [`run_docker_json`], for a query whose output is `buildx du
/// --verbose`'s documented text shape rather than JSON.
fn run_docker_text(args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = crate::fs_gate::spawn::run(crate::fs_gate::spawn::Program::Docker, args, timeout)
        .map_err(|e| format!("docker: unavailable ({e})"))?;
    if out.timed_out {
        return Err("docker: unavailable (timed out)".to_string());
    }
    if !out.success() {
        return Err("docker: unavailable (daemon not responding)".to_string());
    }
    Ok(out.stdout_lossy())
}

/// The daemon's API version and the buildx builders' records, after
/// `docker system df` answered. Each failure is a stated capability
/// limit on the facts, never an error: an old daemon, a missing buildx,
/// or one builder that does not answer leaves everything else intact.
fn load_buildkit_detail(facts: &mut DockerFacts) {
    match run_docker_json(&["version", "--format", "json"], DOCKER_TIMEOUT) {
        Ok(v) => facts.capabilities.api_version = api_version_of(&v),
        Err(e) => facts
            .capabilities
            .buildx_limits
            .push(format!("the daemon's API version is unknown ({e})")),
    }
    let builders = match run_docker_json(&["buildx", "ls", "--format", "json"], BUILDX_TIMEOUT) {
        Ok(serde_json::Value::Array(rows)) => rows.iter().filter_map(parse_builder).collect(),
        Ok(row) => parse_builder(&row).into_iter().collect(),
        Err(e) => {
            facts.capabilities.buildx_limits.push(format!(
                "buildx builders are not listed ({e}); only the daemon's own build cache is shown"
            ));
            Vec::new()
        }
    };
    facts.builders = builders;
    let others: Vec<String> = facts
        .builders
        .iter()
        .filter(|b| b.driver.as_deref() != Some("docker"))
        .map(|b| b.name.clone())
        .collect();
    if others.len() > MAX_BUILDX_BUILDERS {
        facts.capabilities.buildx_limits.push(format!(
            "{} builders listed; the records of only the first {MAX_BUILDX_BUILDERS} are asked for",
            others.len()
        ));
    }
    for name in others.into_iter().take(MAX_BUILDX_BUILDERS) {
        match run_docker_text(
            &["buildx", "du", "--verbose", "--builder", &name],
            BUILDX_TIMEOUT,
        ) {
            Ok(text) => {
                let records = parse_buildx_du_verbose(&text);
                merge_builder_records(facts, &name, records);
            }
            Err(e) => facts
                .capabilities
                .buildx_limits
                .push(format!("builder `{name}`: records not listed ({e})")),
        }
    }
}

/// Loads Docker facts either from a mocked facts file (tests, or the
/// CLI's `--docker-facts <file>`) or, when `facts_path` is `None`, from a
/// live, bounded `docker system df -v --format json` call. Never errors:
/// a missing binary, unreachable daemon, timeout, or bad output all
/// become `DockerFacts::unavailable`.
pub fn load(facts_path: Option<&Path>) -> DockerFacts {
    match facts_path {
        Some(path) => load_from_file(path),
        None => load_live(),
    }
}

/// How long a live Docker answer is reused before the daemon is asked
/// again. A file touch under the root says nothing about Docker; asking
/// the daemon cost ~0.9 s per observation.
pub const DOCKER_CACHE_TTL_SECS: u64 = 300;

/// How long an *unavailable* answer is reused before the daemon is asked
/// again.
///
/// `load_cached` used to write its cache only when the answer was
/// successful, so a daemon that is installed and stopped was re-probed
/// on every single pass with no TTL at all. An unavailable daemon is a
/// fact with the same shape as any other and deserves the same
/// treatment; the window is shorter because a daemon that has just been
/// started should be noticed soon (the 2026-09-22 re-review's CE6).
pub const DOCKER_UNAVAILABLE_TTL_SECS: u64 = 60;

/// `load`, with the live answer cached under the store
/// (`docker_facts.json`) for [`DOCKER_CACHE_TTL_SECS`]. `fresh` forces the
/// daemon (the scheduled `observe`, `--enrich`).
pub fn load_cached(
    facts_path: Option<&Path>,
    store_dir: Option<&Path>,
    fresh: bool,
) -> DockerFacts {
    if facts_path.is_some() {
        return load(facts_path);
    }
    let Some(dir) = store_dir else {
        return load_live();
    };
    let now = crate::entities::now();
    if !fresh && let Some((facts, cached_at)) = read_cached_facts(dir) {
        let ttl = if facts.unavailable.is_none() {
            DOCKER_CACHE_TTL_SECS
        } else {
            DOCKER_UNAVAILABLE_TTL_SECS
        };
        if now.saturating_sub(cached_at) < ttl {
            return facts;
        }
    }
    let facts = load_live();
    // The unavailable answer is cached too. Caching only success meant
    // an unreachable daemon was re-probed on every pass forever.
    let _ = write_cached_facts(dir, &facts, now);
    facts
}

// ---------------------------------------------------------------------
// The daemon-answer cache (R18b): `docker_meta.parquet` + one table per
// object kind + `docker_values.parquet` (every list/map field) +
// `docker_containers.parquet`. Replaces `docker_facts.json`; the TTL is
// measured from the meta row's `cached_at`, not a file mtime.
// ---------------------------------------------------------------------

fn docker_table(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("docker_{name}.parquet"))
}

/// Writes the daemon's answer as tables, replacing the previous cache.
pub fn write_cached_facts(dir: &Path, facts: &DockerFacts, cached_at: u64) -> anyhow::Result<()> {
    use crate::growth::columns as c;
    crate::fs_gate::store::StoreDir::at(dir)?.create()?;
    let mut values: Vec<c::StoredDockerValueRow> = Vec::new();
    let mut containers: Vec<c::StoredDockerContainerRow> = Vec::new();
    fn push_list(
        values: &mut Vec<c::StoredDockerValueRow>,
        object_kind: &str,
        object_id: &str,
        list_kind: &str,
        items: &[String],
    ) {
        for (seq, v) in items.iter().enumerate() {
            values.push(c::StoredDockerValueRow {
                object_kind: object_kind.to_string(),
                object_id: object_id.to_string(),
                list_kind: list_kind.to_string(),
                seq: seq as u32,
                key: None,
                value: v.clone(),
            });
        }
    }
    fn push_labels(
        values: &mut Vec<c::StoredDockerValueRow>,
        object_kind: &str,
        object_id: &str,
        labels: &HashMap<String, String>,
    ) {
        let mut keys: Vec<&String> = labels.keys().collect();
        keys.sort();
        for (seq, k) in keys.into_iter().enumerate() {
            values.push(c::StoredDockerValueRow {
                object_kind: object_kind.to_string(),
                object_id: object_id.to_string(),
                list_kind: "label".to_string(),
                seq: seq as u32,
                key: Some(k.clone()),
                value: labels[k].clone(),
            });
        }
    }
    fn push_containers(
        containers: &mut Vec<c::StoredDockerContainerRow>,
        object_kind: &str,
        object_id: &str,
        refs: &[ContainerRef],
    ) {
        for (seq, r) in refs.iter().enumerate() {
            containers.push(c::StoredDockerContainerRow {
                object_kind: object_kind.to_string(),
                object_id: object_id.to_string(),
                seq: seq as u32,
                name: r.name.clone(),
                state: r.state.clone(),
                finished_at: r.finished_at.clone(),
            });
        }
    }
    let images: Vec<c::StoredDockerImageRow> = facts
        .images
        .iter()
        .map(|i| {
            push_list(&mut values, "image", &i.id, "repo_tag", &i.repo_tags);
            push_list(&mut values, "image", &i.id, "layer", &i.layers);
            push_list(&mut values, "image", &i.id, "shared_with", &i.shared_with);
            push_labels(&mut values, "image", &i.id, &i.labels);
            push_containers(&mut containers, "image", &i.id, &i.containers);
            c::StoredDockerImageRow {
                id: i.id.clone(),
                shared_bytes: i.shared_bytes,
                unique_bytes: i.unique_bytes,
                created_at: i.created_at.clone(),
                dangling: i.dangling,
            }
        })
        .collect();
    let caches: Vec<c::StoredDockerCacheRow> = facts
        .build_cache
        .iter()
        .map(|b| {
            push_list(&mut values, "cache", &b.id, "parent", &b.parents);
            c::StoredDockerCacheRow {
                id: b.id.clone(),
                bytes: b.bytes,
                last_used: b.last_used.clone(),
                usage_count: b.usage_count,
                in_use: b.in_use,
                shared: b.shared,
                cache_type: b.cache_type.clone(),
                description: b.description.clone(),
                created_at: b.created_at.clone(),
                reclaimable: b.reclaimable,
                mutable: b.mutable,
                builder: b.builder.clone(),
            }
        })
        .collect();
    let volumes: Vec<c::StoredDockerVolumeRow> = facts
        .volumes
        .iter()
        .map(|v| {
            push_labels(&mut values, "volume", &v.name, &v.labels);
            push_containers(&mut containers, "volume", &v.name, &v.containers);
            c::StoredDockerVolumeRow {
                name: v.name.clone(),
                bytes: v.bytes,
                created_at: v.created_at.clone(),
                driver: v.driver.clone(),
            }
        })
        .collect();
    push_list(
        &mut values,
        "meta",
        "",
        "buildx_limit",
        &facts.capabilities.buildx_limits,
    );
    let builders: Vec<c::StoredDockerBuilderRow> = facts
        .builders
        .iter()
        .map(|b| c::StoredDockerBuilderRow {
            name: b.name.clone(),
            driver: b.driver.clone(),
            status: b.status.clone(),
        })
        .collect();
    c::write_docker_image_rows(&docker_table(dir, "images"), &images)?;
    c::write_docker_cache_rows(&docker_table(dir, "build_cache"), &caches)?;
    c::write_docker_volume_rows(&docker_table(dir, "volumes"), &volumes)?;
    c::write_docker_builder_rows(&docker_table(dir, "builders"), &builders)?;
    c::write_docker_value_rows(&docker_table(dir, "values"), &values)?;
    c::write_docker_container_rows(&docker_table(dir, "containers"), &containers)?;
    // The meta row last: a reader that finds it can trust the rest.
    c::write_docker_meta_rows(
        &docker_table(dir, "meta"),
        &[c::StoredDockerMetaRow {
            cached_at,
            captured_at: facts.captured_at,
            unavailable: facts.unavailable.clone(),
            api_version: facts.capabilities.api_version.clone(),
        }],
    )?;
    Ok(())
}

/// The cached daemon answer and when it was cached, if there is one.
pub fn read_cached_facts(dir: &Path) -> Option<(DockerFacts, u64)> {
    use crate::growth::columns as c;
    let meta = c::read_docker_meta_rows(&docker_table(dir, "meta"))
        .ok()?
        .into_iter()
        .next()?;
    let values = c::read_docker_value_rows(&docker_table(dir, "values")).ok()?;
    let containers = c::read_docker_container_rows(&docker_table(dir, "containers")).ok()?;
    let list = |object_kind: &str, object_id: &str, list_kind: &str| -> Vec<String> {
        let mut v: Vec<&c::StoredDockerValueRow> = values
            .iter()
            .filter(|r| {
                r.object_kind == object_kind && r.object_id == object_id && r.list_kind == list_kind
            })
            .collect();
        v.sort_by_key(|r| r.seq);
        v.into_iter().map(|r| r.value.clone()).collect()
    };
    let labels = |object_kind: &str, object_id: &str| -> HashMap<String, String> {
        values
            .iter()
            .filter(|r| {
                r.object_kind == object_kind && r.object_id == object_id && r.list_kind == "label"
            })
            .filter_map(|r| r.key.clone().map(|k| (k, r.value.clone())))
            .collect()
    };
    let refs = |object_kind: &str, object_id: &str| -> Vec<ContainerRef> {
        let mut v: Vec<&c::StoredDockerContainerRow> = containers
            .iter()
            .filter(|r| r.object_kind == object_kind && r.object_id == object_id)
            .collect();
        v.sort_by_key(|r| r.seq);
        v.into_iter()
            .map(|r| ContainerRef {
                name: r.name.clone(),
                state: r.state.clone(),
                finished_at: r.finished_at.clone(),
            })
            .collect()
    };
    let images = c::read_docker_image_rows(&docker_table(dir, "images"))
        .ok()?
        .into_iter()
        .map(|i| DockerImageFact {
            repo_tags: list("image", &i.id, "repo_tag"),
            labels: labels("image", &i.id),
            layers: list("image", &i.id, "layer"),
            shared_with: list("image", &i.id, "shared_with"),
            containers: refs("image", &i.id),
            id: i.id,
            shared_bytes: i.shared_bytes,
            unique_bytes: i.unique_bytes,
            created_at: i.created_at,
            dangling: i.dangling,
        })
        .collect();
    let build_cache = c::read_docker_cache_rows(&docker_table(dir, "build_cache"))
        .ok()?
        .into_iter()
        .map(|b| DockerCacheFact {
            parents: list("cache", &b.id, "parent"),
            id: b.id,
            bytes: b.bytes,
            last_used: b.last_used,
            usage_count: b.usage_count,
            in_use: b.in_use,
            shared: b.shared,
            cache_type: b.cache_type,
            description: b.description,
            created_at: b.created_at,
            reclaimable: b.reclaimable,
            mutable: b.mutable,
            builder: b.builder,
        })
        .collect();
    let volumes = c::read_docker_volume_rows(&docker_table(dir, "volumes"))
        .ok()?
        .into_iter()
        .map(|v| DockerVolumeFact {
            labels: labels("volume", &v.name),
            containers: refs("volume", &v.name),
            name: v.name,
            bytes: v.bytes,
            created_at: v.created_at,
            driver: v.driver,
        })
        .collect();
    let builders = c::read_docker_builder_rows(&docker_table(dir, "builders"))
        .ok()?
        .into_iter()
        .map(|b| DockerBuilderFact {
            name: b.name,
            driver: b.driver,
            status: b.status,
        })
        .collect();
    Some((
        DockerFacts {
            images,
            build_cache,
            volumes,
            unavailable: meta.unavailable,
            builders,
            capabilities: DockerCapabilities {
                api_version: meta.api_version,
                buildx_limits: list("meta", "", "buildx_limit"),
            },
            captured_at: meta.captured_at,
        },
        meta.cached_at,
    ))
}

fn load_from_file(path: &Path) -> DockerFacts {
    // A facts file the caller named explicitly (`--docker-facts`, tests).
    let text = match crate::fs_gate::read::read_owned_string(path) {
        Ok(t) => t,
        Err(e) => {
            return DockerFacts {
                unavailable: Some(format!("docker: unavailable ({e})")),
                ..Default::default()
            };
        }
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => parse_value(&v),
        Err(e) => DockerFacts {
            unavailable: Some(format!("docker: unavailable (invalid facts file: {e})")),
            ..Default::default()
        },
    }
}

/// `docker system df -v` never carries image labels or layer digests
/// (verified against a live daemon), so a live load layers three more
/// best-effort calls on top of the df call for object identity/sizes:
///
/// 1. a batched `docker image inspect <ids> --format json` for labels
///    and `RootFS.Layers` (used for `shared_with`);
/// 2. a batched `docker volume inspect <names> --format json` for
///    volume `CreatedAt` (df's `Driver` is already enough on its own,
///    but `CreatedAt` is inspect-only);
/// 3. `docker ps -a --format json` for the container list, then a
///    batched `docker inspect <container ids> --format json` for the
///    precise `FinishedAt` and volume mount names `ps` doesn't carry.
///
/// Each of these is best-effort: a failure or timeout leaves that one
/// piece of detail unset rather than failing the whole report -- df's
/// object identity/sizes already succeeded, so the report still
/// reconciles.
fn load_live() -> DockerFacts {
    let df_value =
        match run_docker_json(&["system", "df", "-v", "--format", "json"], DOCKER_TIMEOUT) {
            Ok(v) => v,
            Err(reason) => {
                return DockerFacts {
                    unavailable: Some(reason),
                    ..Default::default()
                };
            }
        };
    let mut facts = parse_value(&df_value);
    facts.captured_at = Some(crate::entities::now());
    load_buildkit_detail(&mut facts);

    let ids: Vec<&str> = facts
        .images
        .iter()
        .map(|i| i.id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    if !ids.is_empty() {
        let mut args: Vec<&str> = vec!["image", "inspect"];
        args.extend(ids);
        args.extend(["--format", "json"]);
        if let Ok(inspect_value) = run_docker_json(&args, DOCKER_TIMEOUT)
            && let Some(entries) = inspect_value.as_array()
        {
            merge_image_inspect(&mut facts.images, entries);
        }
    }

    let volume_names: Vec<&str> = facts.volumes.iter().map(|v| v.name.as_str()).collect();
    if !volume_names.is_empty() {
        let mut args: Vec<&str> = vec!["volume", "inspect"];
        args.extend(volume_names);
        args.extend(["--format", "json"]);
        if let Ok(inspect_value) = run_docker_json(&args, DOCKER_TIMEOUT)
            && let Some(entries) = inspect_value.as_array()
        {
            merge_volume_inspect(&mut facts.volumes, entries);
        }
    }

    if let Ok(ps_value) = run_docker_json(&["ps", "-a", "--format", "json"], DOCKER_TIMEOUT)
        && let Some(rows) = ps_value.as_array()
    {
        let container_ids: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("ID").and_then(|v| v.as_str()).map(String::from))
            .collect();
        if !container_ids.is_empty() {
            let id_refs: Vec<&str> = container_ids.iter().map(String::as_str).collect();
            let mut args: Vec<&str> = vec!["inspect"];
            args.extend(id_refs);
            args.extend(["--format", "json"]);
            if let Ok(inspect_value) = run_docker_json(&args, DOCKER_TIMEOUT)
                && let Some(entries) = inspect_value.as_array()
            {
                let containers: Vec<ContainerFact> =
                    entries.iter().map(parse_container_inspect).collect();
                join_containers(&mut facts, &containers);
            }
        }
    }

    compute_shared_with(&mut facts.images);

    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_and_raw_sizes() {
        assert_eq!(parse_size("153MB"), 153_000_000);
        assert_eq!(parse_size("0B"), 0);
        assert_eq!(parse_size("83.9kB"), 83_900);
        assert_eq!(parse_size("8388608"), 8_388_608);
    }

    #[test]
    fn missing_facts_file_is_unavailable_not_an_error() {
        let facts = load_from_file(Path::new("/nonexistent/does-not-exist.json"));
        assert!(facts.unavailable.is_some());
        assert!(facts.images.is_empty());
    }

    #[test]
    fn dangling_image_has_no_repo_tags() {
        let value = serde_json::json!({
            "Images": [
                {"ID": "sha256:abc", "Repository": "<none>", "Tag": "<none>", "Size": "10"},
                {"ID": "sha256:def", "Repository": "named", "Tag": "latest", "Size": "10"},
            ]
        });
        let facts = parse_value(&value);
        assert!(facts.images[0].dangling);
        assert!(!facts.images[1].dangling);
    }

    #[test]
    fn shared_with_lists_other_images_sharing_a_layer() {
        let value = serde_json::json!({
            "Images": [
                {"ID": "sha256:a", "Repository": "img-a", "Tag": "latest", "Size": "10"},
                {"ID": "sha256:b", "Repository": "img-b", "Tag": "latest", "Size": "10"},
                {"ID": "sha256:c", "Repository": "img-c", "Tag": "latest", "Size": "10"},
            ],
            "ImageInspect": [
                {"Id": "sha256:a", "RepoTags": ["img-a:latest"], "RootFS": {"Layers": ["L1", "L2"]}},
                {"Id": "sha256:b", "RepoTags": ["img-b:latest"], "RootFS": {"Layers": ["L1", "L3"]}},
                {"Id": "sha256:c", "RepoTags": ["img-c:latest"], "RootFS": {"Layers": ["L9"]}},
            ]
        });
        let facts = parse_value(&value);
        let a = facts.images.iter().find(|i| i.id == "sha256:a").unwrap();
        assert_eq!(a.shared_with, vec!["img-b:latest".to_string()]);
        let c = facts.images.iter().find(|i| i.id == "sha256:c").unwrap();
        assert!(c.shared_with.is_empty());
    }

    #[test]
    fn container_join_attaches_state_and_finished_at_to_its_image() {
        let value = serde_json::json!({
            "Images": [
                {"ID": "sha256:aaa", "Repository": "hiphi-relay", "Tag": "staging", "Size": "10"},
            ],
            "Containers": [
                {
                    "Id": "c1",
                    "Name": "/hiphi-staging-relay-1",
                    "Config": {"Image": "hiphi-relay:staging"},
                    "State": {"Status": "exited", "FinishedAt": "2026-09-10T12:00:00Z"},
                    "Mounts": [],
                }
            ]
        });
        let facts = parse_value(&value);
        let image = &facts.images[0];
        assert_eq!(image.containers.len(), 1);
        assert_eq!(image.containers[0].name, "hiphi-staging-relay-1");
        assert_eq!(image.containers[0].state, "exited");
        assert_eq!(
            image.containers[0].finished_at.as_deref(),
            Some("2026-09-10T12:00:00Z")
        );
    }

    #[test]
    fn container_never_finished_normalizes_to_none() {
        let value = serde_json::json!({
            "Images": [{"ID": "sha256:aaa", "Repository": "running-img", "Tag": "latest", "Size": "10"}],
            "Containers": [
                {
                    "Id": "c1",
                    "Name": "/still-running",
                    "Config": {"Image": "running-img:latest"},
                    "State": {"Status": "running", "FinishedAt": "0001-01-01T00:00:00Z"},
                    "Mounts": [],
                }
            ]
        });
        let facts = parse_value(&value);
        assert_eq!(facts.images[0].containers[0].finished_at, None);
    }

    /// A facts file in the documented shapes: `docker system df -v
    /// --format json`'s `BuildCache`, `docker version --format json`,
    /// `docker buildx ls --format json`, and `docker buildx du --verbose`
    /// text per builder. Synthetic ids and descriptions.
    const BUILDKIT_FACTS: &str = r#"{
      "BuildCache": [
        {"ID": "r1", "CacheType": "regular", "Description": "[build 2/5] RUN apt-get update",
         "Size": "120MB", "CreatedAt": "2024-01-15T10:32:00Z", "LastUsedAt": "2024-02-01T08:00:00Z",
         "UsageCount": 4, "InUse": false, "Shared": true},
        {"ID": "r2", "CacheType": "exec.cachemount", "Description": "mount / from exec /bin/sh -c pip install",
         "Size": "300MB", "Parent": "r1", "InUse": true, "Shared": false}
      ],
      "Version": {"Client": {"ApiVersion": "1.45"}, "Server": {"ApiVersion": "1.45", "Version": "26.1.0"}},
      "Builders": [
        {"Name": "default", "Driver": "docker", "Nodes": [{"Status": "running"}]},
        {"Name": "ci", "Driver": "docker-container", "Nodes": [{"Status": "running"}]},
        {"Name": "cold", "Driver": "docker-container", "Nodes": [{"Status": "inactive"}]}
      ],
      "BuildxDu": [
        {"Builder": "ci", "Verbose": "ID:\t\tc1\nParents:\tc0\nCreated at:\t2024-03-01 09:00:00 +0000 UTC\nMutable:\tfalse\nReclaimable:\ttrue\nShared:\t\tfalse\nSize:\t\t50MB\nDescription:\t[stage-1 1/3] COPY . .\nUsage count:\t1\nLast used:\t2024-03-01 09:05:00 +0000 UTC\nType:\t\tregular\n\nID:\t\tc0\nCreated at:\t2024-03-01 08:59:00 +0000 UTC\nReclaimable:\ttrue\nShared:\t\ttrue\nSize:\t\t10MB\nType:\t\tsource.local\n\nShared:\t\t10MB\nPrivate:\t50MB\nReclaimable:\t60MB\nTotal:\t\t60MB\n"},
        {"Builder": "cold", "Unavailable": "timed out"}
      ]
    }"#;

    #[test]
    fn buildkit_records_keep_every_daemon_field_per_builder() {
        let v: serde_json::Value = serde_json::from_str(BUILDKIT_FACTS).unwrap();
        let f = parse_value(&v);
        assert_eq!(f.capabilities.api_version.as_deref(), Some("1.45"));
        assert!(!f.capabilities.build_cache_api_unsupported());
        assert_eq!(f.builder_names(), vec!["default", "ci", "cold"]);
        let r2 = f.build_cache.iter().find(|r| r.id == "r2").unwrap();
        assert_eq!(
            r2.parents,
            vec!["r1".to_string()],
            "the deprecated `Parent` is read"
        );
        assert!(r2.in_use);
        assert_eq!(r2.cache_type.as_deref(), Some("exec.cachemount"));
        let ci = f.records_of("ci");
        assert_eq!(
            ci.len(),
            2,
            "the verbose text's summary block is not a record"
        );
        let c1 = ci.iter().find(|r| r.id == "c1").unwrap();
        assert_eq!(c1.bytes, 50_000_000);
        assert_eq!(c1.reclaimable, Some(true));
        assert_eq!(c1.parents, vec!["c0".to_string()]);
        assert_eq!(
            c1.last_used.as_deref(),
            Some("2024-03-01 09:05:00 +0000 UTC")
        );
        assert!(f.records_of("cold").is_empty());
        assert!(
            f.capabilities
                .buildx_limits
                .iter()
                .any(|l| l.contains("`cold`") && l.contains("timed out")),
            "a builder that did not answer is a stated limit: {:?}",
            f.capabilities.buildx_limits
        );
        assert_eq!(f.records_of("default").len(), 2);
    }

    #[test]
    fn an_old_daemon_api_is_stated_and_an_unknown_one_is_not_assumed_old() {
        let old = DockerCapabilities {
            api_version: Some("1.38".into()),
            ..Default::default()
        };
        assert!(old.build_cache_api_unsupported());
        assert!(!DockerCapabilities::default().build_cache_api_unsupported());
    }

    #[test]
    fn only_allow_listed_observation_queries_may_run() {
        // The allow-list itself lives in `fs_gate::spawn::shapes` now (its
        // own tests assert the shapes this module needs are accepted); here
        // it is enough to prove a mutating command is refused before any
        // process is spawned, through both entry points this module uses.
        let (r, counted) = crate::work_counters::measured(|| {
            run_docker_json(&["builder", "prune", "-f"], DOCKER_TIMEOUT)
        });
        assert!(r.is_err());
        assert_eq!(
            counted.subprocess_spawns, 0,
            "refused before any process is spawned"
        );
        let (r, counted) = crate::work_counters::measured(|| {
            run_docker_text(&["buildx", "prune", "--filter", "id=x"], DOCKER_TIMEOUT)
        });
        assert!(r.is_err());
        assert_eq!(
            counted.subprocess_spawns, 0,
            "refused before any process is spawned"
        );
    }
}
