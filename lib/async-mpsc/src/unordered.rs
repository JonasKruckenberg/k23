use cordyceps::{Linked, Stack, TransferStack, stack};
use maitake_sync::WaitCell;
use util::loom_const_fn;

use crate::loom::sync::atomic::{AtomicBool, Ordering};

pub struct Channel<T: Linked<stack::Links<T>>> {
    stack: TransferStack<T>,
    has_consumer: AtomicBool,
    rx_notify: WaitCell,
}

impl<T: Linked<stack::Links<T>>> Channel<T> {
    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                stack: TransferStack::new(),
                has_consumer: AtomicBool::new(false),
                rx_notify: WaitCell::new(),
            }
        }
    }

    /// Enqueue a new element at the end of the queue.
    pub fn send(&self, element: T::Handle) {
        if self.stack.push_was_empty(element) {
            self.rx_notify.wake();
        }
    }

    pub fn receiver(&self) -> Receiver<'_, T> {
        self.try_receiver().expect("receiver already exists")
    }

    pub fn try_receiver(&self) -> Option<Receiver<'_, T>> {
        if self
            .has_consumer
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }

        let stack = self.stack.take_all();

        Some(Receiver {
            channel: self,
            stack,
        })
    }
}

pub struct Receiver<'q, T: Linked<stack::Links<T>>> {
    channel: &'q Channel<T>,
    stack: Stack<T>,
}

impl<T: Linked<stack::Links<T>>> Receiver<'_, T> {
    pub async fn recv(&mut self) -> T::Handle {
        self.channel
            .rx_notify
            .wait_for_value(|| self.try_recv())
            .await
            .unwrap()
    }

    pub fn try_recv(&mut self) -> Option<T::Handle> {
        if let Some(el) = self.stack.pop() {
            Some(el)
        } else {
            self.stack = self.channel.stack.take_all();
            self.stack.pop()
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ptr::NonNull;

    use super::*;

    struct Msg {
        links: stack::Links<Self>,
        value: u32,
    }

    impl Msg {
        fn new(value: u32) -> Box<Self> {
            Box::new(Self {
                links: stack::Links::new(),
                value,
            })
        }
    }

    // Safety: `links` is a live field of every message, handles own their message through a
    // `Box`, and nothing hands out a reference to a queued message.
    unsafe impl Linked<stack::Links<Self>> for Msg {
        type Handle = Box<Self>;

        fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
            NonNull::new(Box::into_raw(handle)).unwrap()
        }

        unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
            // Safety: every pointer in a queue came from `into_ptr` above.
            unsafe { Box::from_raw(ptr.as_ptr()) }
        }

        unsafe fn links(target: NonNull<Self>) -> NonNull<stack::Links<Self>> {
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
                static ref CHANNEL: Channel<Msg> = Channel::new();
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
