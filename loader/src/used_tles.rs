use crate::kconfig;
use core::ops::Range;
use kmm::{Mode, VirtualAddress};
use rand::distributions::{Distribution, Uniform};
use rand::prelude::IteratorRandom;
use rand_chacha::ChaCha20Rng;

#[derive(Debug)]
pub struct UsedTLEs {
    /// Whether an entry is in use by the kernel.
    entry_state: [bool; kconfig::MEMORY_MODE::PAGE_TABLE_ENTRIES],
    /// A random number generator that should be used to generate random addresses or
    /// `None` if aslr is disabled.
    rng: Option<ChaCha20Rng>,
}

impl UsedTLEs {
    pub fn new(rng: ChaCha20Rng) -> Self {
        let mut this = Self {
            entry_state: [false; kconfig::MEMORY_MODE::PAGE_TABLE_ENTRIES],
            rng: Some(rng),
        };

        // mark the zero page as used
        this.entry_state[0] = true;

        this
    }

    // fn mark_range_as_used(&mut self, range: Range<VirtualAddress>) {
    //     const SHIFT: usize = kconfig::MEMORY_MODE::PAGE_ENTRY_SHIFT
    //         * (kconfig::MEMORY_MODE::PAGE_TABLE_LEVELS - 1)
    //         + kconfig::MEMORY_MODE::PAGE_SHIFT;
    //
    //     let start = (range.start.as_raw() >> SHIFT) & kconfig::MEMORY_MODE::PAGE_ENTRY_MASK;
    //     let end = (range.end.as_raw() >> SHIFT) & kconfig::MEMORY_MODE::PAGE_ENTRY_MASK;
    //
    //     for i in start..=end {
    //         self.entry_state[i] = true;
    //     }
    // }

    pub fn get_free_entries(&mut self, num_entries: usize) -> usize {
        // find a consecutive range of `num` entries that are not used
        let mut free_entries = self
            .entry_state
            .windows(num_entries.div_ceil(8))
            .enumerate()
            .filter_map(|(idx, entries)| {
                if entries.iter().all(|used| !used) {
                    Some(idx)
                } else {
                    None
                }
            });

        let maybe_idx = if let Some(rng) = self.rng.as_mut() {
            free_entries.choose(rng)
        } else {
            free_entries.next()
        };

        if let Some(idx) = maybe_idx {
            for i in 0..num_entries {
                self.entry_state[idx + i] = true;
            }

            idx
        } else {
            panic!("no usable top-level entries found ({num_entries} entries requested)");
        }
    }

    pub fn get_free_range(&mut self, size: usize, alignment: usize) -> Range<VirtualAddress> {
        assert!(alignment.is_power_of_two());

        // calculate the size of a top-level entry
        const TLE_SIZE: usize = kconfig::PAGE_SIZE
            << (kconfig::MEMORY_MODE::PAGE_ENTRY_SHIFT
                * (kconfig::MEMORY_MODE::PAGE_TABLE_LEVELS - 1));

        // how many top-level entries are needed to map `size` bytes
        // and attempt to allocate them
        let entry_idx = self.get_free_entries(size.div_ceil(TLE_SIZE));

        // calculate the base address of the top-level entry
        let base = entry_idx
            << (kconfig::MEMORY_MODE::PAGE_ENTRY_SHIFT
                * (kconfig::MEMORY_MODE::PAGE_TABLE_LEVELS - 1)
                + kconfig::MEMORY_MODE::PAGE_SHIFT);
        let base = {
            const SHIFT: u32 = usize::BITS - kconfig::MEMORY_MODE::VA_BITS;

            VirtualAddress::new(((base << SHIFT) as isize >> SHIFT) as usize)
        };

        let offset = if let Some(rng) = self.rng.as_mut() {
            // Choose a random offset.
            let max_offset = TLE_SIZE - (size % TLE_SIZE);
            let uniform_range = Uniform::new(0, max_offset / alignment);

            uniform_range.sample(rng) * alignment
        } else {
            0
        };

        base.add(offset)..base.add(offset + size)
    }
}
