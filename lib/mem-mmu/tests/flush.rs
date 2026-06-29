// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use mem_core::VirtualAddress;
use mem_mmu::Flush;
use mem_testkit::proptest::any_virt;
use proptest::prelude::*;

proptest! {
    /// `invalidate` must never panic, regardless of how many ranges are pushed, and
    /// every pushed range must remain covered by the resulting `Flush`.
    #[test]
    fn invalidate_records_every_range_without_panicking(
        ranges in proptest::collection::vec(
            (any_virt(), any_virt())
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
