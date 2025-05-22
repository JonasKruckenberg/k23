// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicU64, Ordering};
use core::fmt;

/// An opaque ID that uniquely identifies a task relative to all other currently
/// running tasks.
///
/// # Notes
///
/// - Task IDs are unique relative to other *currently running* tasks. When a
///   task completes, the same ID may be used for another task.
/// - Task IDs are *not* sequential, and do not indicate the order in which
///   tasks are spawned, what runtime a task is spawned on, or any other data.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct Id(u64);

impl Id {
    pub const fn stub() -> Self {
        Self(0)
    }

    pub(crate) fn next() -> Self {
        #[cfg(loom)]
        crate::loom::lazy_static! {
            static ref NEXT_ID: AtomicU64 = AtomicU64::new(1);
        }
        #[cfg(not(loom))]
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Self(id)
    }

    pub(crate) fn as_u64(self) -> u64 {
        self.0
    }

    pub fn is_stub(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
