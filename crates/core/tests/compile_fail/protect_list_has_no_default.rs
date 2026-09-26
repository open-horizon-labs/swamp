//! protection-fails-closed: an unreadable protect file is an error, and
//! there is no empty default to swallow it into. Callers cannot turn a
//! failed read into an empty list with `.unwrap_or_default()` because
//! `ProtectList` does not satisfy `Default`.

fn requires_default<T: Default>() {}

fn main() {
    requires_default::<swamp_core::protection::ProtectList>();
}
