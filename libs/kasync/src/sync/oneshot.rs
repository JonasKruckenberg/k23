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
    /// Returns true if the associated [`Receiver`] handle has been closed.
    ///
    /// A [`Receiver`] is closed by either calling [`close`][Receiver::close] explicitly or #
    /// the [`Receiver`] value is dropped.
    /// If `true` is returned, a call to [`send`][Sender::send] will always result in an error.
    ///
    /// This method never blocks.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn is_closed(&self) -> bool {
        let inner = self.inner.as_ref().unwrap();
        inner.rx_waker.is_closed()
    }

    /// Attempts to send a value on this channel, returning it back if it could not be sent.
    ///
    /// This method consumes self as only one value may ever be sent on an `oneshot` channel. A successful
    /// send occurs when it is determined that the other end of the channel has not hung up already.
    /// An unsuccessful send would be one where the corresponding receiver has already been deallocated.
    /// Note that a return value of `Err` means that the data will never be received, but a return value
    /// of `Ok` does not mean that the data will be received. It is possible for the corresponding receiver
    /// to hang up immediately after this function returns `Ok`.
    ///
    /// This method never blocks.
    ///
    /// # Errors
    ///
    /// If the channel is closed and sending the value fails, it is returned in the `Err` variant.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
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
    /// Prevents the associated [`Sender`] handle from sending a value.
    ///
    /// Any `send` operation which happens after calling close is guaranteed to fail. After calling
    /// `close`, `[poll_recv`][Self::poll_recv] should be called to receive a value if one was sent
    /// before the call to close completed.
    ///
    /// This function is useful to perform a graceful shutdown and ensure that a value will not be
    /// sent into the channel and never received.
    ///
    /// `close` is no-op if a message is already received or the channel is already closed.
    ///
    /// This method never blocks.
    pub fn close(&mut self) {
        self.inner.as_ref().rx_waker.close();
    }

    /// Poll to wait on this `Receiver`, returning the sent value or registering the [`Waker`][core::task::Waker]
    /// from the provided [`Context`] when a value is sent.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
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
