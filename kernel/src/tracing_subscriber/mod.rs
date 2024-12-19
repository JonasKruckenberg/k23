#![allow(unused)]

mod registry;

use registry::Registry;
use tracing::collect::Interest;
use tracing::span::{Attributes, Record};
use tracing::{Collect, Event, Id, Metadata};
use tracing_core::span::Current;

struct Subscriber {
    registry: Registry,
}

impl Collect for Subscriber {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        todo!()
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        todo!()
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        self.registry.new_span(attrs)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        todo!()
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        todo!()
    }

    fn event(&self, event: &Event<'_>) {
        todo!()
    }

    fn enter(&self, id: &Id) {
        self.registry.enter(id);
    }

    fn exit(&self, id: &Id) {
        self.registry.exit(id);
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.registry.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.registry.try_close(id)
    }

    fn current_span(&self) -> Current {
        self.registry.current_span()
    }
}
