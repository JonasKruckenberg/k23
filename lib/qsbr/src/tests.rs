// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::mem::ManuallyDrop;
use std::ptr::{self, NonNull};

use cpu_local::cpu_local;

use super::*;
use crate::loom::sync::atomic::AtomicU64;
use crate::loom::thread;

fn defer<F: FnOnce() + Send>(domain: &QsbrDomain, f: F) {
    #[repr(C)]
    struct Carrier<F> {
        head: QsbrHead,
        f: ManuallyDrop<F>,
    }
    unsafe fn run<F: FnOnce()>(node: NonNull<QsbrHead>) {
        let mut c: Box<Carrier<F>> = unsafe { Box::from_raw(node.as_ptr().cast()) };
        let f = unsafe { ManuallyDrop::take(&mut c.f) };
        drop(c);
        f();
    }
    let carrier = Box::new(Carrier {
        head: QsbrHead::new(run::<F>),
        f: ManuallyDrop::new(f),
    });
    // SAFETY: freshly allocated, exclusively owned, repr(C) with the node
    // first; `run::<F>` frees it exactly once; the closure owns its
    // captures.
    unsafe { domain.retire(NonNull::from(Box::leak(carrier)).cast()) };
}

#[test]
fn retire_waits_for_quiescence() {
    static DOMAIN: QsbrDomain = QsbrDomain::new();
    cpu_local! {
        static READER: QsbrReader = QsbrReader::new();
    }

    unsafe { READER.register(&DOMAIN) };

    let drops = Arc::new(AtomicUsize::new(0));
    let d = drops.clone();
    defer(&DOMAIN, move || {
        d.fetch_add(1, Ordering::Relaxed);
    });

    // CPU active, no quiescent state since the retire: pending.
    assert_eq!(DOMAIN.reclaim(usize::MAX), 0);
    assert_eq!(drops.load(Ordering::Relaxed), 0);

    unsafe { READER.quiescent(&DOMAIN) };
    assert_eq!(DOMAIN.reclaim(usize::MAX), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn idle_cpu_does_not_block_reclaim() {
    static DOMAIN: QsbrDomain = QsbrDomain::new();
    cpu_local! {
        static READER: QsbrReader = QsbrReader::new();
    }

    unsafe { READER.register(&DOMAIN) };
    unsafe { READER.enter_idle() };

    let drops = Arc::new(AtomicUsize::new(0));
    let d = drops.clone();
    defer(&DOMAIN, move || {
        d.fetch_add(1, Ordering::Relaxed);
    });
    // An all-idle system reclaims freely (min_active_epoch == MAX).
    assert_eq!(DOMAIN.reclaim(usize::MAX), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    unsafe { READER.exit_idle(&DOMAIN) };
}

#[test]
fn budgeted_reclaim() {
    let domain = QsbrDomain::new(); // no CPUs: every epoch complete
    let count = Arc::new(AtomicUsize::new(0));
    for _ in 0..5 {
        let count = count.clone();
        defer(&domain, move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
    }
    assert_eq!(domain.reclaim(2), 2);
    assert_eq!(domain.reclaim(usize::MAX), 3);
    assert_eq!(count.load(Ordering::Relaxed), 5);
}

#[test]
fn advance_poll_epochs() {
    static DOMAIN: QsbrDomain = QsbrDomain::new();
    cpu_local! {
        static READER: QsbrReader = QsbrReader::new();
    }

    unsafe { READER.register(&DOMAIN) };

    let epoch = DOMAIN.advance();
    assert!(!DOMAIN.poll(epoch), "active CPU has not moved past it yet");
    unsafe { READER.quiescent(&DOMAIN) };
    assert!(DOMAIN.poll(epoch));

    // A read section blocks completion until it ends.
    let epoch = DOMAIN.advance();
    READER.read(|_| assert!(!DOMAIN.poll(epoch)));
    unsafe { READER.quiescent(&DOMAIN) };
    assert!(DOMAIN.poll(epoch));
}

#[test]
fn intrusive_retire() {
    #[repr(C)]
    struct Node {
        retired: QsbrHead,
        value: u64,
    }
    unsafe fn drop_node(node: NonNull<QsbrHead>) {
        drop(unsafe { Box::from_raw(node.as_ptr().cast::<Node>()) });
    }

    let domain = QsbrDomain::new();
    let node = Box::into_raw(Box::new(Node {
        retired: QsbrHead::new(drop_node),
        value: 7,
    }));
    unsafe {
        assert_eq!((*node).value, 7);
        domain.retire(NonNull::new_unchecked(ptr::addr_of_mut!((*node).retired)));
    }
    assert_eq!(domain.reclaim(usize::MAX), 1);
}

#[test]
fn atomic_load_store_cas() {
    static DOMAIN: QsbrDomain = QsbrDomain::new();
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Payload(u64);
    impl Drop for Payload {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    cpu_local! {
        static READER: QsbrReader = QsbrReader::new();
    }

    unsafe { READER.register(&DOMAIN) };

    {
        let slot = QsbrCell::new(Payload(1), &DOMAIN);

        READER.read(|guard| {
            let one = slot.load(guard);
            assert_eq!(one.0, 1);

            // Successful CAS retires the displaced value.
            slot.compare_exchange(one, Payload(2), guard)
                .map_err(drop)
                .expect("CAS should succeed");

            // Failed CAS hands the new value back, unspent.
            let (actual, unspent) = slot
                .compare_exchange(one, Payload(3), guard)
                .expect_err("CAS should fail");
            assert_eq!(actual.0, 2);
            assert_eq!(unspent.0, 3);

            slot.store(Payload(4));
            assert_eq!(slot.load(guard).0, 4);
        });
        // 1 (CAS) and 2 (store) retired; 3 dropped in place on CAS
        // failure; 4 retired by `slot` dropping here.
    }

    unsafe { READER.quiescent(&DOMAIN) };
    assert_eq!(DOMAIN.reclaim(usize::MAX), 3);
    // 3 reclaimed + the in-place drop of the failed CAS value.
    assert_eq!(DROPS.load(Ordering::Relaxed), 4);
}

/// Readers hammer the safe `Atomic` path while a writer (not itself a
/// reader!) replaces values; every displaced value must drop exactly once
/// and never while readers can still reach it.
#[test]
fn concurrent_stress() {
    const WRITES: u64 = 20_000;
    const READERS: usize = 4;

    static DOMAIN: QsbrDomain = QsbrDomain::new();
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Payload(u64);
    impl Drop for Payload {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    let slot = QsbrCell::new(Payload(0), &DOMAIN);
    let done = AtomicU64::new(0);

    thread::scope(|s| {
        for _ in 0..READERS {
            s.spawn(|| {
                cpu_local! {
                    static READER: QsbrReader = QsbrReader::new();
                }

                unsafe { READER.register(&DOMAIN) };
                while done.load(Ordering::Acquire) == 0 {
                    READER.read(|guard| {
                        // The dereference is the assertion: a premature
                        // free is a use-after-free here.
                        let value = slot.load(guard);
                        assert!(value.0 <= WRITES);
                    });
                    unsafe { READER.quiescent(&DOMAIN) };
                }
                // Threads exit but the leaked, registered CPU must not
                // stall the domain afterwards: park it idle.
                unsafe { READER.enter_idle() };
            });
        }

        s.spawn(|| {
            // Writers need no registration at all.
            for i in 1..=WRITES {
                slot.store(Payload(i));
                if i % 64 == 0 {
                    DOMAIN.reclaim(usize::MAX);
                }
            }
            done.store(1, Ordering::Release);
        });
    });

    drop(slot); // retires the final value
    DOMAIN.reclaim(usize::MAX);
    // WRITES values displaced by stores + the final value retired on drop.
    assert_eq!(DROPS.load(Ordering::Relaxed), WRITES as usize + 1);
}
