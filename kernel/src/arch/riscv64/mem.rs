// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;

// #[cold]
// pub fn init() {
//     let root_pgtable = get_active_pgtable(DEFAULT_ASID);
//
//     // Zero out the lower half of the kernel address space to remove e.g. the leftover loader identity mappings
//     // Safety: `get_active_pgtable` & `VirtualAddress::from_phys` do minimal checking that the address is valid
//     // but otherwise we have to trust the address is valid for the entire page.
//     unsafe {
//         slice::from_raw_parts_mut(phys_to_virt(root_pgtable).as_mut_ptr(), PAGE_SIZE / 2).fill(0);
//     }
//
//     wmb();
// }
