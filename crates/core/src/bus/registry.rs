//! The event bus's registry: the only code that can add a consumer.
//! `bus/mod.rs` holds the events, the context and the report entry
//! point; this module holds the consumer list and the dispatcher, so
//! `register` is private to the one function that uses it.

use super::{Consumer, Ctx, Event};

/// Proof that code is running as a stage of the event bus: minted only
/// by [`EventBus::run`] (the field is private to this module, and there
/// is no other constructor outside tests) and handed to each consumer's
/// `on_event`.
///
/// The pipeline stages that do the measuring -- `walk::discover_and_attribute`,
/// `growth::stage_tracked_with_source` -- take a `&Stage`, so calling a
/// stage directly from `report.rs` (or through a fn item bound to a
/// local, returned from a helper, stored in a `const`) does not compile:
/// there is no `Stage` to pass (`.oh/guardrails/all-report-paths-through-bus.md`).
#[derive(Debug)]
pub struct Stage {
    _minted_by_the_bus: (),
}

impl Stage {
    fn mint() -> Stage {
        Stage {
            _minted_by_the_bus: (),
        }
    }

    /// A stage token for tests that drive a stage function directly
    /// (`testing` feature or this crate's unit tests only).
    #[cfg(any(test, feature = "testing"))]
    pub fn for_tests() -> Stage {
        Stage::mint()
    }
}
use anyhow::{Result, bail};
use std::collections::VecDeque;

pub struct EventBus {
    consumers: Vec<Box<dyn Consumer>>,
    sealed: bool,
}

impl EventBus {
    fn new() -> Self {
        EventBus {
            consumers: Vec::new(),
            sealed: false,
        }
    }

    /// Every builtin stage, in registration order. Static: this is the
    /// one place the pipeline's membership is written down.
    pub fn with_builtins() -> Self {
        use crate::consumers::*;
        let mut bus = EventBus::new();
        for c in [
            Box::new(WalkConsumer::default()) as Box<dyn Consumer>,
            Box::new(ProjectsConsumer),
            Box::new(SignalsConsumer),
            Box::new(EcosystemConsumer),
            Box::new(GithubConsumer::default()),
            Box::new(DockerConsumer),
            Box::new(AssemblyGate::default()),
            Box::new(CargoConsumer::default()),
            Box::new(GrowthConsumer),
            Box::new(TrackingConsumer),
            Box::new(HistoryConsumer),
            Box::new(ReportAssembler::default()),
            Box::new(CacheWriter),
        ] {
            bus.register(c).expect("builtins register before any run");
        }
        bus
    }

    /// Registers a consumer. Refused once `run` has started: the registry
    /// is fixed before the first event fires.
    ///
    /// Private to this module: [`EventBus::with_builtins`] is the only
    /// caller, so a late or second registration -- through an untyped
    /// receiver, a wrapper, `Box::from`, anywhere in `bus/mod.rs` or
    /// outside it -- does not compile
    /// (`.oh/guardrails/extractors-are-pluggable.md`).
    fn register(&mut self, consumer: Box<dyn Consumer>) -> Result<()> {
        if self.sealed {
            bail!(
                "event bus is sealed: `{}` cannot register after run started",
                consumer.name()
            );
        }
        self.consumers.push(consumer);
        Ok(())
    }

    pub fn consumer_names(&self) -> Vec<&str> {
        self.consumers.iter().map(|c| c.name()).collect()
    }

    /// Dispatches `seed` and every follow-on until the queue is empty.
    /// Subscribers of one event run concurrently; their follow-ons are
    /// queued depth-first (a follow-on is dispatched before anything that
    /// was already waiting). Returns every event that was dispatched, in
    /// dispatch order.
    pub async fn run(&mut self, seed: Event, ctx: &Ctx<'_>) -> Result<Vec<Event>> {
        self.sealed = true;
        let stage = Stage::mint();
        let mut queue: VecDeque<Event> = VecDeque::from([seed]);
        let mut dispatched: Vec<Event> = Vec::new();
        while let Some(event) = queue.pop_front() {
            let kind = event.kind();
            let subscribers: Vec<&dyn Consumer> = self
                .consumers
                .iter()
                .map(|c| c.as_ref())
                .filter(|c| c.subscribes_to().contains(&kind))
                .collect();
            let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
            let results = futures_util::future::join_all(subscribers.iter().map(|c| async {
                let t = std::time::Instant::now();
                let r = c.on_event(&event, ctx, &stage).await;
                if trace {
                    eprintln!("[trace] {:?} → {}: {:?}", kind, c.name(), t.elapsed());
                }
                r
            }))
            .await;
            let mut follow_on: Vec<Event> = Vec::new();
            for (c, r) in subscribers.iter().zip(results) {
                let events =
                    r.map_err(|e| anyhow::anyhow!("consumer `{}` on {:?}: {e}", c.name(), kind))?;
                follow_on.extend(events);
            }
            for e in follow_on.into_iter().rev() {
                queue.push_front(e);
            }
            dispatched.push(event);
        }
        Ok(dispatched)
    }

    /// `run` on a fresh tokio current-thread runtime, for the synchronous
    /// callers (CLI, TUI worker thread). Must not be called from
    /// inside another tokio runtime.
    pub fn run_blocking(&mut self, seed: Event, ctx: &Ctx<'_>) -> Result<Vec<Event>> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(self.run(seed, ctx))
    }
}

#[cfg(test)]
impl EventBus {
    /// An empty bus, for the routing tests.
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    /// [`EventBus::register`], for the routing tests.
    pub(crate) fn register_for_test(&mut self, consumer: Box<dyn Consumer>) -> Result<()> {
        self.register(consumer)
    }
}
