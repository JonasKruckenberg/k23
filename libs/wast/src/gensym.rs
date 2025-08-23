use core::cell::Cell;

use cpu_local::cpu_local;

use crate::token::{Id, Span};

cpu_local!(static NEXT: Cell<u32> = Cell::new(0));

pub fn reset() {
    NEXT.set(0);
}

pub fn generate(span: Span) -> Id<'static> {
    let generation = NEXT.get() + 1;
    NEXT.set(generation);
    Id::gensym(span, generation)
}

pub fn fill<'a>(span: Span, slot: &mut Option<Id<'a>>) -> Id<'a> {
    *slot.get_or_insert_with(|| generate(span))
}
