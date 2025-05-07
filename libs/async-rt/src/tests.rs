// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::{Schedule, TaskRef};

#[derive(Copy, Clone, Debug)]
pub(crate) struct NopScheduler;

impl Schedule for NopScheduler {
    fn schedule(&self, task: TaskRef) {
        unimplemented!("nop scheduler tried to schedule task {:?}", task);
    }
}