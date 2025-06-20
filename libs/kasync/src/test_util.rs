// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::executor::Executor;
use crate::park::Park;

#[must_use]
pub struct StopOnPanic<'e, P: Park + Send + Sync> {
    exec: &'e Executor<P>,
}
impl<'e, P: Park + Send + Sync> StopOnPanic<'e, P> {
    pub fn new(exec: &'e Executor<P>) -> Self {
        Self { exec }
    }
}
impl<'e, P: Park + Send + Sync> Drop for StopOnPanic<'e, P> {
    fn drop(&mut self) {
        self.exec.stop();
    }
}
