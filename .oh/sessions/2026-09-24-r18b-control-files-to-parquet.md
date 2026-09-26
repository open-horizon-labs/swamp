# R18b: the last JSON control files -> Parquet (2026-09-24)

Owner-run slice, straight after R20. Every remaining non-Parquet data
file under the store is converted; `fs_gate::store::JsonFile` keeps one
variant (`UiState`).

| was | now | notes |
| --- | --- | --- |
| `<volume>/fsevents.json` | `<volume>/cursors.parquet` (rows: `walk`, `unit_root`) | `growth::read_fsevents_anchor`/`write_fsevents_anchor` are the public whole-anchor accessors (tests that aged `rules_version` by rewriting JSON use them); the two observation-path writers still merge families. |
| `docker_facts.json` | `docker_meta/images/build_cache/volumes/builders/values/containers.parquet` | `docker::write_cached_facts`/`read_cached_facts`; TTL from `docker_meta.cached_at`. Labels, tags, layers, parents, buildx limits and container refs are typed child rows. |
| `scope.json` | `scope.parquet` + `scope_values` + `scope_roots` + `scope_root_reasons` | `coverage_changes` needs only roots/reasons/status; detector summaries and prune notes are not persisted (nothing read them). |
| `last_run.json` | `scheduled_runs.parquet` (one row) | `schedule::{write,read}_last_run` unchanged in signature. |
| `ledger.jsonl` | `ledger.parquet` + `ledger_facts.parquet` | `ActionRecord.evidence: Vec<LedgerFact>` (was `serde_json::Value`); the TUI's confirm-line facts become rows. `LedgerFile` in the gate replaces `LogFile::Ledger*`; append = read + rewrite (a ledger is a human's few actions). |
| `continuity/<id>.json` | `continuity/<id>.parquet` + `<id>_entries.parquet` | Linux collector checkpoint; `consume()` goes through `read_checkpoint`. `.sync`/`.lock` stay (control tokens, not data). |

`growth::columns` gained a `table!` macro (row struct = columns, in
order; `write_<x>_rows`/`read_<x>_rows` generated) over a `Col` trait
for the scalar types, all through one `write_table` that is named in the
audit's `TABLE_WRITERS`. Hand-written writers outside it are still
flagged by the audit; a new `table!` invocation is caught by the runtime
allow-list (`store_contents_are_allowlisted`, exact names).

Removed: `fs_gate::read::read_owned_lines`, `JsonFile::{Scope, LastRun,
DockerFacts, FsEventsCursor}`, `LogFile::{Ledger, LedgerResolved}`, the
`Encoding` distinction.

Not converted, on purpose: `ui_state.json` (allowed), `config.toml`,
`observe.log` (a text log), `*.lock`, `continuity/<id>.sync` (a token),
and the Trash envelope's `restore.json` (written beside the moved files
in the Trash, not under the store -- a recovery manifest for the human).
