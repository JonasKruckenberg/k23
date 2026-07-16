// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Exercises the panic/unwind runtime end-to-end: the real `#[panic_handler]`,
//! unwinder, and per-CPU accounting are all live under the in-kernel harness.

use core::cell::Cell;
use core::panic::AssertUnwindSafe;

/// A `panic!` whose unwind runs a `Drop` that itself catches an inner `panic!`
/// must deliver the inner panic to the drop's `catch_unwind` and still deliver
/// the outer panic to the outer one — nesting is strictly LIFO.
#[test::test]
async fn nested_catch_during_drop() {
    struct Guard<'a>(&'a Cell<bool>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let inner = panic_unwind::catch_unwind(|| panic!("inner"));
            self.0.set(inner.is_err());
        }
    }

    let inner_caught = Cell::new(false);
    let outer = panic_unwind::catch_unwind(AssertUnwindSafe(|| {
        let _guard = Guard(&inner_caught);
        panic!("outer");
    }));

    assert!(inner_caught.get(), "inner panic was not caught during drop");
    assert!(outer.is_err(), "outer panic was not delivered");
}

/// A panic caught with `catch_unwind` and then re-raised with `resume_unwind`
/// must be caught again at an outer frame.
#[test::test]
async fn resume_reraises_to_outer_catch() {
    let outer = panic_unwind::catch_unwind(|| {
        let inner = panic_unwind::catch_unwind(|| panic!("caught then resumed"));
        assert!(inner.is_err(), "inner panic was not caught");
        panic_unwind::resume_unwind();
    });

    assert!(
        outer.is_err(),
        "resumed unwind was not caught at the outer frame"
    );
}
