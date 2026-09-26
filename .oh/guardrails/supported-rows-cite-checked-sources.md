---
id: supported-rows-cite-checked-sources
severity: hard
statement: "A Supported row in the agent support matrix cites an upstream file at a pinned commit (or a documentation page at a retrieval date), that file is vendored as a minimal excerpt with its blake3 digest, CI greps the excerpt for every symbol the claim depends on, and no prose paragraph asserts doubt about a tool its own row lists as supported."
outcome: decision-relevant-storage-evidence
audit: none
audit_none_reason: "2026-09-22: the claim is about upstream files and vendored excerpts, not about this workspace's source, so no source audit can check it; the runtime tests below do, offline, on every run"
runtime_tests:
  - crates/core/tests/upstream_citations_are_checked.rs
  - crates/core/tests/agent_matrix_matches_docs.rs::no_prose_paragraph_asserts_doubt_a_supported_row_has_resolved
  - crates/core/tests/agent_matrix_matches_docs.rs
---

## Rationale

The 2026-09-21 review's objection was "fixture-backed labels are not
proof of real format support", and `SupportLevel` plus
`MatrixEntry::verification` answered it by requiring a citation. The
2026-09-22 independent re-review fetched all fourteen rows' citations
and showed the objection had only moved one level up: **the citation was
now present and unchecked**.

What CI guaranteed about a `Supported` row's provenance was that the
string was longer than thirty characters
(`agent_matrix_matches_docs.rs`) and non-empty
(`matrix.rs::a_supported_row_cites_what_confirmed_it`). Six of twelve
`Supported` rows cited something that did not establish the claim, and
four of those were consequential in the shipped adapter:

- Gemini CLI's `bin/` cache was at a path no version writes
  (`tmp/bin`), so the unit never appeared;
- OpenCode's `session_diff` bytes were in no unit at all, because the
  row called a `.json` file a directory;
- Codex's seventh SQLite store was unfolded and unprotected, because the
  row said six;
- Cline refused a linkage that was available in a file the adapter
  already walked, because the row named the wrong store.

Five `Supported` rows with actions enabled cited no commit or date at
all ("read during chunk #93"), which is a provenance string nobody can
re-check. One row cited a file that does not contain the string the
claim rests on -- the claim was true; the citation did not establish it.

A length check on a provenance string is not provenance.

## Detection

Mechanism: runtime test.

`audit: none`, deliberately, and this is the honest label rather than a
missing one: the property is about the *content of an upstream file*,
which no AST pass over this workspace can see. It is enforced by three
runtime checks instead, which is why they are named in the frontmatter
and in `scripts/check.sh` rather than left to be found:

1. **Every vendored excerpt hashes to its recorded digest.** An excerpt
   edited to make a claim fit stops matching, so the fixture cannot be
   quietly bent toward the assertion.
2. **Every excerpt contains every symbol its citation lists.** A claim
   edited past what the source says fails naming the citation. The
   manifest's symbol lists are single-quoted so a symbol may itself
   contain a double quote -- several must
   (`workspaceDirectory: ""`, `"state", "taskHistory.json"`), and
   dropping the quotes would weaken exactly the citations that need to
   be strongest.
3. **Every `Supported` row pins a commit or a date, and every citation
   naming a repository file has a vendored excerpt; every vendored file
   is cited by the manifest.** Both directions, so neither a row without
   evidence nor evidence without a row can accumulate.

Plus the prose half: a paragraph in `docs/agent-storage.md` that names a
`Supported` tool and no `Unverified` one may not assert "no confirmed",
"assumed", "community-documented", "not re-fetched", "not modeled yet"
or "not independently re-confirmed" unless the same paragraph marks that
doubt as history ("previously", "used to", "was wrong", "superseded",
...). The table and the prose used to be reconciled only by people
reading both, and three paragraphs contradicted the table above them.

**Limits.** These are offline checks against a *vendored* excerpt: they
prove the excerpt was read and still says what the row claims, not that
the excerpt is a faithful extract of the upstream file. Nothing here
re-fetches at test time, on purpose -- a check that needs the network is
a check that is skipped. Re-fetching is a human act, recorded by bumping
the pinned commit. A licence that forbids redistribution gets a
`NO-VENDOR-*` note of line numbers and symbol strings, which is weaker
than a citation and backs no claim.

## Runtime tests that complete it

- `crates/core/tests/upstream_citations_are_checked.rs` -- the three
  checks above, over 45 citations covering all fourteen rows.
- `crates/core/tests/agent_matrix_matches_docs.rs` -- the table cells
  against `matrix.rs`, plus
  `no_prose_paragraph_asserts_doubt_a_supported_row_has_resolved`. That
  test found a real stale paragraph on its first run (Codex desktop
  claiming no Windows build was confirmed, which its own cited source
  confirms) and rejects the exact Continue sentence the re-review
  quoted.
- Each corrected adapter's own five contract tests, plus the specific
  adversarial ones the corrections needed:
  `the_downloaded_tools_cache_is_tmp_bin_and_not_a_project_id`,
  `every_one_of_the_seven_runtime_databases_is_folded_and_protected`,
  `session_diff_files_are_counted_claimed_or_not`,
  `a_task_in_the_history_file_resolves_to_its_declared_project`,
  `the_history_file_is_read_once_per_host_not_once_per_task`,
  `a_plausible_cwd_field_is_not_a_citation_and_must_not_link`,
  `an_empty_workspace_directory_is_unresolved_not_missing`,
  `unmatched_todos_are_folded_into_one_orphan_note_not_dropped`,
  `seatbelt_moves_the_runtime_dir_to_the_cache_and_only_then`,
  `the_shared_data_root_follows_upstreams_override_order`.
