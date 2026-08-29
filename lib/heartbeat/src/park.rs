// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Per-worker blocking: a wake token, and a backend that sleeps the hart.
//!
//! Blocking is per-*worker*, never per-job: a hart can only block on itself, and
//! only another hart can wake it. The whole synchronization lives in the token —
//! the three-state [`Park::state`] below — and *not* in the sleep instruction. A
//! waker makes its condition true and then records a token; a sleeper commits to
//! sleeping and then re-checks. So the backend is free to return spuriously, or
//! never to sleep at all: a [`ParkVTable`] whose `park` is a bare
//! [`spin_loop`](core::hint::spin_loop) is correct, merely wasteful, and
//! everything else is an optimization on top of that.

use crate::loom::sync::atomic::{AtomicI8, Ordering};

/// No token, and nobody sleeping.
const EMPTY: i8 = 0;

/// Committed to sleeping: the parker has decided to sleep, but may not have
/// reached [`ParkVTable::park`] yet. This is the state the stickiness
/// requirement is about.
const PARKED: i8 = -1;

/// A token is available; the next [`Park::park`] consumes it and returns at once.
const NOTIFIED: i8 = 1;

/// A worker's wake permit, plus the backend that actually sleeps its hart.
///
/// The permit is exactly `std::thread::park`'s, k23's `Notify`, or maitake's
/// `WaitCell`: recording a token when one is already recorded means the target
/// has an unconsumed permit, and therefore *cannot stay asleep* — it is
/// guaranteed to wake, consume it, and re-check its own condition.
pub struct Park {
    ptr: *const (),
    vtable: &'static ParkVTable,
    state: AtomicI8,
}

/// How to sleep and wake one hart. Each function receives the `ptr` given to
/// [`Park::new`].
///
/// # The one hard requirement: the backend must be sticky
///
/// [`unpark`](ParkVTable::unpark) may be called *before* the matching
/// [`park`](ParkVTable::park). A parker commits to sleeping (the state becomes
/// [`PARKED`]) and only then calls into the backend; a waker that sees `PARKED`
/// signals immediately. Nothing closes the window in between — nothing *can*,
/// without a lock — so a backend has to behave like a **permit**, not like a
/// condition-variable signal:
///
/// > once unparked, the *next* `park` must return immediately rather than sleep,
/// > even if that `park` had not been entered yet when the wake arrived.
///
/// `std::thread::park`/`unpark` is sticky. A bare `wfi` + IPI is sticky *only*
/// if the hart parks with interrupts globally masked but individually enabled
/// (on RISC-V: `sstatus.SIE` clear, `sie.SSIE` set) — the IPI then becomes
/// pending rather than taken, and a pending interrupt makes `wfi` fall through
/// instead of sleeping. Clearing `sie.SSIE` instead is the mistake worth
/// fearing: `wfi` then really does sleep forever. A plain condition variable, or
/// a notification primitive that drops a wake when nobody is waiting, is **not**
/// sticky and will lose wakeups here.
///
/// The always-correct fallback is not to sleep at all:
///
/// ```
/// # use heartbeat::ParkVTable;
/// fn park(_ptr: *const ()) {
///     core::hint::spin_loop();
/// }
/// fn unpark(_ptr: *const ()) {}
/// fn drop(_ptr: *const ()) {}
///
/// static SPIN: ParkVTable = ParkVTable { park, unpark, drop };
/// ```
///
/// which burns power but cannot hang, because [`Park::park`] re-checks the token
/// in a loop. Spurious returns from `park` are always permitted.
///
/// The scheduler itself never touches interrupt state; that is entirely
/// `park`'s business. `park` must also never take a lock another worker could
/// hold — it is never called with any scheduler state locked, so an `ecall` into
/// the SBI is fine.
pub struct ParkVTable {
    /// Sleep until unparked. May return spuriously, at any time, for any reason;
    /// may also not sleep at all. Must be sticky — see the type docs.
    pub park: fn(*const ()),
    /// Wake the hart, whether or not it is currently parked.
    pub unpark: fn(*const ()),
    /// Release the `ptr` given to [`Park::new`]. Called exactly once, when the
    /// `Park` is dropped.
    pub drop: fn(*const ()),
}

// Safety: `ptr` is only ever handed back to the vtable's own functions, which
// `Park::new`'s contract requires to be callable from any hart. All mutable
// state is in `state`, which is atomic.
unsafe impl Send for Park {}

// Safety: as above.
unsafe impl Sync for Park {}

impl Park {
    /// Bind a backend to a new, untokened `Park`.
    ///
    /// # Safety
    ///
    /// - `ptr` must stay valid until this `Park` is dropped. The scheduler hands
    ///   it to `vtable`'s functions from *other* harts, so whatever it points at
    ///   must be `Send + Sync`.
    /// - `vtable` must be sticky, as described on [`ParkVTable`].
    // Not `const`: under `--cfg loom` the atomics are not const-constructible.
    #[must_use]
    pub unsafe fn new(ptr: *const (), vtable: &'static ParkVTable) -> Self {
        Self {
            ptr,
            vtable,
            state: AtomicI8::new(EMPTY),
        }
    }

    /// Consume one token, sleeping until one arrives.
    ///
    /// May return spuriously: the caller re-checks its own condition.
    pub fn park(&self) {
        // NOTIFIED => EMPTY (consume the token and return), or EMPTY => PARKED.
        if self.state.fetch_sub(1, Ordering::Acquire) == NOTIFIED {
            return;
        }

        loop {
            (self.vtable.park)(self.ptr);

            // NOTIFIED => EMPTY: our token, consumed. Anything else was spurious
            // — the state is still PARKED — so sleep again.
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Ordering::Acquire, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Record a token, and signal the hart if it is asleep. Idempotent.
    ///
    /// Returns `true` if this call recorded the token, i.e. if the target had no
    /// unconsumed permit. Callers must make their condition true *before* calling
    /// this.
    pub fn unpark(&self) -> bool {
        match self.state.swap(NOTIFIED, Ordering::AcqRel) {
            // Awake, and now holding a token: its next `park` returns at once, so
            // there is nothing to signal.
            EMPTY => true,
            // Committed to sleeping — though possibly not yet inside the
            // backend's `park`, which is exactly what stickiness covers.
            PARKED => {
                (self.vtable.unpark)(self.ptr);
                true
            }
            // Already NOTIFIED: somebody beat us to it, and one token is enough.
            _ => false,
        }
    }
}

impl Drop for Park {
    fn drop(&mut self) {
        (self.vtable.drop)(self.ptr);
    }
}
