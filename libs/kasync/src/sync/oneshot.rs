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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{Executor, Worker};
    use crate::loom;
    use crate::loom::sync::atomic::{AtomicUsize, Ordering};
    use crate::test_util::{StdPark, std_clock};
    use fastrand::FastRand;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn oneshot_ping_pong() {
        #[cfg(loom)] // loom can do *fewer* pings, but in like a cutesy way
        const NUM_PINGS: usize = 7;
        #[cfg(not(loom))]
        const NUM_PINGS: usize = 10_000;

        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_ids(true)
            .set_default();

        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
            }

            let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
            let rem = Arc::new(AtomicUsize::new(NUM_PINGS));

            let h = EXEC
                .try_spawn(async {
                    for _ in 0..NUM_PINGS {
                        let rem = rem.clone();

                        let (tx1, rx1) = channel();
                        let (tx2, rx2) = channel();

                        EXEC.try_spawn(async {
                            rx1.await.unwrap();
                            tx2.send(()).unwrap();
                        })
                        .unwrap();

                        tx1.send(()).unwrap();
                        rx2.await.unwrap();

                        if 1 == rem.fetch_sub(1, Ordering::Relaxed) {
                            tracing::info!("done!");
                        }
                    }
                })
                .unwrap();

            worker.block_on(h).unwrap();
        });
    }
}
