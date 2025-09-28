// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;

use rand::distr::{Distribution, Uniform};
use rand::prelude::IteratorRandom;
use rand_chacha::ChaCha20Rng;

use crate::arch;

pub fn init(prng: Option<ChaCha20Rng>) -> PageAllocator {
    PageAllocator {
        page_state: [false; arch::PAGE_TABLE_ENTRIES / 2],
        prng,
    }
}

/// Virtual memory allocator for setting up initial mappings.
///
/// All regions will be huge page (1GiB) aligned.
#[derive(Debug)]
pub struct PageAllocator {
    /// Whether a top-level page is in use.
    page_state: [bool; arch::PAGE_TABLE_ENTRIES / 2],
    /// A random number generator that should be used to generate random addresses or
    /// `None` if aslr is disabled.
    prng: Option<ChaCha20Rng>,
}

impl PageAllocator {
    fn allocate_pages(&mut self, num_pages: usize) -> usize {
        // find a consecutive range of `num` entries that are not used
        let mut free_pages = self
            .page_state
            .windows(num_pages.div_ceil(8))
            .enumerate()
            .filter_map(|(idx, entries)| {
                if entries.iter().all(|used| !used) {
                    Some(idx)
                } else {
                    None
                }
            });

        let maybe_idx = if let Some(rng) = self.prng.as_mut() {
            free_pages.choose(rng)
        } else {
            free_pages.next()
        };

        if let Some(idx) = maybe_idx {
            for i in 0..num_pages {
                self.page_state[idx + i] = true;
            }

            idx
        } else {
            panic!("no usable top-level pages found ({num_pages} pages requested)");
        }
    }

    pub fn reserve(&mut self, mut virt_base: usize, mut remaining_bytes: usize) {
        log::trace!(
            "marking {virt_base:#x}..{:#x} as used",
            virt_base.checked_add(remaining_bytes).unwrap()
        );

        let top_level_page_size = arch::page_size_for_level(arch::PAGE_TABLE_LEVELS - 1);
        debug_assert!(virt_base.is_multiple_of(top_level_page_size));

        while remaining_bytes > 0 {
            let page_idx = (virt_base - (usize::MAX << arch::VIRT_ADDR_BITS)) / top_level_page_size;

            self.page_state[page_idx] = true;

            virt_base = virt_base.checked_add(top_level_page_size).unwrap();
            remaining_bytes -= top_level_page_size;
        }
    }

    pub fn allocate(&mut self, layout: Layout) -> Range<usize> {
        assert!(layout.align().is_power_of_two());

        let top_level_page_size = arch::page_size_for_level(arch::PAGE_TABLE_LEVELS - 1);

        // how many top-level pages are needed to map `size` bytes
        // and attempt to allocate them
        let page_idx = self.allocate_pages(layout.size().div_ceil(top_level_page_size));

        // calculate the base address of the page
        //
        // we know that entry_idx is between 0 and PAGE_TABLE_ENTRIES / 2
        // and represents a top-level page in the *higher half* of the address space.
        //
        // we can then take the lowest possible address of the higher half (`usize::MAX << VA_BITS`)
        // and add the `idx` multiple of the size of a top-level entry to it
        let base = (usize::MAX << arch::VIRT_ADDR_BITS) + page_idx * top_level_page_size;

        let offset = if let Some(rng) = self.prng.as_mut() {
            // Choose a random offset.
            let max_offset = top_level_page_size - (layout.size() % top_level_page_size);

            if max_offset / layout.align() > 0 {
                let uniform_range = Uniform::new(0, max_offset / layout.align()).unwrap();

                uniform_range.sample(rng) * layout.align()
            } else {
                0
            }
        } else {
            0
        };

        base.checked_add(offset).unwrap()..base.checked_add(offset + layout.size()).unwrap()
    }
}
