// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! # QSBR: Quiescent-state-based reclamation.
//!
//! QSBR can be used to implement efficient, and safe concurrent object
//! reclamation.
//!
//! It works by effectively delaying the reclamation of objects until no threads
//! hold references to it anymore. This has essentially 3 important pieces to
//! it:
//!
//! 1. There is a shared global  "epoch" counter (per [`Domain`]), whenever an
//!    object is scheduled to be reclaimed (by giving it to [`Local::retire`])
//!    it is stamped with this epoch number and put onto a reclaim queue. We
//!    then increment this epoch number [^1].
//! 2. Each participating threads maintains a local "current epoch" counter.
//!    Whenever it knows that **cannot hold references to QSBR protected data**
//!    (called a "quiescent state") it advances its local epoch to the global
//!    epoch.
//! 3. Each participating thread periodically calls [`Local::reclaim`] which
//!    will calculate the oldest epoch any thread is still in, and free any
//!    objects in the queue tagged with an older epoch.
//!
//! These three mechanisms are provably enough for safe memory reclamation:
//! Every object is tagged with the epoch it got freed in. If we see that all
//! threads have advances past this epoch, we know - because threads only
//! advance into a new epoch when they are absolutely sure no outstanding
//! references can exist - that this object is safe to reclaim.
//!
//! ## How do I know no outstanding references exits?
//!
//! Knowing when no outstanding references can exist might sound difficult to
//! impossible to ensure. But notice that most programs in practice fall into
//! roughly loop-shaped architectures: A web server can be thought of as loop
//! over incoming requests. Every loop iteration it runs arbitrary, probably
//! quite complex, logic to handle the request, respond to the request, then
//! loop again. Similarly, a GUI application also has a main loop running at its
//! core responding to incoming events.
//!
//! If you structure your code right, every loop iteration is a natural
//! quiescent state. In a web server you very likely don't want to keep state
//! across requests anyway. `qsbr` makes structuring this easy: every access to
//! a QSBR protected object must obtain a [`Guard`] object first. Rusts lifetime
//! system then ensures you cannot stash away an unsafe reference at all.
//!
//! It also, and this is how `qsbr` is used in k23, means that you cannot hold a
//! read guard across an `.await` point which makes quiescence trivially easy in
//! an async executor: every iteration of the executors poll loop is
//! automatically a quiescent state. In k23 we use this to our advantage: the
//! async runtime drives the QSBR machinery automatically (calling
//! [`Local::quiescent`], [`Local::enter_idle`], [`Local::exit_idle`] and
//! [`Local::reclaim`] when its appropriate) so the rest of the kernel can
//! simply and safely use QSBR protected objects for lock-free data structures
//! without having to worry about anything.
//!
//! [^1]: in practice we batch objects together when retiring them to amortize epoch churn.

#![cfg_attr(not(test), no_std)]

mod atomic;
mod loom;

use core::cell::RefCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;

use cordyceps::{Stack, TransferStack, stack};
use util::CachePadded;

pub use crate::atomic::{Atomic, Shared};
use crate::loom::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};

// sentinel value for threads that are not currently participating.
// counting "real" epochs starts at 1.
const NO_EPOCH: u64 = 0;

/// A set of threads cooperating to reclaim shared objects.
pub struct Domain<const N: usize> {
    // The currently active epoch, a batch retired at this epoch is safe to reclaim
    // once every thread is in an epoch greater than this.
    global_epoch: AtomicU64,
    // What epoch each thread is in. `NO_EPOCH` means it is idle, and we can ignore it
    // (idle always means quiescent).
    epochs: [CachePadded<AtomicU64>; N],
    // how many threads have registered themselves. tracked so we dont to `N` scans every time
    // if only a few threads have registered themselves.
    watermark: AtomicUsize,
}

impl<const N: usize> Domain<N> {
    #[cfg(not(loom))]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            global_epoch: AtomicU64::new(1),
            epochs: [const { CachePadded(AtomicU64::new(NO_EPOCH)) }; N],
            watermark: AtomicUsize::new(0),
        }
    }

    #[cfg(loom)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            global_epoch: AtomicU64::new(1),
            epochs: core::array::from_fn(|_| CachePadded(AtomicU64::new(NO_EPOCH))),
            watermark: AtomicUsize::new(0),
        }
    }

    /// Opens a new epoch and returns the one that just closed.
    pub fn advance(&self) -> Epoch {
        // Release: orders this thread's retires before the epoch other threads will
        // acknowledge on the strength of.
        Epoch(self.global_epoch.fetch_add(1, Ordering::Release))
    }

    /// Returns `true` if `epoch`'s grace period has elapsed, meaning every
    /// participating thread has passed a quiescent state since it closed.
    pub fn poll(&self, epoch: Epoch) -> bool {
        // Pairs with the SeqCst fence in `Local::exit_idle`: if an Acquire load
        // below misses a concurrent epoch store (reads NO_EPOCH), the fence
        // pairing guarantees that thread's subsequent loads see every write the
        // caller made before this call — in particular the unlinking of
        // anything already retired — so skipping it is safe.
        fence(Ordering::SeqCst);

        // Acquire: pairs with the Release `fetch_max` in `Local::new`, so a
        // slot counted here is fully published before it is read below.
        let in_use = self.watermark.load(Ordering::Acquire);

        let oldest = self.epochs[..in_use]
            .iter()
            .filter_map(|slot| {
                // Acquire: pairs with the thread's Release epoch stores; orders
                // its prior read section before frees made on the strength of
                // this observation.
                let epoch = slot.load(Ordering::Acquire);

                (epoch != NO_EPOCH).then_some(epoch)
            })
            .min()
            // Every thread is idle, so nothing holds anything back.
            .unwrap_or(u64::MAX);

        epoch.0 < oldest
    }
}

impl<const N: usize> Default for Domain<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A closed epoch, returned by [`Domain::advance`].
#[derive(Debug, Clone, Copy)]
pub struct Epoch(u64);

/// One thread's participation in a [`Domain`].
pub struct Local<'d, const MAX_threadS: usize = 64> {
    domain: &'d Domain<MAX_threadS>,
    thread: usize,
    // lock-free queue of nodes retired since the last call to `reclaim`.
    //
    // this speeds up the actual calls to `retire` since no locks
    // need to be taken AND speeds up `reclaim` by batching together frees.
    current: TransferStack<Links>,
    batch: RefCell<Batch>,
    _not_send_or_sync: PhantomData<*const ()>,
}

struct Batch {
    nodes: Stack<Links>,
    // the epoch this batch was created at
    epoch: Epoch,
}

impl<'d, const MAX_threadS: usize> Local<'d, MAX_threadS> {
    /// Returns a new, idle `Local` within the given `domain`.
    ///
    /// # Safety
    ///
    /// The given `thread` MUST be unique for the `domain` this local is created
    /// in.
    ///
    /// # Panics
    ///
    /// Panics if `thread` is too big for the given `domain`.
    #[must_use]
    pub unsafe fn new(domain: &'d Domain<MAX_threadS>, thread: usize) -> Self {
        assert!(thread < MAX_threadS, "thread has no slot in this domain");

        // Release: the scan in `poll` reads the watermark before the slots, so
        // the slot must be reachable to it before anything is retired here.
        domain.watermark.fetch_max(thread + 1, Ordering::Release);

        Self {
            domain,
            thread,
            current: TransferStack::new(),
            batch: RefCell::new(Batch {
                nodes: Stack::new(),
                epoch: Epoch(NO_EPOCH),
            }),
            _not_send_or_sync: PhantomData,
        }
    }

    #[inline]
    fn epoch(&self) -> &AtomicU64 {
        &self.domain.epochs[self.thread]
    }

    /// Runs `f` inside a QSBR read section.
    #[inline(always)]
    pub fn read<R>(&self, f: impl FnOnce(&Guard) -> R) -> R {
        debug_assert_ne!(
            self.epoch().load(Ordering::Relaxed),
            NO_EPOCH,
            "qsbr: read section on an idle thread"
        );

        f(&Guard {
            _local: PhantomData,
        })
    }

    /// Mark `node` for safe deferred reclamation.
    ///
    /// # Safety
    ///
    /// Ownership is transferred to this `Local`. Once retired you may not
    /// access its memory anymore.
    pub unsafe fn retire<T: Reclaimable>(&self, node: NonNull<T>) {
        debug_assert_ne!(
            self.epoch().load(Ordering::Relaxed),
            NO_EPOCH,
            "qsbr: retire on an idle thread"
        );

        unsafe fn free<T: Reclaimable>(links: NonNull<Links>) {
            // SAFETY: `retire` derived `links` from a `NonNull<T>` the same way.
            let node = unsafe { links.byte_sub(T::LINKS_OFFSET).cast::<T>() };
            // SAFETY: the grace period elapsed, so no thread can still reach it.
            drop(unsafe { T::from_ptr(node) });
        }

        // Safety: the caller promised `LINKS_OFFSET` to be valid when implementing
        // the trait.
        let links = unsafe { node.byte_add(T::LINKS_OFFSET).cast::<Links>() };

        // Safety: see above.
        unsafe { (*links.as_ptr()).reclaim = MaybeUninit::new(free::<T>) };

        self.current.push(links);
    }

    /// Reclaims up to `budget` retired nodes whose grace period has elapsed and
    /// returns how many were freed.
    pub fn reclaim(&self, budget: usize) -> usize {
        let Ok(mut batch) = self.batch.try_borrow_mut() else {
            return 0;
        };

        let epoch = batch.epoch;
        let mut reclaimed = self.reclaim_inner(&mut batch.nodes, epoch, budget);

        // Rotate only once `pending` is fully drained — otherwise the leftover
        // nodes would be dropped on the floor.
        if batch.nodes.is_empty() {
            batch.nodes = self.current.take_all();
            let epoch = self.domain.advance();
            batch.epoch = epoch;

            // The freshly rotated batch may already be past its grace period.
            reclaimed += self.reclaim_inner(&mut batch.nodes, epoch, budget - reclaimed);
        }

        reclaimed
    }

    /// Pops and frees nodes from `pending`, oldest first, up to `budget`, but
    /// only if `epoch`'s grace period has elapsed. Returns the number freed.
    fn reclaim_inner(&self, pending: &mut Stack<Links>, epoch: Epoch, budget: usize) -> usize {
        let mut reclaimed = 0;

        if self.domain.poll(epoch) {
            while reclaimed < budget
                && let Some(node) = pending.pop()
            {
                // SAFETY: `retire` set `reclaim` and transferred ownership of
                // this node, and `poll` confirmed its grace period elapsed.
                unsafe {
                    let reclaim = node.as_ref().reclaim.assume_init();
                    reclaim(node);
                }
                reclaimed += 1;
            }
        }

        reclaimed
    }

    /// Reports that this thread holds no references to shared data, letting the
    /// domain free anything retired before its current epoch.
    ///
    /// # Safety
    ///
    /// The caller must not be inside a read section, and must hold no reference
    /// obtained from one.
    pub unsafe fn quiescent(&self) {
        let local_epoch = self.epoch().load(Ordering::Relaxed);
        debug_assert_ne!(local_epoch, NO_EPOCH, "qsbr: quiescent on an idle thread");

        // Acquire: make sure that we observe every Release from `advance`/`retire`
        let global_epoch = self.domain.global_epoch.load(Ordering::Acquire);
        if local_epoch != global_epoch {
            // Release: pairs with the Acquire load in `Domain::poll`; orders
            // this thread's preceding reads (its last uses of retired objects)
            // before the reclaimer's frees.
            self.epoch().store(global_epoch, Ordering::Release);
        }
    }

    /// Marks this thread idle, excluding it from epoch accounting.
    ///
    /// Call before going to sleep or offline. The thread must not enter a read
    /// section again until [`exit_idle`][Local::exit_idle].
    ///
    /// # Safety
    ///
    /// The caller must not be inside a read section, and must hold no reference
    /// obtained from one.
    #[inline]
    pub unsafe fn enter_idle(&self) {
        // Release: as in `quiescent`, this is the thread's final report before it
        // stops publishing, so its prior reads must not sink past it.
        self.epoch().store(NO_EPOCH, Ordering::Release);
    }

    /// Marks this thread active, including it in epoch accounting.
    ///
    /// # Safety
    ///
    /// The caller must run this on the thread that owns this `Local`, before
    /// entering any read section.
    #[inline]
    pub unsafe fn exit_idle(&self) {
        let epoch = self.domain.global_epoch.load(Ordering::Relaxed);
        self.epoch().store(epoch, Ordering::Relaxed);

        // Store→load barrier, paired with the SeqCst fence in `Domain::poll`:
        // either the reclaimer sees our epoch store, or we see every unlink it
        // made before scanning — so it can never free something our upcoming
        // read sections can still reach.
        fence(Ordering::SeqCst);
    }
}

pub struct Guard<'l> {
    _local: PhantomData<(&'l (), *const ())>,
}

pub struct Links {
    links: stack::Links<Self>,
    // the reclaim function is set by [`Local::retire`].
    reclaim: MaybeUninit<unsafe fn(NonNull<Self>)>,
}

impl Links {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            links: stack::Links::new(),
            reclaim: MaybeUninit::uninit(),
        }
    }
}

// Safety: callers transfer ownership when calling `Local::retire` AND
// `stack::Links` makes the whole type !Unpin and we in this crate never move
// out of the allocation.
unsafe impl cordyceps::Linked<stack::Links<Self>> for Links {
    type Handle = NonNull<Self>;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(target: NonNull<Self>) -> NonNull<stack::Links<Self>> {
        // SAFETY: raw field projection; no intermediate reference formed.
        unsafe { NonNull::new_unchecked(&raw mut (*target.as_ptr()).links) }
    }
}

/// An object that can be reclaimed through QSBR.
///
/// ```
/// # use core::mem::offset_of;
/// # use core::ptr::NonNull;
/// # use qsbr::{Links, Reclaimable};
/// struct Item {
///     value: u32,
///     links: Links,
/// }
///
/// // SAFETY: `links` is `Item`'s own hook, and `from_ptr` undoes `Box::into_raw`.
/// unsafe impl Reclaimable for Item {
///     type Handle = Box<Self>;
///     const LINKS_OFFSET: usize = offset_of!(Self, links);
///
///     unsafe fn from_ptr(ptr: NonNull<Self>) -> Box<Self> {
///         unsafe { Box::from_raw(ptr.as_ptr()) }
///     }
/// }
/// ```
///
/// # Safety
///
/// [`LINKS_OFFSET`][Self::LINKS_OFFSET] must be the offset of the [`Links`]
/// field.
pub unsafe trait Reclaimable: Sized {
    /// Owns a retired `Self`; dropping it frees the object.
    type Handle;

    /// Offset of the [`Links`] hook within `Self` — always
    /// `offset_of!(Self, <hook field>)`.
    const LINKS_OFFSET: usize;

    /// Takes back ownership of a retired `Self`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle;
}

#[cfg(all(test, not(loom)))]
mod tests {
    use core::mem::offset_of;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// Brings thread `thread` of `domain` online.
    fn online<const N: usize>(domain: &Domain<N>, thread: usize) -> Local<'_, N> {
        // SAFETY: each test names every thread at most once, and the returned
        // `Local` never leaves this thread.
        let local = unsafe { Local::new(domain, thread) };
        // SAFETY: never active, so not inside a read section.
        unsafe { local.exit_idle() };

        local
    }

    /// Retires `f` so that it runs once the grace period elapses.
    fn defer<F: FnOnce(), const N: usize>(local: &Local<'_, N>, f: F) {
        struct Carrier<F: FnOnce()> {
            f: Option<F>,
            links: Links,
        }

        impl<F: FnOnce()> Drop for Carrier<F> {
            fn drop(&mut self) {
                (self.f.take().unwrap())();
            }
        }

        // SAFETY: `links` is `Carrier`'s own hook, and `from_ptr` undoes the
        // `Box::leak` below.
        unsafe impl<F: FnOnce()> Reclaimable for Carrier<F> {
            type Handle = Box<Self>;
            const LINKS_OFFSET: usize = offset_of!(Self, links);

            unsafe fn from_ptr(ptr: NonNull<Self>) -> Box<Self> {
                unsafe { Box::from_raw(ptr.as_ptr()) }
            }
        }

        let carrier = NonNull::from(Box::leak(Box::new(Carrier {
            f: Some(f),
            links: Links::new(),
        })));

        // SAFETY: freshly allocated, exclusively owned, and retired once.
        unsafe { local.retire(carrier) };
    }

    /// A counter and the closure that bumps it.
    fn counter() -> (Arc<AtomicUsize>, impl FnOnce()) {
        let count = Arc::new(AtomicUsize::new(0));
        let bump = {
            let count = count.clone();
            move || {
                count.fetch_add(1, Ordering::Relaxed);
            }
        };

        (count, bump)
    }

    #[test]
    fn retire_waits_for_quiescence() {
        static DOMAIN: Domain<64> = Domain::new();

        let local = online(&DOMAIN, 0);
        let (drops, bump) = counter();
        defer(&local, bump);

        // thread active, no quiescent state since the retire: pending.
        assert_eq!(local.reclaim(usize::MAX), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        // SAFETY: not in a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_local_publishes_only_its_own_slot() {
        static DOMAIN: Domain<64> = Domain::new();

        let local = online(&DOMAIN, 3);
        assert_ne!(DOMAIN.epochs[3].load(Ordering::Relaxed), NO_EPOCH);
        assert_eq!(DOMAIN.epochs[0].load(Ordering::Relaxed), NO_EPOCH);

        // SAFETY: not in a read section.
        unsafe { local.enter_idle() };
        assert_eq!(DOMAIN.epochs[3].load(Ordering::Relaxed), NO_EPOCH);
    }

    #[test]
    fn another_thread_holds_the_batch_back_until_it_goes_idle() {
        static DOMAIN: Domain<64> = Domain::new();

        let holder = online(&DOMAIN, 0);
        let local = online(&DOMAIN, 1);

        let (drops, bump) = counter();
        defer(&local, bump);

        // Rotates the retire into `pending` and opens its epoch.
        assert_eq!(local.reclaim(usize::MAX), 0);

        // SAFETY: not in a read section.
        unsafe { local.quiescent() };
        // This thread has passed a quiescent state since the epoch opened, but
        // `holder` has not — one active thread is enough to hold the batch back.
        assert_eq!(local.reclaim(usize::MAX), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        // Hotplug: an offline thread releases the batch without ever reporting a
        // quiescent state.
        //
        // SAFETY: not in a read section.
        unsafe { holder.enter_idle() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn an_offline_thread_keeps_its_batch_for_when_it_returns() {
        static DOMAIN: Domain<64> = Domain::new();

        let local = online(&DOMAIN, 0);
        let (drops, bump) = counter();
        defer(&local, bump);

        // Offline with the retire still outstanding.
        //
        // SAFETY: not in a read section.
        unsafe { local.enter_idle() };
        assert_eq!(drops.load(Ordering::Relaxed), 0, "not freed prematurely");

        // Back online: the batch never left this thread.
        //
        // SAFETY: on the owning thread, before any read section.
        unsafe { local.exit_idle() };

        assert_eq!(local.reclaim(usize::MAX), 0, "rotates the batch");
        // SAFETY: not in a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_reclaim_callback_may_retire() {
        static DOMAIN: Domain<64> = Domain::new();

        let local = online(&DOMAIN, 0);
        let (drops, bump) = counter();

        let inner = drops.clone();
        defer(&local, || {
            bump();
            // Retired from inside `drain`, with the batch borrow held.
            defer(&local, move || {
                inner.fetch_add(1, Ordering::Relaxed);
            });
        });

        assert_eq!(local.reclaim(usize::MAX), 0, "rotates the batch");
        // SAFETY: not in a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        // The re-entrant retire landed in `current` and rotates like any other.
        assert_eq!(local.reclaim(usize::MAX), 0, "rotates the batch");
        // SAFETY: not in a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn budgeted_reclaim_frees_at_most_the_budget() {
        static DOMAIN: Domain<64> = Domain::new();

        let local = online(&DOMAIN, 0);
        let (count, _) = counter();
        for _ in 0..5 {
            let count = count.clone();
            defer(&local, move || {
                count.fetch_add(1, Ordering::Relaxed);
            });
        }

        // Rotates all five into `pending` and opens their epoch.
        assert_eq!(local.reclaim(2), 0);
        // SAFETY: not in a read section.
        unsafe { local.quiescent() };

        assert_eq!(local.reclaim(2), 2, "budget caps the batch");
        assert_eq!(count.load(Ordering::Relaxed), 2);
        assert_eq!(local.reclaim(usize::MAX), 3);
        assert_eq!(count.load(Ordering::Relaxed), 5);
    }
}

#[cfg(loom)]
mod loom_tests {
    use core::mem::offset_of;
    use std::sync::Arc;

    use super::*;
    use crate::loom::sync::atomic::AtomicBool;
    use crate::loom::{model, thread};

    /// Two threads is all these models need, and every extra slot is another
    /// loom atomic in the registry scan.
    type TestDomain = Domain<2>;

    /// A retirable node carrying the flag its drop asserts on.
    struct Node {
        in_use: Arc<AtomicBool>,
        links: Links,
    }

    impl Node {
        fn new(in_use: Arc<AtomicBool>) -> NonNull<Self> {
            NonNull::from(Box::leak(Box::new(Node {
                in_use,
                links: Links::new(),
            })))
        }
    }

    impl Drop for Node {
        fn drop(&mut self) {
            assert!(
                !self.in_use.load(Ordering::SeqCst),
                "freed a node a waking thread could still reach"
            );
        }
    }

    // SAFETY: `links` is `Node`'s own hook, and `from_ptr` undoes the
    // `Box::leak` in `new`.
    unsafe impl Reclaimable for Node {
        type Handle = Box<Self>;
        const LINKS_OFFSET: usize = offset_of!(Self, links);

        unsafe fn from_ptr(ptr: NonNull<Self>) -> Box<Self> {
            unsafe { Box::from_raw(ptr.as_ptr()) }
        }
    }

    /// The reclaimer must not free a node a thread coming back from idle can
    /// still reach.
    #[test]
    fn a_waking_thread_is_never_freed_out_from_under() {
        model(|| {
            let domain = Arc::new(TestDomain::new());
            let slot = Arc::new(Atomic::<Node>::null());
            let in_use = Arc::new(AtomicBool::new(false));

            // SAFETY: freshly allocated, and freed only by the retire below.
            unsafe { slot.store(Some(Node::new(in_use.clone())), Ordering::Release) };

            let reader = {
                let (domain, slot) = (domain.clone(), slot.clone());

                thread::spawn(move || {
                    // SAFETY: the only `Local` naming thread 1 in this domain.
                    let local = unsafe { Local::new(&domain, 1) };

                    // Wake and immediately look: whatever this thread can still
                    // reach must still be there.
                    //
                    // SAFETY: on the owning thread, before any read section.
                    unsafe { local.exit_idle() };
                    local.read(|guard| {
                        let reachable = slot.load(Ordering::Acquire, guard).is_some();
                        in_use.store(reachable, Ordering::SeqCst);
                    });
                    in_use.store(false, Ordering::SeqCst);

                    // SAFETY: the read section is over and nothing outlives it.
                    unsafe { local.enter_idle() };
                })
            };

            // SAFETY: the only `Local` naming thread 0 in this domain.
            let local = unsafe { Local::new(&domain, 0) };
            // SAFETY: never active, so not inside a read section.
            unsafe { local.exit_idle() };

            // Unlink, then retire: a thread that wakes after this cannot reach it.
            let unlinked = local.read(|guard| {
                // SAFETY: as above.
                unsafe { slot.swap(None, Ordering::AcqRel, guard) }.map(Shared::as_ptr)
            });
            // SAFETY: unlinked above, so unreachable to any future reader, and
            // retired exactly once.
            unsafe { local.retire(unlinked.expect("the slot was filled above")) };

            let mut freed = local.reclaim(usize::MAX);
            // SAFETY: not in a read section.
            unsafe { local.quiescent() };
            freed += local.reclaim(usize::MAX);

            reader.join().unwrap();

            // Nothing holds the batch back now, so this frees the node if the
            // interleaving had not already — leaking it once per execution
            // would swamp the model.
            //
            // SAFETY: not in a read section.
            unsafe { local.quiescent() };
            freed += local.reclaim(usize::MAX);
            assert_eq!(freed, 1);
        });
    }
}
