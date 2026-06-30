// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use mem_core::{MemoryAttributes, WriteOrExecute};
use mem_testkit::proptest::attrs as any_attrs;
use proptest::prelude::*;

proptest! {
    /// `is_read_only` must be `true` exactly when reading is permitted and the
    /// `WRITE_OR_EXECUTE` field is `Neither` — it must not ignore that field.
    #[test]
    fn is_read_only_iff_read_and_not_write_or_execute(attrs in any_attrs()) {
        let expected = attrs.allows_read()
            && matches!(
                attrs.get(MemoryAttributes::WRITE_OR_EXECUTE),
                WriteOrExecute::Neither,
            );

        prop_assert_eq!(attrs.is_read_only(), expected);
    }
}
