//! target: crates/core/src/actions.rs
//! expect: accept
//! source: accept
//! why: a sink consulting human keep intent through the one predicate, `ProtectList::conflict`
/// Accept: protection asked, not re-implemented.
fn sweep_accept_protected(store: &std::path::Path, p: &std::path::Path) -> anyhow::Result<Option<String>> {
    let list = crate::protection::load_protect(store)?;
    Ok(list.conflict(p))
}
