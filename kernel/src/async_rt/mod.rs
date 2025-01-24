// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod queue;
mod scheduler;
mod task;
mod yield_now;

use core::future::Future;
use rand::RngCore;
use sync::OnceLock;
pub use task::JoinHandle;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub struct Runtime {
    scheduler: scheduler::multi_thread::Handle,
}

#[cold]
pub fn init(num_cores: usize, rng: &mut impl RngCore) -> &'static Runtime {
    #[allow(tail_expr_drop_order)]
    RUNTIME.get_or_init(|| Runtime {
        scheduler: scheduler::multi_thread::Handle::new(num_cores, rng),
    })
}

pub fn current() -> &'static Runtime {
    RUNTIME.get().expect("async runtime not initialized")
}

impl Runtime {
    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.scheduler.spawn(future)
    }
}

