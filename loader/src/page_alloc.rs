use core::marker::PhantomData;
use core::ops::Range;
use pmm::VirtualAddress;
use rand::distributions::{Distribution, Uniform};
use rand::prelude::IteratorRandom;
use rand_chacha::ChaCha20Rng;

/// Virtual memory allocator for setting up initial mappings.
///
/// All regions will be huge page (1GiB) aligned.
#[derive(Debug)]
pub struct PageAllocator<A>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    /// Whether a top-level page is in use.
    page_state: [bool; A::PAGE_TABLE_ENTRIES / 2],
    /// A random number generator that should be used to generate random addresses or
    /// `None` if aslr is disabled.
    rng: Option<ChaCha20Rng>,
    _m: PhantomData<A>,
}

impl<A> PageAllocator<A>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    /// Create a new `PageAllocator` with KASLR enabled.
    ///
    /// This means regions will be randomly placed in the higher half of the address space.
    pub const fn new(rng: ChaCha20Rng) -> Self {
        Self {
            page_state: [false; A::PAGE_TABLE_ENTRIES / 2],
            rng: Some(rng),
            _m: PhantomData,
        }
    }

    /// Create a new `PageAllocator` with KASLR **disabled**.
    ///
    /// Allocated regions will be placed consecutively in the higher half of the address space.
    pub const fn new_no_kaslr() -> Self {
        Self {
            page_state: [false; A::PAGE_TABLE_ENTRIES / 2],
            rng: None,
            _m: PhantomData,
        }
    }

    pub fn reserve_pages(&mut self, num_pages: usize) -> usize {
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

        let maybe_idx = if let Some(rng) = self.rng.as_mut() {
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

    pub fn reserve_range(&mut self, size: usize, alignment: usize) -> Range<VirtualAddress> {
        assert!(alignment.is_power_of_two());

        let top_level_page_size: usize =
            A::PAGE_SIZE << (A::PAGE_ENTRY_SHIFT * (A::PAGE_TABLE_LEVELS - 1));

        // how many top-level pages are needed to map `size` bytes
        // and attempt to allocate them
        let page_idx = self.reserve_pages(size.div_ceil(top_level_page_size));

        // calculate the base address of the page
        //
        // we know that entry_idx is between 0 and PAGE_TABLE_ENTRIES / 2
        // and represents a top-level page in the *higher half* of the address space.
        //
        // we can then take the lowest possible address of the higher half (`usize::MAX << VA_BITS`)
        // and add the `idx` multiple of the size of a top-level entry to it
        let base = VirtualAddress::new((usize::MAX << A::VA_BITS) + page_idx * top_level_page_size);

        let offset = if let Some(rng) = self.rng.as_mut() {
            // Choose a random offset.
            let max_offset = top_level_page_size - (size % top_level_page_size);
            
            if max_offset / alignment != 0 {
                let uniform_range = Uniform::new(0, max_offset / alignment);
    
                uniform_range.sample(rng) * alignment
            } else {
                0
            }
        } else {
            0
        };

        base.add(offset)..base.add(offset + size)
    }
}
