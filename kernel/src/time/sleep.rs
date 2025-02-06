// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::device::cpu::with_cpu;
use crate::scheduler;
use crate::scheduler::scheduler;
use crate::time::clock::Ticks;
use crate::time::timer::Timer;
use crate::time::Instant;
use crate::util::atomic_waker::AtomicWaker;
use core::fmt;
use core::future::Future;
use core::marker::PhantomPinned;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use core::time::Duration;
use pin_project::{pin_project, pinned_drop};

pub fn sleep(duration: Duration) -> Sleep<'static> {
    let timer = scheduler().cpu_local_timer();
    let ticks = with_cpu(|cpu| cpu.clock.duration_to_ticks(duration).unwrap());

    Sleep::new(timer, ticks)
}

pub fn sleep_until(instant: Instant) -> Sleep<'static> {
    let timer = scheduler().cpu_local_timer();
    let now = with_cpu(|cpu| cpu.clock.now());
    let duration = instant.duration_since(now);
    let ticks = with_cpu(|cpu| cpu.clock.duration_to_ticks(duration).unwrap());

    Sleep::new(timer, ticks)
}

#[pin_project(PinnedDrop)]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Sleep<'t> {
    state: State,
    timer: &'t Timer,
    #[pin]
    entry: Entry,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum State {
    Unregistered,
    Registered,
}

#[derive(Debug)]
pub struct Entry {
    pub(crate) deadline: Ticks,
    ticks: Ticks,
    pub(super) links: linked_list::Links<Self>,
    /// The currently-registered waker
    waker: AtomicWaker,
    _pin: PhantomPinned,
}

impl<'t> Sleep<'t> {
    pub fn new(timer: &'t Timer, ticks: Ticks) -> Self {
        let now = with_cpu(|cpu| cpu.clock.now_ticks());
        let deadline = Ticks(now.0 + ticks.0);

        Self {
            state: State::Unregistered,
            timer,
            entry: Entry {
                links: linked_list::Links::new(),
                waker: AtomicWaker::new(),
                deadline,
                ticks,
                _pin: PhantomPinned,
            },
        }
    }

    /// Returns the [`Duration`] that this `Sleep` future will sleep for.
    pub fn duration(&self) -> Duration {
        with_cpu(|cpu| cpu.clock.ticks_to_duration(self.entry.ticks))
    }
}

impl Future for Sleep<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        log::trace!("Sleep::poll {self:?}");
        let mut this = self.as_mut().project();

        match this.state {
            State::Unregistered => {
                let mut lock = this.timer.core.lock();

                // While we are holding the wheel lock, go ahead and advance the
                // timer, too. This way, the timer wheel gets advanced more
                // frequently than just when a scheduler tick completes or a
                // timer IRQ fires, helping to increase timer accuracy.
                this.timer.turn_locked(&mut lock);

                // Safety: the timer impl promises to treat the pointer as pinned
                let ptr = unsafe { NonNull::from(Pin::into_inner_unchecked(this.entry.as_mut())) };

                *this.state = State::Registered;
                // Safety: we just created the pointer from a mutable reference
                if unsafe { lock.register(ptr) } == Poll::Ready(()) {
                    return Poll::Ready(());
                }
            }
            State::Registered if this.entry.links.is_linked() => {}
            _ => return Poll::Ready(()),
        }

        // current().defer(cx.waker());
        this.entry.waker.register_by_ref(cx.waker());

        Poll::Pending
    }
}

#[pinned_drop]
impl PinnedDrop for Sleep<'_> {
    fn drop(mut self: Pin<&mut Self>) {
        log::trace!("Sleep::drop");
        let this = self.project();
        // we only need to remove the sleep from the timer wheel if it's
        // currently part of a linked list --- if the future hasn't been polled
        // yet, or it has already completed, we don't need to lock the timer to
        // remove it.
        if this.entry.links.is_linked() {
            let mut lock = this.timer.core.lock();
            lock.cancel(this.entry);
        }
    }
}

impl fmt::Debug for Sleep<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            state,
            entry,
            timer,
        } = self;
        f.debug_struct("Sleep")
            .field("duration", &self.duration())
            .field("state", &state)
            .field_with("addr", |f| fmt::Pointer::fmt(&entry, f))
            .field_with("timer", |f| fmt::Pointer::fmt(timer, f))
            .finish()
    }
}

impl Entry {
    pub(super) fn fire(&self) {
        log::trace!("firing sleep {self:p}");
        self.waker.wake();
    }
}

// Safety: TODO
unsafe impl linked_list::Linked for Entry {
    type Handle = NonNull<Entry>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}
