---
id: roots-are-declared-never-inferred
severity: hard
statement: "A source root exists only because the built-in catalog names it or the user declared it (`swamp config add-root`, or the first-run answer). No code reads shell history, an editor's recent-project list, git's includeIf/safe.directory configuration, a directory jumper's database or Spotlight to propose or add one."
outcome: disk-growth-by-project
audit: no_root_inference_sources
runtime_tests:
  - crates/core/tests/declared_roots.rs
---

## Rationale

Guessing where someone keeps their code needs a broader look at their machine than measuring the code does, and a wrong guess puts a directory swamp was never asked about into every report. Shell history, editor recents, `~/.gitconfig` and Spotlight all hold an approximation of "where I work"; each is a private record the user did not hand to swamp for this purpose. The user says where their source is, once, and swamp records it in `[scan] include`.

The first-run question offers `~/src` only if that one path exists and lists nothing else; `swamp scope`, `swamp report` and `swamp config show` read the declared roots and the stored observation and walk nothing.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `no_root_inference_sources` scans every production string literal (constants, `format!`/`concat!` arguments and `#[serde(rename)]` strings included) for the names of those sources: `zsh_history`, `bash_history`, `fish_history`, `.gitconfig`, `includeIf`, `safe.directory`, `mdfind`, `mdls`, `kMDItem`, `com.apple.Spotlight`, `recentlyOpened`, `openedPathsList`, `recentProjects`, `recently-used`, `recentitems`, `zoxide`, `autojump`. A program that needs one for another purpose is a reviewed exception added to the rule by name, in the open.

**Runtime test.** `crates/core/tests/declared_roots.rs` asserts the first-run question proposes nothing it did not stat (`first_run_does_not_offer_or_propose_anything_it_did_not_stat`) and that declared-root status walks nothing.
