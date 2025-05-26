// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod parker;
mod parking_lot;

use crate::time::{Clock, Deadline};
pub use parker::{Parker, UnparkToken};
pub use parking_lot::ParkingLot;

pub trait Park {
    fn park(&self);
    fn park_until(&self, deadline: Deadline, clock: &Clock);
    fn unpark(&self);
}
