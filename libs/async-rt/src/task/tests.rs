// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::{Builder, PollResult};
use crate::tests::NopScheduler;

#[test]
fn taskref_poll() {
    let builder = Builder::new(NopScheduler);

    let task = builder.build(async {});
    assert!(!task.is_complete(), "newly created task can't be complete");

    let res = task.poll();
    assert!(matches!(res, PollResult::Ready));
}

#[cfg(loom)]
mod loom {
    use super::NopScheduler;
    use crate::loom::alloc::{Track, TrackFuture};
    use crate::task;

    #[test]
    fn taskref_deallocates() {
        loom::model(|| {
            let track = Track::new(());

            let task = task::Builder::new(NopScheduler).build(async move {
                drop(track);
            });

            // if the task is not deallocated by dropping the `TaskRef`, the
            // `Track` will be leaked.
            drop(task);
        });
    }

    #[test]
    #[should_panic]
    // Miri (correctly) detects a memory leak in this test and fails, which is
    // good...but Miri doesn't understand "should panic".
    #[cfg_attr(miri, ignore)]
    fn do_leaks_work() {
        loom::model(|| {
            let track = Track::new(());
            std::mem::forget(track);
        });
    }
}
