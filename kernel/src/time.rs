// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use async_kit::time::{Clock, Timer};
use spin::OnceLock;

static TIMER: OnceLock<Timer> = OnceLock::new();

pub fn init(clock: Clock) {
    let t = TIMER.get_or_init(|| Timer::new(clock));
    async_kit::time::set_global_timer(t).unwrap();
}

pub fn global_timer() -> &'static Timer {
    TIMER.get().unwrap()
}
