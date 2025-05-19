// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod parker;
mod parking_lot;

use cfg_if::cfg_if;
pub use parker::{Parker, UnparkToken};
pub use parking_lot::ParkingLot;

pub trait Park {
    fn park(&self);
    fn unpark(&self);
}

cfg_if! {
    if #[cfg(test)] {
        struct StdPark(crate::loom::thread::Thread);

        impl Park for StdPark {
            fn park(&self) {
                crate::loom::thread::park();
            }

            fn unpark(&self) {
                self.0.unpark();
            }
        }

        impl StdPark {
            pub fn for_current() -> Self {
                Self(crate::loom::thread::current())
            }
        }
    }
}
