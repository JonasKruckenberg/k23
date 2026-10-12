use cordyceps::{Linked, MpscQueue, mpsc_queue};
use maitake_sync::WaitCell;

use crate::loom::sync::atomic::AtomicBool;

pub enum TryDequeueError {
    Empty,
    Inconsistent,
}

pub struct Channel<T: Linked<mpsc_queue::Links<T>>> {
    queue: MpscQueue<T>,
    has_consumer: AtomicBool,
    rx_notify: WaitCell,
}

impl<T: Linked<mpsc_queue::Links<T>>> Channel<T> {
    pub fn new_with_stub(stub: T::Handle) -> Self {
        Self {
            queue: MpscQueue::new_with_stub(stub),
            has_consumer: AtomicBool::new(false),
            rx_notify: WaitCell::new(),
        }
    }

    #[cfg(not(loom))]
    pub const unsafe fn new_with_static_stub(stub: &'static T) -> Self {
        Self {
            // Safety: the caller promises `stub` is exclusive to this channel and immortal.
            queue: unsafe { MpscQueue::new_with_static_stub(stub) },
            has_consumer: AtomicBool::new(false),
            rx_notify: WaitCell::new(),
        }
    }

    pub fn send(&self, element: T::Handle) {
        self.queue.enqueue(element);

        self.rx_notify.wake();
    }

    pub fn receiver(&self) -> Receiver<'_, T> {
        self.try_receiver().expect("receiver already exists")
    }

    pub fn try_receiver(&self) -> Option<Receiver<'_, T>> {
        self.queue.try_consume().map(|inner| Receiver {
            inner,
            rx_notify: &self.rx_notify,
        })
    }
}

pub struct Receiver<'q, T: Linked<mpsc_queue::Links<T>>> {
    inner: mpsc_queue::Consumer<'q, T>,
    rx_notify: &'q WaitCell,
}

impl<T: Linked<mpsc_queue::Links<T>> + Send> Receiver<'_, T> {
    pub async fn recv(&mut self) -> T::Handle {
        self.rx_notify
            .wait_for_value(|| self.try_recv().ok())
            .await
            .unwrap()
    }

    pub fn try_recv(&mut self) -> Result<T::Handle, TryDequeueError> {
        self.inner.try_dequeue().map_err(|err| match err {
            mpsc_queue::TryDequeueError::Inconsistent => TryDequeueError::Inconsistent,
            mpsc_queue::TryDequeueError::Empty => TryDequeueError::Empty,
            mpsc_queue::TryDequeueError::Busy => unreachable!(),
        })
    }
}

#[cfg(test)]
mod tests {
    use core::ptr::NonNull;

    use super::*;

    struct Msg {
        links: mpsc_queue::Links<Self>,
        value: u32,
    }

    impl Msg {
        fn new(value: u32) -> Box<Self> {
            Box::new(Self {
                links: mpsc_queue::Links::new(),
                value,
            })
        }
    }

    // Safety: `links` is a live field of every message, handles own their message through a
    // `Box`, and nothing hands out a reference to a queued message.
    unsafe impl Linked<mpsc_queue::Links<Self>> for Msg {
        type Handle = Box<Self>;

        fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
            NonNull::new(Box::into_raw(handle)).unwrap()
        }

        unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
            // Safety: every pointer in a queue came from `into_ptr` above.
            unsafe { Box::from_raw(ptr.as_ptr()) }
        }

        unsafe fn links(target: NonNull<Self>) -> NonNull<mpsc_queue::Links<Self>> {
            // Safety: `target` points at a live message, so the projection is in bounds.
            unsafe { NonNull::new_unchecked(&raw mut (*target.as_ptr()).links) }
        }
    }

    #[test]
    #[cfg(loom)]
    fn two_senders() {
        use crate::loom;

        loom::model(|| {
            loom::lazy_static! {
                static ref CHANNEL: Channel<Msg> = Channel::new_with_stub(Msg::new(0));
            }

            let mut receiver = CHANNEL.receiver();

            let sender = loom::thread::spawn(move || CHANNEL.send(Msg::new(2)));
            CHANNEL.send(Msg::new(1));

            // Both sends must arrive, and the wake elision in `send` must not lose one.
            let mut sum = 0;
            loom::future::block_on(async {
                for _ in 0..2 {
                    sum += receiver.recv().await.value;
                }
            });
            assert_eq!(sum, 3);

            sender.join().unwrap();
        });
    }
}
