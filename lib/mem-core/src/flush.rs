// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem;
use core::range::Range;

use arrayvec::ArrayVec;

use crate::VirtualAddress;
use crate::arch::Arch;

pub enum Flush<const CAP: usize = 16> {
    Ranges(ArrayVec<Range<VirtualAddress>, CAP>),
    All,
}

impl Default for Flush {
    fn default() -> Self {
        Self::new()
    }
}

impl Flush {
    pub const fn new() -> Self {
        Self::Ranges(ArrayVec::new())
    }

    /// Flush the range of virtual addresses from the TLB.
    pub fn flush<A>(self, arch: &A)
    where
        A: Arch,
    {
        match self {
            Flush::Ranges(ranges) => {
                for range in ranges {
                    log::trace!("flushing range {range:?}");
                    arch.fence(range);
                }
            }
            Flush::All => {
                log::trace!("flushing entire address space");
                arch.fence_all();
            }
        }
    }

    /// Ignores the contents of the `Flush` and will NOT issue TLB invalidations.
    ///
    /// # Safety
    ///
    /// Not flushing after mutating the page translation tables will likely lead to unintended
    /// consequences such as inconsistent views of the address space between different cpus.
    ///
    /// You should only call this if you know what you're doing.
    pub const unsafe fn ignore(self) {
        mem::forget(self);
    }

    /// Records `range` as needing TLB invalidation.
    pub fn invalidate(&mut self, range: Range<VirtualAddress>) {
        match self {
            Flush::Ranges(ranges) => {
                // Coarsen to a full flush once the range buffer is full.
                if ranges.try_push(range).is_err() {
                    *self = Flush::All;
                }
            }
            Flush::All => {}
        }
    }

    pub fn invalidate_all(&mut self) {
        *self = Flush::All;
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// `invalidate` must never panic, regardless of how many ranges are pushed, and
        /// every pushed range must remain covered by the resulting `Flush`.
        #[test]
        fn invalidate_records_every_range_without_panicking(
            ranges in proptest::collection::vec(
                (any::<VirtualAddress>(), any::<VirtualAddress>())
                    .prop_map(|(a, b)| Range::from(a.min(b)..a.max(b))),
                0..256,
            ),
        ) {
            let mut flush = Flush::new();
            for range in &ranges {
                flush.invalidate(*range);
            }

            match flush {
                // Coarsening to `All` covers every range trivially.
                Flush::All => {}
                // Otherwise every pushed range must have been recorded.
                Flush::Ranges(recorded) => {
                    for range in &ranges {
                        prop_assert!(recorded.contains(range));
                    }
                }
            }
        }
    }
}
