// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Generic trap/exception handling types

#![no_std]

/// Generic trap type that can represent either an interrupt or an exception
/// The specific Interrupt and Exception types are defined by each architecture
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Trap<I, E> {
    Interrupt(I),
    Exception(E),
}

/// Generic trait for interrupt controllers
pub trait InterruptController {
    /// Mask (disable) an interrupt
    fn irq_mask(&mut self, irq: u32);

    /// Unmask (enable) an interrupt
    fn irq_unmask(&mut self, irq: u32);

    /// Complete interrupt handling and get next pending interrupt
    fn irq_complete(&mut self) -> Option<u32>;
}
