# 2026-09-23 — Token binding and gate hardening (stack/22)

Re-review 5 (`review/REVIEW-STACK-5.md`, halted after static reading)
found the capability-gate tokens sound and their *inputs* not: the
values a token was minted from, or a recheck compared against, were
caller-chosen. This chunk fixes all eight findings. Architecture:
`docs/architecture.md`, "Capability gates" ("Provenance", "Trusted
base").

## 1. Decisions

- **Integrity binding = keyed MAC under a per-store key.** `fs_gate::key`
  creates `<store>/authority.key` (32 random bytes from two v4 UUIDs,
  mode 0600, `O_EXCL`). Plan and grant records carry
  `blake3::keyed_hash(key, domain ‖ canonical_json(record))`. The loader
  (`load_plan`, `list_grants`) is the only code that builds a `Plan` or
  `Grant` from bytes; neither is `Deserialize`. One bad grant refuses the
  whole grants file (fail closed, like a corrupt protect list).
  *Limit, stated in the docs:* the key is readable by the user swamp runs
  as; a process that reads it and reimplements the MAC can forge. The
  binding closes every path that does not run swamp's code.
- **Approvals bind content, not ids.** `Plan::content_digest` (id, root,
  times, proposer, every unit; canonical JSON, cached per plan). The
  one-shot grant records it; `authorize` refuses a plan without it.
  `Plan::id` stays a public field because the reviewers' byte-for-byte
  suites read `plan.id`; renaming changes the digest, so it buys nothing
  (`token_binding.rs::an_approval_does_not_cover_different_content_under_the_same_id`).
- **Confirmations name a subject and are spent by value.** Four
  constructors (`cli_approve(&plan)`, `cli_grant(terms)`,
  `cli_protect(change)`, `tui_dialog(store, items)`), each pinned by
  resolved fn path to one handler (`cmd_approve`, `cmd_grant_add`,
  `cmd_protect`, `start_delete`). The brief says "the exact two
  functions"; standing grants and protect changes also need a handler,
  so there are four, each binding a different subject. The TUI dialog
  mints one confirmation per marked unit rather than one splittable
  token.
- **`run_all(&Authorized)`.** The token carries anchor, reviewed
  identity, sidecar member identities (new `PlanUnit::reviewed_members`,
  captured at proposal for session members and Cargo companions),
  store, Docker removal, linked-worktree flag and preserve destination.
  The TUI's store is now the resolved swamp dir carried by the
  confirmation, not the ledger's parent directory -- under
  `SWAMP_LEDGER_PATH` that used to hide every protect entry from the TUI
  sink (`protection_comes_from_the_confirmations_store_not_the_ledger_directory`).
- **`StoreDir` and the reviewers' `&Path` API.** The brief asks for store
  functions to take a `StoreDir` "only from the resolved swamp dir". The
  core entry points keep `dir: &Path` because the reviewers' suites call
  them with temp dirs; they wrap it with `StoreDir::at` (absolute, not a
  symlink, a directory if present), which the audit allows only in the
  store modules. The CLI and TUI use `StoreDir::resolved()`. What the
  gate no longer takes is a *file* path: every store file is a fixed name
  in a `StoreDir`, the ledger is a `ledger.jsonl` or the env override read
  in the gate, the plist is resolved in the gate, `list_owned(path)` and
  `create_dir_all(path)` are gone.
- **Spawn allow-lists** are per-program argument *shapes*
  (`fs_gate::spawn::shapes`): literal verbs/flags plus typed operands.
  `git` has no shape (only `destroy` runs it). The GraphQL shape requires
  `query=` to open with `query(` so a `mutation` cannot ride the
  read-only call.
- **gix inside the gate** (`fs_gate::git::{Repo, IgnoreLens}`), rather
  than an allow-listed group: signals and ignore ask five read-only
  questions, so wrapping was smaller than proving gix's reachable
  capabilities from outside. `gix`/`gix_*` are gate paths; clippy names
  `gix::open`/`discover`/`init` and `gix::Repository`.
- **Release test build: graph check instead of `compile_error!`.**
  Features unify for test builds, so no `cfg` distinguishes `cargo test
  --release` from a shipped release build without a custom `--cfg` in
  RUSTFLAGS, which would force the release workflow to compile
  everything twice. The shipped graph is checked directly instead
  (`cargo tree -p swamp -e normal,build,features` must not contain
  `swamp-core feature "testing"`) in `scripts/check.sh` and before the
  release build; the audit's `[dependencies]` rule stays.
- **Two tiers.** `check.sh` runs each step once (clippy `--all-targets`
  only -- it includes the non-test lib/bins; the named runtime targets
  are `--no-run`, since the workspace run already ran them; the cost test
  is skipped by name there because the reviewer file cannot carry
  `#[ignore]`). `check-full.sh` = `check.sh` + compile-fail + mutation
  sweep (both `#[ignore]`d) + the cost test with `--test-threads=1`.
  CI: `.github/workflows/check-full.yml` (macOS). **Rebase note:** the
  Linux track's `ci.yml` (stack/15, stack/18) should run
  `scripts/check-full.sh` on Ubuntu too; that is also where the cost
  test's `linux_contract` runs.

## 2. Evidence

- 18 new compile-fail cases (58 total), each reviewed to fail for its
  own reason (E0451/private-fields literal, E0277 not `Deserialize`,
  E0603 private fn, E0382 spent confirmation, E0061/E0308 old
  signatures, E0599 no bare constructor, E0616 private field).
- `crates/core/tests/token_binding.rs`: 15 disposable-fixture tests,
  each asserting refusal *and* untouched fixture bytes; two TUI unit
  tests; spawn unit tests for 18 refused invocations (zero spawns).
- 18 new mutation fixtures (`mutations/token_binding_and_gate_hardening/`)
  plus 3 accept fixtures; 8 existing fixtures ported to the new API
  (`ported: 2026-09-23` lines). accept/07's old shape (a helper minting
  its own confirmation) is now a rejection (`…/18`).

## Timings (macOS arm64, shared warm target dir)

| run | wall | result |
|---|---|---|
| `scripts/check.sh` (fast tier, warm) | 8:00 (tests step 7:57) | exit 0; 66 test binaries, 1073 passed, 0 failed |
| `scripts/check-full.sh` (first run after the edits) | 26:05 | exit 0; fast tier 12:25 (+ compile after edits), compile-fail 0:08, mutation sweep 12:40, cost test 0:52 |
| mutation sweep alone (both tests) | 12:35 | fixtures 23/23 accept, 125/125 corpus, 18/18 re-review 5, 45/45 review-3, 46+12+7+3 review-4; operators 1196/1196 |
| `cargo test --workspace --release --locked` | 13:54 (cold release build) | exit 0; 1074 passed, 0 failed, 5 ignored |

The fast tier is at its 8-minute target, not under it: the workspace
test run is the whole cost (clippy, audits and the graph check take ~3 s
warm). The previous `check.sh` took 35:02 because it ran the sweep,
compile-fail cases and the named suites twice.

## 3. Limits

- The audit pins constructor *functions*; the body of a pinned function
  (and all of `fs_gate`) is trusted base a reviewer reads.
- `StoreDir::at` accepts any real absolute directory; it binds names,
  not the location, for the store modules' `&Path` entry points.
- Old plans and grants (no binding) are refused, not migrated.
