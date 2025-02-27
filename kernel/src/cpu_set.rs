// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#[repr(transparent)]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct LogicalCpuId(usize);

impl LogicalCpuId {
    pub const fn new(inner: usize) -> Self {
        Self(inner)
    }
    pub const fn get(self) -> usize {
        self.0
    }
}

impl core::fmt::Debug for LogicalCpuId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[logical cpu #{}]", self.0)
    }
}
impl core::fmt::Display for LogicalCpuId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "#{}", self.0)
    }
}
