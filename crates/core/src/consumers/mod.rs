//! The builtin stages of the report pipeline, one per file. Each is a
//! [`crate::bus::Consumer`] that reads the events it subscribes to and
//! emits facts; none names another. Registration lives in
//! `bus::EventBus::with_builtins`, nowhere else.

mod assemble;
mod cache;
mod cargo;
mod docker;
mod ecosystem;
mod gate;
mod github;
mod growth;
pub(crate) use growth::{add_nested_history_rows, copy_nested_history};
mod history;
mod projects;
mod signals;
mod tracking;
mod walk;

pub use assemble::ReportAssembler;
pub use cache::CacheWriter;
pub use cargo::CargoConsumer;
pub use docker::DockerConsumer;
pub use ecosystem::EcosystemConsumer;
pub use gate::AssemblyGate;
pub use github::GithubConsumer;
pub use growth::GrowthConsumer;
pub use history::HistoryConsumer;
pub use projects::ProjectsConsumer;
pub use signals::SignalsConsumer;
pub use tracking::TrackingConsumer;
pub use walk::WalkConsumer;
