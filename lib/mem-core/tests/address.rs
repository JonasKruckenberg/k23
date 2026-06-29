// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use mem_core::VirtualAddress;
use mem_core::arch::riscv64::Riscv64Sv39;
use proptest::{prop_assert, prop_assert_eq, prop_assert_ne, proptest};

proptest! {
    #[test]
    #[cfg_attr(miri, ignore)]
    fn lower_half_is_canonical(addr in 0x0usize..0x3fffffffff) {
        let addr = VirtualAddress::new(addr);
        prop_assert!(addr.is_canonical::<Riscv64Sv39>());
        prop_assert_eq!(addr.canonicalize::<Riscv64Sv39>(), addr);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn upper_half_is_canonical(addr in 0xffffffc000000000usize..0xffffffffffffffff) {
        let addr = VirtualAddress::new(addr);
        prop_assert!(addr.is_canonical::<Riscv64Sv39>());
        prop_assert_eq!(addr.canonicalize::<Riscv64Sv39>(), addr);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn non_canonical_hole(addr in 0x4000000000usize..0xffffffbfffffffff) {
        let addr = VirtualAddress::new(addr);
        prop_assert_ne!(addr.canonicalize::<Riscv64Sv39>(), addr);
        prop_assert!(!addr.is_canonical::<Riscv64Sv39>());
    }
}
