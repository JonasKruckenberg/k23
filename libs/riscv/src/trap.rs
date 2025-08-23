use crate::scause::{Exception, Interrupt};

pub type Trap = trap::Trap<Interrupt, Exception>;
