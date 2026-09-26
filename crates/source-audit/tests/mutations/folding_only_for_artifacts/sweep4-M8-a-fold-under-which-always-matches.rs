//! target: crates/core/src/walk.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M8
//! by: compile:E0063
//! why: a fold under `if let Some(_) = Some(classify_at(..))`, which always matches
//! blind-spot (old model): a guard is an enclosing `if let` whose initializer text *contains* `classify_at`; wrapping the answer in `Some(..)` keeps the text and discards the classification
/// Sweep 4: classified in name only.
#[allow(dead_code)]
fn sweep4_fold_anything(
    pool: &Pool<AttrJob>,
    parent: &Path,
    name: &str,
    path: PathBuf,
    group: Arc<SizeGroup>,
) {
    if let Some(_kind) = Some(classify_at(parent, name)) {
        pool.push(AttrJob::Size { path, group });
    }
}
