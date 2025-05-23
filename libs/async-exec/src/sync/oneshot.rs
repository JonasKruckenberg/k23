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

unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct RecvError(pub(super) ());

// === impl Sender ===

impl<T: fmt::Debug> Sender<T> {
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

        inner.value.with_mut(|ptr| unsafe {
            *ptr = Some(value);
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

        if let Some(value) = self.take_value() {
            return Poll::Ready(Ok(value));
        }

        let res = inner.rx_waker.poll_wait(cx).map_err(|_| RecvError(()));
        tracing::trace!(?res);
        ready!(res)?;

        let value = self.take_value().unwrap();

        Poll::Ready(Ok(value))
    }

    fn take_value(&self) -> Option<T> {
        self.inner.value.with_mut(|ptr| unsafe { (*ptr).take() })
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
    use crate::park::StdPark;
    use fastrand::FastRand;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;
    // use tracing_subscriber::fmt::format::FmtSpan;
    // use tracing_subscriber::util::SubscriberInitExt;
    // use tracing_subscriber::EnvFilter;

    loom::lazy_static! {
        static ref EXEC: Executor<StdPark> = Executor::new(1);
    }

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
