//! Optional network enrichment of Hugging Face hub repos: one read-only
//! GET of `https://huggingface.co/api/{models,datasets,spaces}/<id>` per
//! repo, OFF by default (`hf_enrich = true` in the store's
//! `config.toml` turns it on), in a scheduled `observe` only.
//!
//! Everything goes through the pass-through card cache:
//!
//! * facts pinned to a revision (pipeline tag, library, license, base
//!   model, as the Hub states them for the revision this machine holds)
//!   are stored under `<repo>@<sha>` and never fetched again;
//! * facts about the repo that move (downloads, likes, last modified,
//!   which revision is current) are stored under `<repo>` and fetched
//!   again after [`MUTABLE_TTL_SECS`];
//! * a failure (404, gated, offline, timeout) is stored too and retried
//!   only after [`NEGATIVE_TTL_SECS`].
//!
//! At most [`MAX_FETCHES_PER_PASS`] fetches run per pass. `/usr/bin/curl`
//! only, through `fs_gate::spawn` (allow-listed shape, counted, killed
//! on timeout), with a scrubbed environment and no header: no token is
//! sent. `report` and the TUI only read what this stored.

use crate::artifact::NestedArtifact;
use crate::build_adapters::NestedUnitBuilder;
use crate::build_adapters::model_cards::{self, CardCache, CardEntry, CardFields};
use crate::build_adapters::model_stores::{CARD_EVIDENCE, FIELD_EVIDENCE, HUB_API_EVIDENCE};
use crate::entities::Confidence;
use std::time::Duration;

/// Repo-level facts that move are fetched again after a week.
pub const MUTABLE_TTL_SECS: u64 = 7 * 86_400;
/// A failed fetch is retried after a day, not every pass.
pub const NEGATIVE_TTL_SECS: u64 = 86_400;
/// Fetches one pass may make; the rest wait for the next pass.
pub const MAX_FETCHES_PER_PASS: usize = 16;
/// The one line a report shows when enrichment is off.
pub const OFF_LINE: &str = "Hugging Face Hub facts are not fetched (off by default); `swamp config set hf-enrich on` (`hf_enrich = true` in config.toml) turns on one read-only request per repo during scheduled observes";

const API_FORMAT: &str = "api-1";
const TIMEOUT: Duration = Duration::from_secs(12);

/// The pinned fields the Hub states for a revision, as card fields.
const PINNED: &[&str] = &["pipeline_tag", "library_name", "license", "base_model"];

fn repo_key(store: &str, kind: &str, id: &str) -> String {
    format!("hfapi|{store}|{kind}/{id}")
}

fn rev_key(store: &str, kind: &str, id: &str, sha: &str) -> String {
    format!("hfapi|{store}|{kind}/{id}@{sha}")
}

/// What one GET answered: the repo-level fields, and the pinned ones
/// with the revision they describe. Errors are words, stored as-is.
pub(crate) fn parse_answer(
    body: &str,
) -> Result<(CardFields, Option<(String, CardFields)>), String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "the answer is not JSON".to_string())?;
    let mut repo = CardFields::new();
    repo.insert("status".into(), "ok".into());
    let s = |x: &serde_json::Value| x.as_str().map(|t| model_cards::sanitize(t, 120));
    if let Some(n) = v.get("downloads").and_then(|x| x.as_u64()) {
        repo.insert("downloads".into(), n.to_string());
    }
    if let Some(n) = v.get("likes").and_then(|x| x.as_u64()) {
        repo.insert("likes".into(), n.to_string());
    }
    if let Some(t) = v.get("lastModified").and_then(s) {
        repo.insert("last_modified".into(), t.chars().take(10).collect());
    }
    let sha = v
        .get("sha")
        .and_then(|x| x.as_str())
        .filter(|t| t.len() == 40 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string);
    if let Some(sha) = &sha {
        repo.insert("sha".into(), sha.clone());
    }
    let mut pinned = CardFields::new();
    for k in ["pipeline_tag", "library_name"] {
        if let Some(t) = v.get(k).and_then(s).filter(|t| !t.is_empty()) {
            pinned.insert(k.into(), t);
        }
    }
    if let Some(card) = v.get("cardData") {
        if let Some(t) = card.get("license").and_then(s).filter(|t| !t.is_empty()) {
            pinned.insert("license".into(), t);
        }
        let base = match card.get("base_model") {
            Some(serde_json::Value::String(b)) => Some(model_cards::sanitize(b, 120)),
            Some(serde_json::Value::Array(a)) => a.first().and_then(s),
            _ => None,
        };
        if let Some(b) = base.filter(|b| !b.is_empty()) {
            pinned.insert("base_model".into(), b);
        }
    }
    Ok((repo, sha.map(|sha| (sha, pinned))))
}

/// One GET. The body, or why there is none.
fn fetch(kind: &str, id: &str) -> Result<String, String> {
    let url = format!("https://huggingface.co/api/{kind}s/{id}");
    if !crate::fs_gate::spawn::hub_api_url(&url) {
        return Err("the repo id is not one swamp asks the Hub about".into());
    }
    let out = crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Curl,
        [
            "-q",
            "-sS",
            "--proto",
            "=https",
            "--max-time",
            "10",
            "--max-filesize",
            "1048576",
            "-w",
            "\n%{http_code}",
            url.as_str(),
        ],
        TIMEOUT,
    )
    .map_err(|e| format!("curl could not run: {e}"))?;
    if out.timed_out {
        return Err("timed out".into());
    }
    let text = out.stdout_lossy();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
    match code.trim() {
        "200" => Ok(body.to_string()),
        "" | "000" => Err("no answer (offline or blocked)".into()),
        c => Err(format!("HTTP {}", model_cards::sanitize(c, 8))),
    }
}

fn fresh(e: &CardEntry, now: u64) -> bool {
    let ttl = if e.fields.get("status").map(String::as_str) == Some("ok") {
        MUTABLE_TTL_SECS
    } else {
        NEGATIVE_TTL_SECS
    };
    now.saturating_sub(e.at) < ttl
}

/// Adds the Hub's facts to every hub repo unit, from the cache when it
/// can, fetching only on a miss, only when `enabled`, and only when
/// `may_fetch` (a CLI or scheduled observe; the TUI's refresh passes
/// `false` and gets cached answers only). Never called by `report`.
pub fn enrich(
    units: &mut [NestedArtifact],
    cards: &CardCache,
    now: u64,
    enabled: bool,
    may_fetch: bool,
) {
    let budget = if may_fetch { MAX_FETCHES_PER_PASS } else { 0 };
    enrich_with(units, cards, now, enabled, budget, &fetch)
}

pub(crate) fn enrich_with(
    units: &mut [NestedArtifact],
    cards: &CardCache,
    now: u64,
    enabled: bool,
    mut budget: usize,
    get: &dyn Fn(&str, &str) -> Result<String, String>,
) {
    for u in units.iter_mut() {
        if u.adapter.as_deref() != Some("model-stores") {
            continue;
        }
        let kind = match u.variant.configuration.as_deref() {
            Some(k @ ("model" | "dataset" | "space")) => k.to_string(),
            _ => continue,
        };
        let Some(id) = u.variant.package.clone() else {
            continue;
        };
        if !enabled {
            *u = NestedUnitBuilder::amend(u.clone())
                .evidence(HUB_API_EVIDENCE, "off", Confidence::High)
                .build();
            continue;
        }
        let store = u
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let key = repo_key(&store, &kind, &id);
        let cached = cards
            .peek(&key)
            .filter(|e| e.fingerprint == API_FORMAT && fresh(e, now));
        let entry = match cached {
            Some(e) => {
                let _ = cards.lookup(&key, API_FORMAT);
                Some(e)
            }
            None if budget > 0 => {
                budget -= 1;
                let mut fields = CardFields::new();
                match get(&kind, &id).and_then(|b| parse_answer(&b)) {
                    Ok((repo, pinned)) => {
                        fields = repo;
                        if let Some((sha, p)) = pinned {
                            cards.insert(
                                &rev_key(&store, &kind, &id, &sha),
                                CardEntry {
                                    fingerprint: API_FORMAT.into(),
                                    at: now,
                                    fields: p,
                                },
                            );
                        }
                    }
                    Err(why) => {
                        fields.insert("status".into(), why);
                    }
                }
                let e = CardEntry {
                    fingerprint: API_FORMAT.into(),
                    at: now,
                    fields,
                };
                cards.insert(&key, e.clone());
                Some(e)
            }
            // No fetch this pass: an older answer is kept (and stays in the
            // cache), labelled as older; never dropped.
            None => cards.lookup(&key, API_FORMAT).map(|mut e| {
                e.fields.insert("past_ttl".into(), "1".into());
                e
            }),
        };
        let mut b = NestedUnitBuilder::amend(u.clone());
        let Some(entry) = entry else {
            b = b.evidence(
                HUB_API_EVIDENCE,
                format!(
                    "not yet fetched from huggingface.co (a pass asks at most {MAX_FETCHES_PER_PASS} times, and only a scheduled or CLI observe asks; the next one does)"
                ),
                Confidence::High,
            );
            *u = b.build();
            continue;
        };
        let day = crate::last_used::format_day(entry.at, now);
        let status = entry.fields.get("status").cloned().unwrap_or_default();
        if status != "ok" {
            b = b.evidence(
                HUB_API_EVIDENCE,
                format!("huggingface.co did not answer for {id}: {status} (asked {day}; asked again after a day)"),
                Confidence::High,
            );
            *u = b.build();
            continue;
        }
        let local = u.variant.version.clone().unwrap_or_default();
        let remote = entry.fields.get("sha").cloned().unwrap_or_default();
        let current = !local.is_empty() && remote.starts_with(&local);
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = entry.fields.get("downloads") {
            parts.push(format!("{n} downloads"));
        }
        if let Some(n) = entry.fields.get("likes") {
            parts.push(format!("{n} likes"));
        }
        if let Some(t) = entry.fields.get("last_modified") {
            parts.push(format!("last modified {t}"));
        }
        if !remote.is_empty() {
            parts.push(if current {
                "the revision here is the Hub's current one".to_string()
            } else {
                format!(
                    "the Hub's current revision is {}, not the one here",
                    remote.chars().take(8).collect::<String>()
                )
            });
        }
        if let Some(sha) = entry.fields.get("sha") {
            // Keep the revision-pinned entry wanted whether or not it is used.
            let _ = cards.lookup(&rev_key(&store, &kind, &id, sha), API_FORMAT);
        }
        let older = if entry.fields.contains_key("past_ttl") {
            "; older than the refresh interval, a scheduled observe asks again"
        } else {
            ""
        };
        b = b.evidence(
            HUB_API_EVIDENCE,
            format!(
                "from huggingface.co, fetched {day}: {}{older}",
                parts.join(", ")
            ),
            Confidence::High,
        );
        // Pinned facts fill only what the local files did not state, and
        // only for the revision this machine holds.
        if current && let Some(p) = cards.lookup(&rev_key(&store, &kind, &id, &remote), API_FORMAT)
        {
            let mut fields: CardFields = u
                .producer_evidence
                .iter()
                .filter(|e| e.source == FIELD_EVIDENCE)
                .filter_map(|e| e.detail.split_once(": "))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let mut added = false;
            for k in PINNED {
                if !fields.contains_key(*k)
                    && let Some(v) = p.fields.get(*k)
                {
                    fields.insert((*k).to_string(), v.clone());
                    b = b.evidence(
                        FIELD_EVIDENCE,
                        format!("{k}: {v} (huggingface.co)"),
                        Confidence::Medium,
                    );
                    added = true;
                }
            }
            if added && let Some(line) = model_cards::summary_line(&fields) {
                b = b.replace_evidence(CARD_EVIDENCE, line);
            }
        }
        *u = b.build();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn unit() -> NestedArtifact {
        let c = crate::build_adapters::BuildContainer::shared_store_of(
            "model-stores",
            "/hub".into(),
            crate::locations::BuildStoreKind::HuggingFaceHub,
        );
        NestedUnitBuilder::new(
            &c,
            crate::artifact::ArtifactRole::SharedStoreEntry,
            "/hub/models--org--m".into(),
        )
        .variant(crate::artifact::ArtifactVariant {
            package: Some("org/m".into()),
            version: Some("aaaaaaaa".into()),
            configuration: Some("model".into()),
            ..Default::default()
        })
        .build()
    }

    const BODY: &str = r#"{"id":"org/m","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","downloads":12,"likes":3,"lastModified":"2025-03-01T00:00:00.000Z","pipeline_tag":"text-generation","cardData":{"license":"mit","base_model":["base/x"]}}"#;

    fn api(u: &NestedArtifact) -> String {
        u.producer_evidence
            .iter()
            .filter(|e| e.source == HUB_API_EVIDENCE)
            .map(|e| e.detail.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Tempting wrong patch: fetching whenever the program is present.
    /// Off is the default, and off fetches nothing.
    #[test]
    fn off_fetches_nothing_and_says_so() {
        let calls = Cell::new(0);
        let cards = CardCache::default();
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000,
            false,
            MAX_FETCHES_PER_PASS,
            &|_, _| {
                calls.set(calls.get() + 1);
                Ok(BODY.into())
            },
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(api(&units[0]), "off");
    }

    /// Tempting wrong patch: a cache keyed on time alone, fetched again
    /// every pass. The second pass inside the TTL fetches nothing; the
    /// pinned facts never expire; the mutable ones do after a week.
    #[test]
    fn a_cache_hit_fetches_nothing_and_the_ttl_applies_only_to_mutable_facts() {
        let calls = Cell::new(0);
        let get = |_: &str, _: &str| {
            calls.set(calls.get() + 1);
            Ok(BODY.to_string())
        };
        let cards = CardCache::default();
        let mut units = vec![unit()];
        enrich_with(&mut units, &cards, 1_000, true, MAX_FETCHES_PER_PASS, &get);
        assert_eq!(calls.get(), 1);
        let text = api(&units[0]);
        assert!(text.starts_with("from huggingface.co, fetched "), "{text}");
        assert!(
            text.contains("12 downloads") && text.contains("current one"),
            "{text}"
        );
        let card = units[0]
            .producer_evidence
            .iter()
            .find(|e| e.source == CARD_EVIDENCE)
            .unwrap();
        assert!(card.detail.contains("text-generation") && card.detail.contains("base base/x"));
        let stored = cards.retained(&|_| false);
        let cards = CardCache::from_entries(stored, 0);
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000 + MUTABLE_TTL_SECS - 1,
            true,
            MAX_FETCHES_PER_PASS,
            &get,
        );
        assert_eq!(calls.get(), 1, "inside the TTL: no fetch");
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000 + MUTABLE_TTL_SECS,
            true,
            MAX_FETCHES_PER_PASS,
            &get,
        );
        assert_eq!(calls.get(), 2, "past the TTL: one fetch");
        assert!(
            cards
                .peek("hfapi|/hub|model/org/m@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .is_some()
        );
    }

    /// Tempting wrong patch: not storing failures, so an offline machine
    /// retries every repo every pass.
    #[test]
    fn a_failure_is_cached_and_retried_after_a_day() {
        let calls = Cell::new(0);
        let get = |_: &str, _: &str| {
            calls.set(calls.get() + 1);
            Err::<String, String>("HTTP 404".into())
        };
        let cards = CardCache::default();
        let mut units = vec![unit()];
        enrich_with(&mut units, &cards, 1_000, true, MAX_FETCHES_PER_PASS, &get);
        assert!(api(&units[0]).contains("HTTP 404"));
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000 + NEGATIVE_TTL_SECS - 1,
            true,
            MAX_FETCHES_PER_PASS,
            &get,
        );
        assert_eq!(calls.get(), 1);
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000 + NEGATIVE_TTL_SECS,
            true,
            MAX_FETCHES_PER_PASS,
            &get,
        );
        assert_eq!(calls.get(), 2);
    }

    /// Tempting wrong patch: no per-pass cap, so a cache of 500 repos
    /// makes 500 requests in one observe.
    #[test]
    fn fetches_per_pass_are_capped_and_the_rest_say_not_yet() {
        let calls = Cell::new(0);
        let get = |_: &str, _: &str| {
            calls.set(calls.get() + 1);
            Ok(BODY.to_string())
        };
        let cards = CardCache::default();
        let mut units: Vec<NestedArtifact> = (0..MAX_FETCHES_PER_PASS + 3)
            .map(|i| {
                let mut u = unit();
                u.path = format!("/hub/models--org--m{i}").into();
                u.variant.package = Some(format!("org/m{i}"));
                u
            })
            .collect();
        enrich_with(&mut units, &cards, 1_000, true, MAX_FETCHES_PER_PASS, &get);
        assert_eq!(calls.get(), MAX_FETCHES_PER_PASS);
        assert!(api(&units[MAX_FETCHES_PER_PASS]).starts_with("not yet fetched"));
    }

    /// Tempting wrong patch: letting the TUI's refresh (an observe too)
    /// fetch. With no fetch budget it answers from the cache only.
    #[test]
    fn a_pass_that_may_not_fetch_answers_from_the_cache_only() {
        let calls = Cell::new(0);
        let get = |_: &str, _: &str| {
            calls.set(calls.get() + 1);
            Ok(BODY.to_string())
        };
        let cards = CardCache::default();
        let mut units = vec![unit()];
        enrich_with(&mut units, &cards, 1_000, true, 0, &get);
        assert_eq!(calls.get(), 0);
        assert!(api(&units[0]).starts_with("not yet fetched"));
        enrich_with(&mut units, &cards, 1_000, true, 1, &get);
        let mut units = vec![unit()];
        enrich_with(&mut units, &cards, 2_000, true, 0, &get);
        assert_eq!(calls.get(), 1);
        assert!(api(&units[0]).starts_with("from huggingface.co"));
        // Past the TTL with no fetch allowed (the TUI refresh): the older
        // answer is shown, labelled, and kept in the cache.
        let mut units = vec![unit()];
        enrich_with(
            &mut units,
            &cards,
            1_000 + MUTABLE_TTL_SECS + 5,
            true,
            0,
            &get,
        );
        assert!(
            api(&units[0]).contains("older than the refresh interval"),
            "{}",
            api(&units[0])
        );
        let kept = cards.retained(&|_| false);
        assert!(kept.contains_key("hfapi|/hub|model/org/m"));
        assert!(
            kept.keys().any(|k| k.contains('@')),
            "the pinned entry was pruned"
        );
        assert_eq!(calls.get(), 1);
    }

    /// Tempting wrong patch: building the URL from the repo id without
    /// checking it, so `../../` or an option reaches curl.
    #[test]
    fn only_hub_api_urls_with_plain_ids_pass_the_gate() {
        use crate::fs_gate::spawn::hub_api_url;
        assert!(hub_api_url("https://huggingface.co/api/models/org/m-1.5_x"));
        assert!(hub_api_url("https://huggingface.co/api/datasets/squad"));
        for bad in [
            "https://huggingface.co/api/models/../x",
            "https://huggingface.co/api/models/org/m/extra",
            "https://huggingface.co/api/models/-o",
            "https://evil.example/api/models/x",
            "http://huggingface.co/api/models/x",
            "https://huggingface.co/api/models/a b",
            "https://huggingface.co/api/users/x",
        ] {
            assert!(!hub_api_url(bad), "{bad}");
        }
    }
}
