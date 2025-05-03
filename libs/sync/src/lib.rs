#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod error;
mod wait_cell;
mod wait_queue;
mod wake_batch;

pub use error::Closed;
pub use wait_cell::{PollWaitError, Wait, WaitCell};
pub use wait_queue::WaitQueue;
pub use wake_batch::WakeBatch;
