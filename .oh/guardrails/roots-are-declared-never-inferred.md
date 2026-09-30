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

**Gate audit.** `no_root_inference_sources` scans every production string literal (constants, `format!`/`concat!` arguments and `#[serde(rename)]` strings included) for the names of those sources: `zsh_history`, `bash_history`, `fish_history`, `.gitconfig`, `includeIf`, `safe.directory`, `mdfind`, `mdls`, `kMDItem`, `com.apple.Spotlight`, `recentlyOpened`, `openedPathsList`, `recentProjects`, `recently-used`, `recentitems`, `zoxide`, `autojump`, `HISTFILE`, `.python_history`, `.node_repl_history`, `.config/git/config`, `state.vscdb`, `globalStorage/storage.json`, `.sfl2`/`.sfl3`, `mdutil`, `fasd` and the exact name `.z`. It covers every crate, not only the store modules. A program that needs one for another purpose is a reviewed exception added to the rule by name, in the open.

**Limitation (known gaps).** The scan sees literals, so a name assembled at run time (`format!`, `.concat()`, `push_str`, `+`) is not caught. Three such mutations are kept, documented and un-swept, in `crates/source-audit/tests/mutations_known_gaps/roots_are_declared_never_inferred/`. Spawning a Spotlight tool still has to pass the `fs_gate::spawn` allow-list; review is the last line.

**Runtime test.** `crates/core/tests/declared_roots.rs` asserts the first-run question proposes nothing it did not stat (`first_run_does_not_offer_or_propose_anything_it_did_not_stat`) and that declared-root status walks nothing.
