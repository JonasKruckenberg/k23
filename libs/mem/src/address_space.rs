mod batch;

use core::alloc::{AllocError, Layout};
use core::iter;
use core::ops::Bound;
use core::range::Range;

use kmem_core::{
    Arch, FrameAllocator, HardwareAddressSpace, MemoryAttributes, PhysMap, VirtualAddress,
};
use rand_chacha::ChaCha20Rng;
use range_tree::{OverlapError, RangeTree};

use crate::address_space::batch::Batch;

pub struct AddressSpace<A: Arch> {
    hardware_address_space: HardwareAddressSpace<A>,
    regions: Regions,
    randomizer: kmem_aslr::Randomizer<A>,
    batch: Batch,
}

struct Regions {
    tree: RangeTree<VirtualAddress, AddressSpaceRegion>,
}

impl Regions {
    pub fn new() -> Result<Self, AllocError> {
        Ok(Self {
            tree: RangeTree::try_new()?,
        })
    }

    pub fn insert(
        &mut self,
        base: VirtualAddress,
        layout: Layout,
        attributes: MemoryAttributes,
    ) -> Result<(), OverlapError> {
        let range = base..=base.add(layout.size());
        let region = AddressSpaceRegion {
            attributes,
            #[cfg(debug_assertions)]
            layout,
        };

        self.tree.insert(range, region)
    }

    pub fn get_containing_mut(
        &mut self,
        address: VirtualAddress,
        layout: Layout,
    ) -> Option<(Range<VirtualAddress>, &mut AddressSpaceRegion)> {
        let mut iter = self.tree.iter_mut_from(Bound::Included(address));

        let (range, region) = iter.next()?;

        debug_assert!(range.contains(&address));
        #[cfg(debug_assertions)]
        assert_eq!(region.layout, layout);

        Some((range, region))
    }

    pub fn remove(
        &mut self,
        base: VirtualAddress,
        layout: Layout,
    ) -> Option<(Range<VirtualAddress>, AddressSpaceRegion)> {
        let range = Range {
            start: base,
            end: base.add(layout.size()),
        };

        let region = self.tree.remove(range.end)?;

        #[cfg(debug_assertions)]
        assert_eq!(region.layout, layout);

        Some((range, region))
    }
}

pub struct AddressSpaceRegion {
    attributes: MemoryAttributes,

    /// The layout this region was allocated with.
    #[cfg(debug_assertions)]
    layout: Layout,
}

impl<A: Arch> AddressSpace<A> {
    pub fn new(
        hardware_address_space: HardwareAddressSpace<A>,
        rng: Option<ChaCha20Rng>,
    ) -> Result<Self, AllocError> {
        Ok(Self {
            hardware_address_space,
            regions: Regions::new()?,
            randomizer: kmem_aslr::Randomizer::new(rng),
            batch: Batch::new(),
        })
    }

    /// Attempts to reserve a region of virtual memory.
    ///
    /// On success, returns a [`NonNull<[u8]>`][NonNull] meeting the size and alignment guarantees
    /// of `layout`. Access to this region must obey the provided `rules` or cause a hardware fault.
    ///
    /// The returned region may have a larger size than specified by `layout.size()`, and may or may
    /// not have its contents initialized.
    ///
    /// The returned region of virtual memory remains mapped as long as it is [*currently mapped*]
    /// and the address space type itself has not been dropped.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or mapping otherwise fails.
    pub fn map(
        &mut self,
        layout: Layout,
        attributes: MemoryAttributes,
        _physmap: &PhysMap,
        _frame_allocator: impl FrameAllocator,
    ) -> crate::Result<()> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::map]");

        let base = self.find_spot_for(layout).unwrap();

        self.regions.insert(base, layout, attributes).unwrap();

        //  - OPTIONAL (TODO figure out heuristic) commit a few pages

        Ok(())
    }

    /// Unmaps the virtual memory region referenced by `ptr`.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    pub unsafe fn unmap(
        &mut self,
        address: VirtualAddress,
        layout: Layout,
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
    ) {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::unmap]");

        let (range, region) = self.regions.remove(address, layout).unwrap();

        todo!();

        self.batch
            .flush_changes(&mut self.hardware_address_space, physmap, frame_allocator)
            .unwrap();
    }

    /// Updates the attributes for the virtual memory region referenced by `ptr`.
    ///
    /// After this returns, access to this region must obey the new `rules` or cause a hardware fault.
    /// If this returns `Ok`, access to this region must obey the new `rules` or cause a hardware fault.
    /// If this method returns `Err`, the access rules of the memory region are unaltered.
    ///
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    pub unsafe fn set_attributes(
        &mut self,
        address: VirtualAddress,
        layout: Layout,
        attributes: MemoryAttributes,
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
    ) {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::set_attributes]");

        let (range, region) = self.regions.get_containing_mut(address, layout).unwrap();

        self.batch.set_memory_attributes(range, attributes);

        todo!();

        // region.set_attributes(&mut self.batch, attributes);

        self.batch
            .flush_changes(&mut self.hardware_address_space, physmap, frame_allocator)
            .unwrap();

        todo!()
    }

    pub fn assert_valid(&self, _ctx: &str) {
        self.regions.assert_valid();
    }

    /// Find a spot in the address space that satisfies the given `layout` requirements.
    ///
    /// If a spot suitable for holding data described by `layout` is found, the base address of the
    /// address range is returned in `Some`. The returned address is already correct aligned to
    /// `layout.align()`.
    ///
    /// Returns `None` if no suitable spot was found. This *does not* mean there are no more gaps in
    /// the address space just that the *combination* of `layout.size()` and `layout.align()` cannot
    /// be satisfied *at the moment*. Calls to this method will a different size, alignment, or at a
    /// different time might still succeed.
    fn find_spot_for(&mut self, layout: Layout) -> Option<VirtualAddress> {
        if self.regions.is_empty() {
            self.randomizer
                .find_spot_in(layout, iter::once(VirtualAddress::MIN..VirtualAddress::MAX))
        } else {
            let gaps = self.regions.gaps().map(|(start, end)| {
                let start = match start {
                    Bound::Included(b) => b,
                    Bound::Excluded(b) => b.add(1),
                    Bound::Unbounded => VirtualAddress::MIN,
                };
                let end = match end {
                    Bound::Included(b) => b.sub(1),
                    Bound::Excluded(b) => b,
                    Bound::Unbounded => VirtualAddress::MAX,
                };
                start..end
            });

            self.randomizer.find_spot_in(layout, gaps)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use kmem_core::test_utils::{Machine, MachineBuilder};
    use rand::distr::Uniform;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha20Rng;

    use super::*;

    /// Hardcoded seed for the ASLR CPRNG to remove the reliance on high-quality entropy for tests
    /// (because that can be an issue in CI runners).
    /// THIS MUST ONLY EVER BE USED FOR TESTS AND NEVER NEVER NEVER FOR PRODUCTION CODE
    const ASLR_SEED: [u8; 32] = [
        232, 66, 52, 206, 40, 195, 141, 166, 130, 237, 114, 177, 190, 54, 88, 88, 30, 196, 41, 165,
        54, 85, 157, 181, 124, 91, 106, 9, 179, 48, 75, 245,
    ];

    kmem_core::for_every_arch!(A => {
        extern crate std;

        // ===== find a spot *before* the regions =====
        #[test_log::test]
        fn find_spot_for_no_aslr_before_regions() {
            let machine: Machine<A> = MachineBuilder::new().with_memory_regions([0x5000]).finish();

            let (hardware_address_space, _, _) = machine.bootstrap_address_space(A::DEFAULT_PHYSMAP_BASE);

            let mut aspace = AddressSpace::new(hardware_address_space, None).unwrap();

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::new(4 * A::GRANULE_SIZE),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::new(8 * A::GRANULE_SIZE),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            // we know the following: NO gap to the left, ONE page gap between the regions and A BIG gap after
            let spot = aspace
                .find_spot_for(Layout::from_size_align(A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap())
                .unwrap();

            assert_eq!(spot, VirtualAddress::new(0));
        }

        // ===== find a spot *between* the regions =====
        #[test_log::test]
        fn find_spot_for_no_aslr_between_regions() {
            let machine: Machine<A> = MachineBuilder::new().with_memory_regions([0x5000]).finish();

            let (hardware_address_space, _, _) = machine.bootstrap_address_space(A::DEFAULT_PHYSMAP_BASE);

            let mut aspace = AddressSpace::new(hardware_address_space, None).unwrap();

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::new(0),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::MAX.sub(5 * A::GRANULE_SIZE),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            // we know the following: NO gap to the left, ONE page gap between the regions and A BIG gap after
            let spot = aspace
                .find_spot_for(Layout::from_size_align(A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap())
                .unwrap();

            assert_eq!(spot, VirtualAddress::new(4 * A::GRANULE_SIZE));
        }

        // ===== find a spot *after* the regions =====
        #[test_log::test]
        fn find_spot_for_no_aslr_after_regions() {
            let machine: Machine<A> = MachineBuilder::new().with_memory_regions([0x5000]).finish();

            let (hardware_address_space, _, _) = machine.bootstrap_address_space(A::DEFAULT_PHYSMAP_BASE);

            let mut aspace = AddressSpace::new(hardware_address_space, None).unwrap();

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::new(0),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                VirtualAddress::new(4 * A::GRANULE_SIZE),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
                Layout::from_size_align(4 * A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
            )));

            // we know the following: NO gap to the left, NO gap between the regions and A BIG gap after
            // we therefore expect even the smallest layout to be placed AFTER 9 * 4096
            let spot = aspace
                .find_spot_for(Layout::from_size_align(A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap())
                .unwrap();

            assert_eq!(spot, VirtualAddress::new(8 * A::GRANULE_SIZE));
        }

        #[test_log::test]
        fn find_spot_aslr_entropy() {
            let machine: Machine<A> = MachineBuilder::new().with_memory_regions([0x5000]).finish();

            let (hardware_address_space, _, _) = machine.bootstrap_address_space(A::DEFAULT_PHYSMAP_BASE);

            let layout = Layout::from_size_align(A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap();

            let mut aspace = AddressSpace::new(
                hardware_address_space,
                Some(ChaCha20Rng::from_seed(ASLR_SEED)),
            ).unwrap();

            // first we fill up the address space with 100 randomly placed regions just so
            // we are not just testing the entropy of the RNG
            for _ in 0..100 {
                let spot = aspace.find_spot_for(layout).unwrap();
                aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                    spot,
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                    layout,
                )));
            }

            let mut rng = ChaCha20Rng::from_os_rng();

            // then we sample the algorithm 500 times, adding the chosen spots to the test buffer
            let mut data = Vec::new();
            for _ in 0..5000 {
                let spot = aspace.find_spot_for(layout).expect("failed to find spot for layout {layout} in {aspace:?}");

                // because all spots are page-aligned we need to generate the lower page-offset-n bits of randomness for
                // our statistical test to pass
                let spot = spot.get() | rng.sample(Uniform::new(0, A::GRANULE_SIZE).unwrap());

                data.push(VirtualAddress::new(spot));
            }

            // finally we run the Frequency (Monobit) Test over the collected data so see if
            // we get proper distribution
            let (passed, pval) = frequency_test::<A>(&data);

            assert!(passed, "test returned P-value of {pval} (expected >= 0.01)");
        }
    });

    /// Implements the Frequency (Monobit) Test
    ///
    /// This function calculates the proportion of zero-bits and one-bits in `spots` expecting
    /// it to be approximately the same as for a truly random sequence.
    ///
    /// from NIST "STATISTICAL TEST SUITE FOR RANDOM AND PSEUDORANDOM NUMBER GENERATORS FOR CRYPTOGRAPHIC APPLICATIONS"
    /// (<https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-22r1a.pdf>)
    pub fn frequency_test<A: Arch>(spots: &[VirtualAddress]) -> (bool, f64) {
        const TEST_THRESHOLD: f64 = 0.01;

        let nbits = spots.len() * A::VIRTUAL_ADDRESS_BITS as usize;
        let ones: usize = spots
            .into_iter()
            .map(|spot| spot.get().count_ones() as usize)
            .sum();

        let sn = 2 * ones as isize - nbits as isize;
        let sobs = sn.abs() as f64 / (nbits as f64).sqrt();
        let p = libm::erfc(sobs / 2.0_f64.sqrt());

        (p >= TEST_THRESHOLD, p)
    }
}
