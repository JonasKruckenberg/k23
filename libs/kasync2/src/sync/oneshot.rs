// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::cell::UnsafeCell;
use crate::sync::WaitCell;
use alloc::sync::Arc;
use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        value: UnsafeCell::new(None),
        rx_waker: WaitCell::new(),
    });

    let tx = Sender {
        inner: Some(inner.clone()),
    };
    let rx = Receiver { inner };

    (tx, rx)
}

#[derive(Debug)]
pub struct Sender<T> {
    inner: Option<Arc<Inner<T>>>,
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

#[derive(Debug)]
struct Inner<T> {
    value: UnsafeCell<Option<T>>,
    rx_waker: WaitCell,
}

// Safety: TODO
unsafe impl<T: Send> Send for Inner<T> {}
// Safety: TODO
unsafe impl<T: Send> Sync for Inner<T> {}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct RecvError(pub(super) ());

// === impl Sender ===

impl<T: fmt::Debug> Sender<T> {
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn is_closed(&self) -> bool {
        let inner = self.inner.as_ref().unwrap();
        inner.rx_waker.is_closed()
    }

    #[tracing::instrument]
    pub fn send(mut self, value: T) -> Result<(), T> {
        let inner = self.inner.take().unwrap();

        if inner.rx_waker.is_closed() {
            return Err(value);
        }

        inner.value.with_mut(|ptr| {
            // Safety: The receiver will not access the `UnsafeCell` until
            // we call .wake() on the wake cell below.
            unsafe {
                *ptr = Some(value);
            }
        });

        inner.rx_waker.wake();

        Ok(())
    }
}

// === impl Receiver ===

impl<T: fmt::Debug> Receiver<T> {
    pub fn close(&mut self) {
        self.inner.as_ref().rx_waker.close();
    }

    #[tracing::instrument]
    pub fn poll_recv(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
        let inner = &self.inner;

        let res = inner.rx_waker.poll_wait(cx).map_err(|_| RecvError(()));
        tracing::trace!(?res);
        ready!(res)?;

        let value = self.inner.value.with_mut(|ptr| {
            // Safety: the WakeCell::poll_wait call returning Poll::Ready means that the Sender
            // wrote to the value field and signalled us to wake up
            unsafe { (*ptr).take().unwrap() }
        });

        Poll::Ready(Ok(value))
    }
}

impl<T: fmt::Debug> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_recv(cx)
    }
}
