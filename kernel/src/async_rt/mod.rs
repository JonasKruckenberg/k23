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
pub use yield_now::yield_now;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub struct Runtime {
    /// Handle to the scheduler used by this runtime
    // If we ever want to support multiple runtimes, this should become an enum over the different
    // variants. For now, we only support the multithreaded scheduler.
    scheduler: scheduler::multi_thread::Handle,
}

/// Get a reference to the current async runtime.
pub fn current() -> &'static Runtime {
    RUNTIME.get().expect("async runtime not initialized")
}

/// Initialize the global async runtime.
///
/// This will allocate required state for `num_cores` of harts. Tasks can immediately be spawned
/// using the returned runtime reference (a reference to the runtime can also be obtained using
/// `async_rt::current()`) but no tasks will run until at least one hart in the system enters its
/// runtime loop using `async_rt::run()`.
#[cold]
pub fn init(num_cores: usize, rng: &mut impl RngCore) -> &'static Runtime {
    #[allow(tail_expr_drop_order)]
    RUNTIME.get_or_init(|| Runtime {
        scheduler: scheduler::multi_thread::Handle::new(num_cores, rng),
    })
}

/// Run the async runtime loop on the calling hart.
///
/// This function will not return until the runtime is shut down.
#[cold]
pub fn run(rt: &'static Runtime, hartid: usize) -> Result<(), ()> {
    scheduler::multi_thread::worker::run(&rt.scheduler, hartid)
}

impl Runtime {
    /// Spawns a future onto the async runtime.
    ///
    /// The returned [`JoinHandle`] can be used to await the result of the future or cancel it.
    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.scheduler.spawn(future)
    }
}
