//! protection-fails-closed (re-review 5, finding 7): the production
//! listing is a rendering. It has no iterator, no entries and no
//! containment query, so a second (one-directional) protection predicate
//! over it does not compile -- and the raw path list exists only under
//! the `testing` feature.
use std::path::Path;

fn main() {
    let listing = swamp_core::agents::protect_listing(Path::new("/tmp/store")).unwrap();
    let _hit = listing.iter().any(|p| Path::new("/w/x").starts_with(p));
    let _raw = swamp_core::agents::protect_list(Path::new("/tmp/store"));
}
