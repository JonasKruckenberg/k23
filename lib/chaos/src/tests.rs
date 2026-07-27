// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#[cfg(all(test, feature = "__test-impl"))]
pub(crate) use self::bundled::{reseed, set_fuel};

#[cfg(feature = "__test-impl")]
mod bundled {
    use core::cell::RefCell;

    use crate::{Callsite, ControlPlane, Impl};

    std::thread_local! {
        static PLANE: RefCell<ControlPlane> = RefCell::new(ControlPlane::new(0));
    }

    /// Replace this thread's plane with a freshly seeded one.
    #[cfg(test)]
    pub(crate) fn reseed(seed: u64) {
        PLANE.with(|plane| *plane.borrow_mut() = ControlPlane::new(seed));
    }

    /// Cap the number of decisions this thread's plane may fire.
    #[cfg(test)]
    pub(crate) fn set_fuel(fuel: u32) {
        PLANE.with(|plane| plane.borrow_mut().set_fuel(fuel));
    }

    struct TestImpl;

    // Safety: the plane is thread-local and each call borrows it only for that
    // call. Tests do not invoke this re-entrantly.
    unsafe impl Impl for TestImpl {
        fn decide(site: Callsite) -> bool {
            PLANE.with(|plane| plane.borrow_mut().decide_at(site))
        }

        fn delay(site: Callsite) {
            PLANE.with(|plane| plane.borrow_mut().delay_at(site));
        }

        fn random(site: Callsite) -> u64 {
            PLANE.with(|plane| plane.borrow_mut().random_at(site))
        }
    }

    crate::set_impl!(TestImpl);
}

/// Properties of the arming function, which is compiled in both configurations.
#[cfg(test)]
mod default_impl {
    use crate::Callsite;
    use crate::default_impl::armed;

    const SEEDS: u64 = 64;

    fn sites() -> impl Iterator<Item = Callsite> {
        (10..40).flat_map(|len| {
            [5_u32, 9, 13, 21, 37]
                .into_iter()
                .flat_map(move |col| (1..500).map(move |line| Callsite::__at(line, col, len)))
        })
    }

    #[test]
    fn arms_about_one_site_in_four() {
        let (hits, total) = (0..SEEDS)
            .flat_map(|s| sites().map(move |k| armed(s, k)))
            .fold((0_u64, 0_u64), |(h, t), a| (h + u64::from(a), t + 1));
        let pct = 100.0 * hits as f64 / total as f64;
        assert!((22.0..28.0).contains(&pct), "arming rate {pct:.1}%");
    }

    /// A mixer arming strictly alternating lines still scores 25% on the
    /// marginal rate, so only the conditional catches it.
    #[test]
    fn neighbouring_sites_are_independent() {
        let (mut pairs, mut both) = (0_u64, 0_u64);
        for seed in 0..SEEDS {
            for len in 10..40 {
                let mut prev = false;
                for line in 1..500 {
                    let now = armed(seed, Callsite::__at(line, 9, len));
                    if line > 1 {
                        pairs += u64::from(prev);
                        both += u64::from(prev && now);
                    }
                    prev = now;
                }
            }
        }
        let pct = 100.0 * both as f64 / pairs as f64;
        assert!(
            (20.0..30.0).contains(&pct),
            "P(armed | prev armed) = {pct:.1}%"
        );
    }

    #[test]
    fn seeds_select_different_subsets() {
        let subset = |s| sites().map(|k| armed(s, k)).collect::<Vec<_>>();
        assert!((1..SEEDS).all(|s| subset(s) != subset(0)));
    }
}

#[cfg(all(test, feature = "chaos"))]
mod enabled {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::{reseed, set_fuel};
    use crate::{IteratorExt, SliceExt};

    #[test]
    fn a_seed_replays_identically() {
        let run = || {
            reseed(7);
            (0..500).map(|_| decide!()).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn fuel_bounds_total_fires() {
        let mut fired = 0;
        for seed in 0..1_000 {
            reseed(seed);
            set_fuel(3);
            let n = (0..10_000).filter(|_| decide!()).count();
            assert!(n <= 3, "fuel exceeded: {n}");
            fired += n;
        }
        // Without this the test also passes when the site is never armed.
        assert!(fired > 0, "no seed armed this site");
    }

    #[test]
    fn shuffle_permutes_without_losing_elements() {
        const SORTED: [u32; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
        let mut shuffled = 0;
        for seed in 0..200 {
            reseed(seed);
            for _ in 0..50 {
                let mut items = SORTED;
                items.shuffle(site!());
                let mut seen = items;
                seen.sort_unstable();
                assert_eq!(seen, SORTED, "shuffle lost or duplicated an element");
                shuffled += usize::from(items != SORTED);
            }
        }
        assert!(shuffled > 0, "no seed armed this site");
    }

    #[test]
    fn shuffled_yields_every_item_once() {
        let mut reordered = 0;
        for seed in 0..200 {
            reseed(seed);
            let got: Vec<u32> = (0..40_u32).shuffled(site!()).collect();
            let mut seen = got.clone();
            seen.sort_unstable();
            assert_eq!(
                seen,
                (0..40).collect::<Vec<_>>(),
                "lost or duplicated an item"
            );
            reordered += usize::from(got != (0..40).collect::<Vec<_>>());
        }
        assert!(reordered > 0, "no seed armed this site");
    }

    #[test]
    fn assert_stable_accepts_an_unchanging_value() {
        for seed in 0..200 {
            reseed(seed);
            assert_stable!(|| 7_u64);
        }
    }

    /// Proves the window opens and both observations are really taken: a value
    /// that changes on every read must be caught whenever the site fires.
    #[test]
    fn assert_stable_catches_a_change() {
        let mut caught = 0;
        for seed in 0..500 {
            reseed(seed);
            let reads = Cell::new(0_u64);
            caught += usize::from(
                catch_unwind(AssertUnwindSafe(|| {
                    assert_stable!(|| {
                        reads.set(reads.get() + 1);
                        reads.get()
                    });
                }))
                .is_err(),
            );
        }
        assert!(
            caught > 0,
            "no seed armed this site, so nothing was checked"
        );
    }
}

#[cfg(all(test, not(feature = "chaos")))]
mod disabled {
    use std::cell::Cell;

    use crate::{IteratorExt, SliceExt};

    // Const-evaluated, so these fail to compile unless the macros fold to
    // compile-time constants.
    const _: () = assert!(!decide!());
    const _: () = delay!();

    #[test]
    fn shuffled_is_the_identity() {
        let it: core::ops::Range<u32> = (0..8_u32).shuffled(site!());
        assert_eq!(it.collect::<Vec<_>>(), (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn shuffle_leaves_the_slice_alone() {
        let mut items = [0_u32, 1, 2, 3];
        items.shuffle(site!());
        assert_eq!(items, [0, 1, 2, 3]);
    }

    /// Even a closure that changes on every read must not run.
    #[test]
    fn assert_stable_is_inert() {
        let reads = Cell::new(0_u64);
        assert_stable!(|| {
            reads.set(reads.get() + 1);
            reads.get()
        });
        assert_eq!(reads.get(), 0, "the closure ran with chaos disabled");
    }
}
