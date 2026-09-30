# Known gaps of `no_root_inference_sources`

These three mutations are NOT rejected by the audit and are deliberately outside
`tests/mutations/`, so the mutation sweep (which requires every fixture there to be
rejected) stays green:

- `05-history-format-concat-adv.rs`: `format!("{}{}", ".zsh", "_history")`
- `06-history-slice-concat-adv.rs`: `[".bash", "_history"].concat()`
- `08-spotlight-name-split-adv.rs`: `"md"` then `push_str("find")`

The rule is a static scan of string literals (with `concat!` joined). A name that is
assembled from pieces at run time through `format!`, `.concat()`, `push_str` or `+` never
exists as one literal in the source, and catching it would need type-resolved data flow.
The guardrail file states the limitation; the runtime test
`first_run_does_not_offer_or_propose_anything_it_did_not_stat` and the fact that a spawn
of Spotlight tools must pass the `fs_gate::spawn` allow-list are the second lines of
defense. Review is the third: a diff that builds such a name is a violation of
`.oh/guardrails/roots-are-declared-never-inferred.md` whether or not the audit sees it.
