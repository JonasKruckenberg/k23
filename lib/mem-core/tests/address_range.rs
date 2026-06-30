// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use mem_core::{AddressRangeExt, VirtualAddress};
use mem_testkit::proptest::any_virt;

proptest::proptest! {
    #[test]
    #[cfg_attr(miri, ignore)]
    fn len(len: usize) {
        let r: Range<VirtualAddress> = Range::from_start_len(VirtualAddress::new(0), len);

        proptest::prop_assert_eq!(len, AddressRangeExt::len(&r))
    }

    /// `len` must be total: an empty or inverted range (`start >= end`) has
    /// length zero and must never panic via `offset_from_unsigned`.
    #[test]
    fn len_is_total(start in any_virt(), end in any_virt()) {
        let r = Range::from(start..end);

        if start >= end {
            proptest::prop_assert_eq!(AddressRangeExt::len(&r), 0);
        } else {
            proptest::prop_assert_eq!(AddressRangeExt::len(&r), end.get() - start.get());
        }
    }
}
