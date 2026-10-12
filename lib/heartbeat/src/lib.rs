// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Heartbeat-scheduled fork/join parallelism.
//!
//! Forking is nearly free — a few plain stores into a worker-local intrusive list
//! — and *sharing* is rare: a timer interrupt periodically flips a per-worker
//! flag, and the worker's next fork then promotes its oldest queued job so an
//! idle worker can pick it up. Workers therefore never contend over fine-grained
//! work items, and a program with no parallelism to exploit runs at sequential
//! speed.
//!
//! # Index of performance techniques
//!
//! Everything that makes (or measurably made) this crate fast, numbered so a
//! future rebuild can cite them. Benchmarked against the Zig reference
//! implementation (Spice) summing a 10M-node balanced binary tree on an M3 Pro;
//! "n-thread parity" below means within noise of Zig's ReleaseFast build.
//!
//! ## Hot-path codegen
//!
//! - **P1 — Pass the per-frame context by value.** The recursion boundary is
//!   `fn(Scope, …)`: worker pointer and job-list tail cross every call in two
//!   argument registers, and intra-frame updates are mem2reg'd away. A tail
//!   living at a fixed address (a field) turns every push/pop at every depth
//!   into a store→load round trip through that address. Upstream's
//!   `callWithContext` comment calls this signature "extremely critical"; it is.
//!   See [`Scope`], [`call`].
//! - **P2 — Never lend the hot context to a cold path.** One `#[inline(never)]`
//!   function taking `&mut Scope` makes every frame's scope address escape, so
//!   LLVM pins it to a stack slot and P1 silently evaporates (measured: the
//!   entire benefit). Cold paths take `&Worker` and rebuild their own context —
//!   see [`Worker::work_until`].
//! - **P3 — Hoist loads above the fork.** A load written *after* a join
//!   (`node.value + l + r`) gets sunk by LLVM below both recursive calls, onto
//!   every frame's critical path. Loaded *before* the fork it waits out the
//!   subtree in a callee-saved register. Worth ~20% end-to-end on the tree sum —
//!   more than every scheduler tweak combined. User-code pattern; see the
//!   `sum` examples.
//! - **P4 — Pop through the job's own `prev`, adopt it as the new tail.** One
//!   load + one store, and the only correct pop: promotion (`shift`) relinks a
//!   queued job's `prev` to the head, so the frame's remembered tail can be
//!   stale — popping through it dangles `head.next` into a dying frame. See
//!   [`Scope::fork_join`].
//! - **P5 — State packed into the link words.** `is_queued` — asked on every
//!   join — is a single load + null-test of `prev` (queued: `prev` set;
//!   promoted: `prev` null, `next` dead). No discriminant, no flags.
//!   See [`Job`].
//! - **P6 — A stub node makes the empty list unrepresentable.** Push and pop
//!   never branch on emptiness, and `is_linked`-style "lone node reports
//!   unlinked" traps disappear. See [`Worker::new`], [`Job::stub`].
//! - **P7 — Inline discipline.** `#[inline(always)]` on `fork_join` and `call`
//!   (the whole hot path must dissolve into the caller); `#[cold]` on
//!   `heartbeat`; `#[inline(never)]` on `work_until` (keeps the join's
//!   fast path small enough to stay inline).
//! - **P8 — Overlap closure and result in one union.** [`Stage`] stores `F`
//!   and `R` in the same bytes; which is live follows from the job state, so a
//!   fork costs `max(F, R)` stack bytes and no tag.
//! - **P9 — Three-word type-erased jobs.** A [`Job`] is `execute` + two links;
//!   the concrete closure/result live in the enclosing [`TypedJob`], and
//!   `execute` is monomorphized per pair. Thieves read nothing else.
//!
//! ## Architecture
//!
//! - **P10 — Owner-only `Cell`s on the fork path.** The job list is only ever
//!   touched by its owning hart, so links are plain `Cell`s: zero atomics, zero
//!   fences on the per-fork path. Cross-hart reachability is confined to
//!   [`WorkerHeader`].
//! - **P11 — One global lock, taken only at heartbeat frequency.** Promote,
//!   steal, sleep — never fork. "A global mutex is fine when there's no
//!   contention" (Spice README).
//! - **P12 — Heartbeat is a polled flag, not preemption.** The timer only does
//!   a relaxed store; the worker folds one relaxed load + branch into each
//!   fork. See [`Worker::heartbeat_flag`].
//! - **P13 — Reclaim un-stolen promotions.** A join whose job was promoted but
//!   never claimed takes it back under the lock and runs it inline, instead of
//!   detouring through the await/steal machinery (which would often execute
//!   somebody *else's* older job first — work inversion — and then run its own
//!   type-erased). Upstream's `waitForJob` fast path. Worth ~9% at every
//!   thread count, including single-threaded, where every promotion is
//!   un-stolen. See [`Worker::reclaim_shared`].
//! - **P14 — Stagger the heartbeats.** One worker per timer tick, round-robin
//!   at `interval / workers` — not all flags at once. Synchronized heartbeats
//!   make every worker promote in the same instant and stampede the scheduler
//!   lock. Applies to the real kernel too: align each hart's heartbeat timer
//!   phase-shifted, not simultaneous. See the example/bench timer threads.
//!
//! ## Measurement discipline (how the above were found)
//!
//! - **P15 — Verify the build you benchmark.** Buck2 applies a target's
//!   `modifiers` only when it is the top-level target: our criterion runner
//!   silently built the whole graph at `-Copt-level=0` (11× slower) until the
//!   runner target carried the modifiers too. Confirm with
//!   `buck2 cquery "deps(target, 1)"` — the configuration *hash* must match a
//!   known-optimized build.
//! - **P16 — Diff disassembly against the reference, don't guess.** Every real
//!   finding here (P1, P2, P3) was visible as a concrete instruction-level
//!   difference (`objdump -d --disassemble-symbols=…`) between our hot loop and
//!   Zig's; hypotheses that sounded plausible (store-count pressure) were
//!   refuted by measuring, not argued.
//! - **P17 — Benchmark hygiene.** Back-to-back A/B runs on the same machine
//!   state; compare min as well as mean (variance is a finding); allocate the
//!   benchmark tree pre-order so walk order matches layout (~2× on its own);
//!   `black_box` the recursion input or LLVM merges iterations.
//!
//! # Where the state lives
//!
//! Two places, and which is which is what makes the scheduler reasonable about:
//!
//! 1. **The running hart's stack** — the job list, rooted at [`Worker`] with
//!    its hot end, the tail, riding by value in the [`Scope`] threaded through
//!    the recursion. No other hart can name either, so forking and joining need
//!    no synchronization at all.
//! 2. **[`Scheduler::synced`]** — the promoted jobs, and the workers parked
//!    looking for work. It lives *inside* the lock, so there is no way to write
//!    down an access to it without holding the guard.
//!
//! The single lock is only taken at heartbeat frequency — on a promote, a steal,
//! or a worker going to sleep — never on the per-fork hot path. "A global mutex
//! is fine when there's no contention", as the Spice README puts it, and this
//! follows the original in taking it.
//!
//! Taking a lock is also what makes the sleep protocol correct: deciding *there
//! is no work* and *registering as idle* happen together, under the guard, so a
//! worker cannot go to sleep while a job is sitting in the queue. The original
//! gets the same atomicity from a condition variable, which releases its mutex as
//! it sleeps; we cannot hold a spinlock across a `wfi`, so the gap between
//! dropping the guard and actually sleeping is covered by [`Park`]'s token
//! instead — see [`ParkVTable`]'s stickiness requirement.

// #![no_std]

mod loom;
#[cfg(all(test, loom))]
mod loom_tests;
mod park;

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::mem::{offset_of, ManuallyDrop};
use core::ptr::{self, NonNull};

use cordyceps::list;
use loom::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
pub use park::{Park, ParkVTable};
use spin::IrqMutex;

/// The state every worker shares.
pub struct Scheduler {
    /// Read on every pass of [`Worker::main_loop`], so it stays an atomic rather
    /// than moving into [`Synced`] — but it is only ever *written* under the lock.
    /// That is what closes the race between a worker deciding to sleep and
    /// [`stop`](Scheduler::stop) draining the idle list: a worker that reads
    /// `false` while holding the guard is guaranteed to have registered itself
    /// before `stop` looks.
    stopping: AtomicBool,
    synced: IrqMutex<Synced>,
}

/// Everything the scheduler's lock guards. The only way to name it is to hold the
/// guard.
struct Synced {
    /// Workers advertising a promoted job, oldest first: promotions append, steals
    /// take from the front. That ordering *is* the oldest-first steal the original
    /// gets from a logical clock plus a scan over every worker.
    ///
    /// A worker is in here exactly while its `shared_job` is non-null, so it needs
    /// no membership flag of its own.
    shared: list::List<WorkerHeader>,
    /// Workers parked in [`Worker::main_loop`] with nothing left to do. Membership
    /// is tracked by [`WorkerHeader::in_idle_list`].
    idle: list::List<WorkerHeader>,
}

impl Synced {
    /// Register `worker` as idle.
    ///
    /// The caller must guarantee `worker` is not linked into [`Synced::shared`]
    /// — the lists share one set of links — which is equivalent to its
    /// `shared_job` slot being empty (the two change together, under the lock
    /// the caller already holds).
    fn push_idle(&mut self, worker: &WorkerHeader) {
        debug_assert!(
            !worker.in_idle_list.load(Ordering::Relaxed),
            "worker registered as idle twice"
        );
        debug_assert!(
            worker.shared_job.load(Ordering::Relaxed).is_null(),
            "a worker registering as idle must not be advertising (the lists share links)"
        );
        worker.in_idle_list.store(true, Ordering::Relaxed);
        self.idle.push_back(NonNull::from_ref(worker));
    }

    /// Take one idle worker off the list, to hand it work or to stop it.
    fn pop_idle(&mut self) -> Option<NonNull<WorkerHeader>> {
        let worker = self.idle.pop_front()?;
        // Safety: workers outlive the scheduler's use of them. See `Worker::new`.
        unsafe { worker.as_ref() }
            .in_idle_list
            .store(false, Ordering::Relaxed);
        Some(worker)
    }

    /// Take `worker` back off the idle list, if it is on it. This is the one thing an
    /// `MpscQueue` could not do, and the reason these are doubly linked.
    fn remove_idle(&mut self, worker: &WorkerHeader) {
        if worker.in_idle_list.swap(false, Ordering::Relaxed) {
            // Safety: the flag says this worker is linked into `idle`, and `idle` is
            // the only list that flag ever refers to.
            unsafe { self.idle.remove(NonNull::from_ref(worker)) };
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    // Not `const`: under `--cfg loom` the atomics are not const-constructible.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stopping: AtomicBool::new(false),
            synced: IrqMutex::new(Synced {
                shared: list::List::new(),
                idle: list::List::new(),
            }),
        }
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// Ask every worker in [`Worker::main_loop`] to return. Idempotent.
    ///
    /// May transiently pop (and wake) joiners waiting out a stolen job; they
    /// re-register until their join completes, which it always does — a thief
    /// finishes the job it is running regardless of `stopping` — so this drain
    /// terminates.
    pub fn stop(&self) {
        {
            // Under the lock, so it cannot land between a worker finding no work
            // and that worker adding itself to `idle`.
            let _synced = self.synced.lock();
            self.stopping.store(true, Ordering::Release);
        }

        loop {
            let worker = self.synced.lock().pop_idle();
            let Some(worker) = worker else { break };

            // Never unpark with the lock held: `IrqMutex` keeps interrupts off for
            // as long as it is held, and an unpark can be an `ecall` into the SBI.
            //
            // Safety: workers outlive the scheduler's use of them. See `Worker::new`.
            unsafe { worker.as_ref() }.park.unpark();
        }
    }

    /// Claim the oldest promoted job, and the worker that owns it.
    ///
    /// The caller must hold the lock, which is what makes the job ours to run: a
    /// job is in `shared` exactly once, and taking it removes it.
    fn take_oldest_shared_work(
        synced: &mut Synced,
    ) -> Option<(NonNull<Job>, NonNull<WorkerHeader>)> {
        let owner = synced.shared.pop_front()?;

        // Safety: workers outlive the scheduler's use of them. See `Worker::new`.
        let job = unsafe { owner.as_ref() }
            .shared_job
            .swap(ptr::null_mut(), Ordering::Relaxed);

        // A worker is linked into `shared` if and only if its slot is full, and
        // both happen together under this lock. Bailing rather than unwrapping
        // keeps the scheduler core panic-free even if that ever stopped holding.
        debug_assert!(!job.is_null(), "a worker in `shared` had no job to share");

        Some((NonNull::new(job)?, owner))
    }
}

/// The part of a worker that *other* harts can reach.
pub struct WorkerHeader {
    /// The job this worker is advertising. Non-null exactly while the worker is
    /// linked into [`Synced::shared`].
    ///
    /// Guarded by the scheduler's lock — every access below is `Relaxed`, because
    /// the lock already orders them. It is an atomic only so that `WorkerHeader`
    /// stays `Sync` without an `unsafe impl`.
    shared_job: AtomicPtr<Job>,

    /// Set by this hart's timer interrupt; read and cleared by the worker's next
    /// fork. See [`Worker::heartbeat_flag`].
    ///
    /// The one piece of shared state the lock does *not* guard, because an
    /// interrupt handler must never take a lock. So it is a genuine atomic, and a
    /// single relaxed store is the whole of what the ISR does.
    heartbeat: AtomicBool,

    /// Whether this worker is currently linked into [`Synced::idle`].
    ///
    /// `Links::is_linked` cannot answer this. It is `next.is_some() ||
    /// prev.is_some()`, so the *only* node in a list reports itself as **unlinked** —
    /// the same trap the job list keeps a stub node around to avoid (see
    /// [`Worker::new`]). A lone idle worker would therefore fail to take itself back
    /// off the list and enqueue itself twice.
    ///
    /// Guarded by the scheduler's lock, like `shared_job`.
    in_idle_list: AtomicBool,

    park: Park,

    /// This worker's node in **either** [`Synced::shared`] or [`Synced::idle`].
    ///
    /// One set of links for both lists, which is sound because membership in them
    /// is mutually exclusive: a worker is in `idle` only while it is parked in
    /// `main_loop` with nothing to do, and in `shared` only while it has a promoted
    /// job in flight — and a worker with nothing to do has no promoted job, because
    /// every fork is joined before its `fork_join` returns. `heartbeat`
    /// debug-asserts it.
    ///
    /// The lists are doubly linked, so a worker can take *itself* back out of one.
    /// That is what lets `main_loop` clean up after a `park` that returned without
    /// anybody having dequeued it.
    links: list::Links<WorkerHeader>,
}

impl WorkerHeader {
    fn new(park: Park) -> Self {
        Self {
            shared_job: AtomicPtr::new(ptr::null_mut()),
            heartbeat: AtomicBool::new(false),
            in_idle_list: AtomicBool::new(false),
            park,
            links: list::Links::new(),
        }
    }
}

// Safety: the links are at the `links` field, and `map_addr` preserves the
// provenance of the header the pointer came from.
unsafe impl cordyceps::Linked<list::Links<Self>> for WorkerHeader {
    type Handle = NonNull<Self>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

/// One hart's handle on the scheduler.
///
/// The job list is rooted here — its tail rides in the [`Scope`] — and lives on
/// the owning hart's stack, where no other hart can name it. Everything another
/// hart *can* reach is in the [`WorkerHeader`].
pub struct Worker<'a> {
    header: WorkerHeader,

    /// The stub handed to [`Worker::new`]: the permanent front of this worker's
    /// job list. The list itself is an intrusive doubly-linked chain threaded
    /// through the three-word [`Job`]s, which live in their `fork_join` frames
    /// on this hart's stack. Forks push at the tail and joins pop from it
    /// (LIFO); the heartbeat [`shift`](Worker::shift)s from the front (FIFO),
    /// so thieves always get the *oldest* job. That asymmetry is the heart of
    /// heartbeat scheduling.
    ///
    /// **Owner-only.** Every link is only ever touched by the hart that owns
    /// the worker, which is what makes the plain `Cell`s in [`Job`] sound:
    /// there is never a concurrent reader or writer. There is deliberately no
    /// `len` and no emptiness branch — the stub means the empty case is not
    /// special.
    job_head: NonNull<Job>,
    scheduler: &'a Scheduler,
}

impl<'a> Worker<'a> {
    /// Attach a worker to `scheduler`.
    ///
    /// `stub` is the sentinel that sits at the front of this worker's job list.
    /// It must belong to this worker alone: `Links::is_linked` is
    /// `next.is_some() || prev.is_some()`, so a job that is the *only* node in
    /// the list would report itself as unlinked. Keeping a node in front of
    /// every real job makes `is_linked` mean what `fork_join` needs it to mean —
    /// "still mine, nobody took it".
    ///
    /// # A worker must outlive the scheduler's use of it
    ///
    /// A `Worker` owns its [`WorkerHeader`], and the header is what other harts
    /// reach it through — so a worker may not be dropped while any hart can still
    /// touch it. Concretely, a thief unparks the owner *after* it has published a
    /// job's result, and the owner may already have returned from `fork_join` by
    /// then. Dropping the `Worker` in that window is a use-after-free of its
    /// header.
    ///
    /// On bare metal this is free — [`main_loop`](Worker::main_loop) diverges, so a
    /// worker lives as long as its hart. On a hosted target, join every worker
    /// thread before the last `Worker` goes out of scope.
    pub fn new(scheduler: &'a Scheduler, park: Park, stub: &'a Job) -> Self {
        Self {
            header: WorkerHeader::new(park),
            job_head: NonNull::from_ref(stub),
            scheduler,
        }
    }

    /// The flag that hands this worker a heartbeat, i.e. permission to share its
    /// oldest job with someone else the next time it forks.
    ///
    /// It is set from this hart's timer interrupt, which is the only reason any of
    /// this worker has to be reachable from another CPU at all.
    pub fn heartbeat_flag(&self) -> &AtomicBool {
        &self.header.heartbeat
    }

    /// Run shared jobs, and sleep when there are none. Returns once the scheduler
    /// is [`stop`](Scheduler::stop)ped.
    pub fn main_loop(&mut self) {
        self.work_until(|| self.scheduler.is_stopping());
    }

    /// Run `f` with fork/join access to this worker.
    ///
    /// This is how a hart that *has* work enters the scheduler (the equivalent
    /// of upstream's `pool.call`); [`main_loop`](Worker::main_loop) is how a
    /// hart *without* work does.
    pub fn scope<R>(&mut self, f: impl FnOnce(Scope<'_, '_>) -> R) -> R {
        // The list must be empty: the stub is its own tail.
        // Safety: the stub outlives the worker (`Worker::new`'s contract).
        debug_assert!(unsafe { self.job_head.as_ref() }.next.get().is_null());

        let r = f(Scope {
            worker: self,
            job_tail: self.job_head,
        });

        // Everything forked was joined, so it is empty again.
        // Safety: as above.
        debug_assert!(unsafe { self.job_head.as_ref() }.next.get().is_null());

        r
    }

    /// Promote our oldest queued job so another worker can pick it up, and wake one
    /// if any is asleep.
    ///
    /// `&self`: only the owning hart calls this (from [`Scope::fork_join`]), and
    /// everything it touches is a `Cell` behind the owner-only discipline, an
    /// atomic, or lives inside the scheduler's lock.
    #[cold]
    fn heartbeat(&self) {
        let to_wake = {
            let mut synced = self.scheduler.synced.lock();

            // Already advertising a job? Then there is nothing to share: one promoted
            // job per worker, as upstream.
            if self.header.shared_job.load(Ordering::Relaxed).is_null() {
                if let Some(job) = self.shift() {
                    self.header
                        .shared_job
                        .store(job.as_ptr(), Ordering::Relaxed);

                    // The two lists share one set of links, so this must hold — a
                    // worker that is running (and therefore able to promote) has
                    // already taken itself off `idle`.
                    debug_assert!(
                        !self.header.in_idle_list.load(Ordering::Relaxed),
                        "a worker promoting a job cannot also be idle"
                    );
                    synced.shared.push_back(NonNull::from_ref(&self.header));

                    synced.pop_idle()
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Never signal with the lock held: `IrqMutex` keeps interrupts off, and an
        // unpark can be an `ecall` into the SBI.
        if let Some(worker) = to_wake {
            // Safety: workers outlive the scheduler's use of them. See `Worker::new`.
            unsafe { worker.as_ref() }.park.unpark();
        }

        // Publishes nothing, so it needs no ordering: it is the same flag the timer
        // interrupt sets with a relaxed store.
        self.header.heartbeat.store(false, Ordering::Relaxed);
    }

    /// Take a promoted-but-unclaimed job back from the shared set.
    ///
    /// Returns `true` if `job` was still ours to take — nobody claimed it — in
    /// which case its stage still holds the untouched closure and the caller
    /// runs it inline, exactly as if it had never been promoted. That is much
    /// cheaper than the alternative (executing it type-erased via
    /// [`await_shared_job`], often *after* first executing somebody else's
    /// older job), and it is what upstream's `waitForJob` does first.
    #[cold]
    fn reclaim_shared(&self, job: &Job) -> bool {
        let mut synced = self.scheduler.synced.lock();

        // Relaxed: guarded by the lock, like every `shared_job` access.
        if ptr::eq(self.header.shared_job.load(Ordering::Relaxed), job) {
            self.header
                .shared_job
                .store(ptr::null_mut(), Ordering::Relaxed);
            // Safety: a worker whose `shared_job` is non-null is linked into
            // `shared` — the two change together, under this lock.
            unsafe { synced.shared.remove(NonNull::from_ref(&self.header)) };
            true
        } else {
            false
        }
    }

    /// Pull and run shared jobs until `done`; park when there is nothing to pull.
    ///
    /// The entire taker side of the scheduler, shared by its two users:
    /// [`main_loop`](Worker::main_loop) runs it until [`stop`](Scheduler::stop),
    /// and a join whose job was stolen runs it until the thief publishes the
    /// result ([`fork_join`](Scope::fork_join)).
    ///
    /// `done` is checked under the guard, in the same critical section that
    /// cleans up a stale registration and decides between taking work and
    /// registering as idle — so every exit is deregistered and empty-handed,
    /// and a worker can never fall asleep while a job sits in `shared`.
    ///
    /// A `Worker` method on purpose: this is `fork_join`'s cold path, and taking
    /// `&mut Scope` here would make every frame's scope address escape into this
    /// out-of-line call — pinning the scope to a stack slot and undoing the
    /// whole by-value discipline (P2). Taking only the worker keeps the hot
    /// path's scope a pure register value.
    #[inline(never)]
    fn work_until(&self, done: impl Fn() -> bool) {
        loop {
            let work = {
                let mut synced = self.scheduler.synced.lock();

                // Stale-token cleanup: our unpark can land long after an
                // advertise-wake already pulled us off the list (a thief's
                // result token, say). Take ourselves back out before anything
                // else — a worker about to run a job (or return to user code)
                // must not hold a place in `idle`, because running means
                // promoting, and promoting needs these very links.
                //
                // For `stop`: it writes `stopping` under this lock, so seeing
                // `false` in `done` here means it has not drained `idle` yet
                // and is guaranteed to find the node we may push below.
                synced.remove_idle(&self.header);

                if done() {
                    break;
                }

                match Scheduler::take_oldest_shared_work(&mut synced) {
                    Some(work) => Some(work),
                    // Register as idle, decided together with "there is
                    // nothing to pull" under the guard. Being in `idle` means
                    // a heartbeat's advertise now wakes blocked joiners too,
                    // instead of letting available parallelism sleep until the
                    // joiner's own job completes.
                    //
                    // **Unless we are advertising.** A joiner can get here
                    // with an older promotion of its own still unclaimed (the
                    // job being joined was stolen while the slot holds an
                    // older one), and a worker in `shared` must never enter
                    // `idle`: the two lists share one set of links, so pushing
                    // here would silently corrupt both. Checked under this
                    // lock, which is what the slot changes under. Such a
                    // joiner waits on its own token exactly as before — its
                    // wake comes from its thief, or from whoever steals the
                    // advertised job.
                    None => {
                        if self.header.shared_job.load(Ordering::Relaxed).is_null() {
                            synced.push_idle(&self.header);
                        }
                        None
                    }
                }
            };

            if let Some((job, owner)) = work {
                // Our own queued list is empty right now — `main_loop` workers
                // run nothing of their own, and for a joiner the job it awaits
                // was promoted, a promoted job is older than anything queued —
                // so everything older was promoted before it — and everything
                // younger has already been joined. Executing from the head
                // keeps the job's forks reachable by [`shift`](Worker::shift)
                // (upstream's `executeJob`/`begin()` discipline, assert
                // included).
                // Safety: the stub outlives the worker (`Worker::new`'s contract).
                debug_assert!(unsafe { self.job_head.as_ref() }.next.get().is_null());
                let mut scope = Scope {
                    worker: self,
                    job_tail: self.job_head,
                };
                scope.execute_job(job, owner);
            } else {
                // The guard is gone by now, so this never sleeps holding the
                // lock. Whoever wakes us — our thief publishing the result, or
                // a promoter with fresh work — removed us from `idle` first or
                // left a token.
                self.header.park.park();
            }
        }
    }

    /// Unlink and return the oldest queued job, moving it to the promoted state
    /// (its `prev` nulled — what [`Job::is_queued`] tests). Returns `None` if
    /// the list is empty **or** the oldest job is also the (linked) tail: the
    /// tail is what the innermost `fork_join` is about to join, and the join's
    /// cheap pop relies on a joined job being the tail. (The Zig original's
    /// `shift`, tail refusal included.)
    fn shift(&self) -> Option<NonNull<Job>> {
        // Safety: every node in the list was pushed by this hart and is kept
        // alive by its blocked `fork_join` frame (push's contract).
        unsafe {
            let head = self.job_head.as_ref();
            let oldest = head.next.get();
            if oldest.is_null() {
                return None;
            }
            let next = (*oldest).next.get();
            if next.is_null() {
                return None;
            }
            (*next).prev.set(self.job_head.as_ptr());
            head.next.set(next);
            (*oldest).prev.set(ptr::null_mut());
            Some(NonNull::new_unchecked(oldest))
        }
    }
}

/// A unit of work that *could* run on another hart.
///
/// Type-erased: the concrete closure and result types live in the [`TypedJob`] this
/// is the first field of, and `execute` is monomorphized per pair.
///
/// Exactly three words, like the upstream Zig `Job`, with the queued/promoted
/// state packed into `prev`:
///
/// - **queued**: `prev` points at the previous node (possibly the stub), `next`
///   at the following one (or null at the tail);
/// - **promoted/executing**: `prev` is null; `next` is dead.
///
/// So [`is_queued`](Job::is_queued) — the question `fork_join` asks on every
/// join — is a *single* load and null-test, instead of `Links::is_linked`'s two
/// loads and two tests.
///
/// The links are `Cell`s because only the owning hart ever touches them — the
/// pushes and pops inlined into [`Scope::fork_join`], and [`Worker::shift`]. A
/// thief is handed the job only *after* `shift` has unlinked it under the
/// scheduler's lock, and reads nothing but `execute` and the [`TypedJob`]
/// fields around it.
#[derive(Debug)]
pub struct Job {
    execute: unsafe fn(NonNull<Job>, &mut Scope<'_, '_>, &WorkerHeader),
    prev: Cell<*mut Job>,
    next: Cell<*mut Job>,
}

impl Job {
    /// The sentinel a [`Worker`] keeps at the front of its job list. It is never
    /// promoted — [`Worker::shift`] shares the job *after* it — so it is never
    /// executed.
    pub const fn stub() -> Self {
        unsafe fn stub_execute(
            _job: NonNull<Job>,
            _executing_scope: &mut Scope<'_, '_>,
            _owning_worker: &WorkerHeader,
        ) {
            unreachable!("the job list stub was executed")
        }

        Self {
            execute: stub_execute,
            prev: Cell::new(ptr::null_mut()),
            next: Cell::new(ptr::null_mut()),
        }
    }

    /// Still in the owner's [`JobList`], i.e. nobody promoted it. One load.
    #[inline(always)]
    fn is_queued(&self) -> bool {
        !self.prev.get().is_null()
    }
}

/// The per-frame execution context: everything the hot path needs, **by value**.
///
/// Two words — a pointer to the hart's [`Worker`] and the current job-list
/// tail — passed by value into every user function and rebuilt fresh per frame
/// by [`call`]. That is upstream's `callWithContext` discipline, and it is
/// load-bearing: a tail that lives at one memory address (a field) turns every
/// push and pop, at every recursion depth, into a store→load→store round trip
/// through that one address; a per-frame value crosses call boundaries in an
/// argument register and its intra-frame updates are mem2reg'd away entirely.
///
/// `Scope` is deliberately **not `Copy`**: a frame duplicating its scope and
/// forking on both copies would give the two diverging pictures of the same
/// list. Move semantics make that unrepresentable, while `fork_join(&mut self)`
/// on the frame's one binding stays ergonomic.
///
/// Coherence between the frames' copies is the fork/join *balance*: every job a
/// frame pushes is joined before the frame returns, so a callee hands back the
/// list exactly as it found it, and the caller's copy is still correct — with
/// one exception, the promoted-job join, documented in
/// [`fork_join`](Scope::fork_join).
pub struct Scope<'a, 'b> {
    worker: &'a Worker<'b>,
    job_tail: NonNull<Job>,
}

/// Upstream's `callWithContext`: enter user code with a *fresh* context passed
/// **by value** — two words, two argument registers, no frame slot. This is the
/// literal shape of the Zig boundary (`callWithContext(worker, job_tail, …)`),
/// visible in its disassembly as `mov x1, sp` where a by-reference context
/// would store the tail to memory.
#[inline(always)]
fn call<R>(worker: &Worker<'_>, job_tail: NonNull<Job>, f: impl FnOnce(Scope<'_, '_>) -> R) -> R {
    f(Scope { worker, job_tail })
}

impl Scope<'_, '_> {
    /// Fork `b` off as a stealable job, run `a`, then join: either `b` was
    /// never stolen and runs inline (the overwhelmingly common case), or its
    /// result is awaited from whoever ran it.
    ///
    /// # The promoted-job join
    ///
    /// A join that finds its job promoted returns with this frame's tail
    /// pointing at a node [`shift`](Worker::shift) has since detached from the
    /// list. That is deliberate, and harmless: pushes onto a detached node
    /// still form a well-linked chain — joins walk `prev`, which stays intact —
    /// the chain is merely invisible to `shift`, so jobs forked after such a
    /// join cannot be promoted until the stack unwinds past the detached node.
    /// Promotion is only a parallelism hint; correctness never depends on it.
    #[inline(always)]
    pub fn fork_join<A, B, RA, RB>(&mut self, a: A, b: B) -> (RA, RB)
    where
        A: FnOnce(Scope<'_, '_>) -> RA,
        B: FnOnce(Scope<'_, '_>) -> RB + Send,
        RB: Send,
    {
        let job = TypedJob::new(b);

        // Push onto the frame's tail: three stores, no branches. (`job.next` is
        // already null from `TypedJob::new`, and a pushed job is the new tail.)
        //
        // Safety: joined (or awaited) below, on every path, before this frame
        // dies — so the job outlives its time in the list. The tail is the stub
        // or a job in a still-live caller frame.
        unsafe {
            self.job_tail
                .as_ref()
                .next
                .set(ptr::from_ref(&job.job).cast_mut());
            job.job.prev.set(self.job_tail.as_ptr());
        }

        if self.worker.header.heartbeat.load(Ordering::Relaxed) {
            self.worker.heartbeat();
        }

        let ra = call(self.worker, NonNull::from_ref(&job.job), a);

        let prev = job.job.prev.get();
        let rb = if !prev.is_null() {
            // Still queued — nobody promoted it; run it inline. This is the hot
            // path.
            //
            // Pop through the job's *own* `prev`, not this frame's `job_tail`:
            // forks are joined LIFO, so our job is the innermost queued one, but
            // `shift` may have promoted everything older and relinked our `prev`
            // to the head. Popping via `job_tail` would then unlink a detached
            // node and leave `head.next` pointing at this dying frame's job —
            // the next `shift` would walk into a dead stack. Adopting `prev` as
            // the frame's tail is what keeps later forks reachable (upstream's
            // `pop(&task.job_tail)` does exactly this), and it is the one place
            // a callee hands its caller back a *different* list position.
            //
            // Safety: a queued job's `prev` is the stub or a job in a still-live
            // caller frame.
            unsafe {
                (*prev).next.set(ptr::null_mut());
                self.job_tail = NonNull::new_unchecked(prev);
            }

            // Safety: still queued means nobody promoted it, so the stage still
            // holds the closure we put there.
            let b = ManuallyDrop::into_inner(unsafe { job.stage.into_inner().f });
            call(self.worker, self.job_tail, b)
        } else if self.worker.reclaim_shared(&job.job) {
            // Promoted, but nobody claimed it: take it back and run it inline.
            //
            // Safety: reclaimed under the lock, so no thief has touched the
            // stage — the closure is still there.
            let b = ManuallyDrop::into_inner(unsafe { job.stage.into_inner().f });
            call(self.worker, self.job_tail, b)
        } else {
            // Stolen: pull and run other shared jobs until the thief publishes
            // ours. `is_ready` is per-job on purpose: several of our jobs can
            // be in flight at once and finish in any order, so only a flag on
            // the job itself can say *this* one is done — the unpark is only a
            // hint (any of our thieves, or a promoter with fresh work, may
            // have sent it); the flag is what we trust.
            self.worker
                .work_until(|| job.is_ready.load(Ordering::Acquire));

            // Safety: `is_ready` is set, so whoever ran the job replaced the closure
            // with its result and will never touch the stage again.
            ManuallyDrop::into_inner(unsafe { job.stage.into_inner().r })
        };

        (ra, rb)
    }

    fn execute_job(&mut self, job: NonNull<Job>, owner: NonNull<WorkerHeader>) {
        // Safety: the job belongs to a `fork_join` frame that stays blocked until we
        // set its `is_ready`, so it is live for as long as we hold it.
        let j = unsafe { job.as_ref() };
        // Safety: workers outlive the scheduler's use of them. See `Worker::new`.
        let owner = unsafe { owner.as_ref() };

        // Safety: we claimed this job out of `shared` under the lock, so it is ours
        // to run and its stage still holds the closure.
        unsafe {
            (j.execute)(job, self, owner);
        }
    }
}

#[repr(C)]
struct TypedJob<F, R> {
    job: Job,
    is_ready: AtomicBool,
    stage: UnsafeCell<Stage<F, R>>,
}

impl<F, R> fmt::Debug for TypedJob<F, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedJob")
            .field("job", &self.job)
            .field("is_ready", &self.is_ready)
            .field("stage", &"<...>")
            .finish()
    }
}

/// The closure and its result, overlapping in one slot: whoever runs the job moves
/// `F` out, runs it, and writes `R` back over the same bytes. Which one is live
/// follows from `is_ready`, so no discriminant is stored.
#[repr(C)]
union Stage<F, R> {
    f: ManuallyDrop<F>,
    r: ManuallyDrop<R>,
}

impl<F, R> TypedJob<F, R> {
    pub fn new(f: F) -> Self
    where
        F: FnOnce(Scope<'_, '_>) -> R + Send,
        R: Send,
    {
        unsafe fn execute<F, R>(
            job: NonNull<Job>,
            executing_scope: &mut Scope<'_, '_>,
            owning_worker: &WorkerHeader,
        ) where
            F: FnOnce(Scope<'_, '_>) -> R,
        {
            // Are we running our *own* promoted job? `work_until` steals from
            // `shared`, and what it finds may well be the very job it is waiting for.
            // There is nobody to wake in that case — we are who would have been woken
            // — and unparking ourselves would leave a token nobody consumes, which
            // the next `park` would burn on a phantom wakeup.
            let is_own = ptr::eq(owning_worker, &executing_scope.worker.header);

            // Safety: the caller claimed this job from `shared` under the lock, so it
            // is in the executing state and this `TypedJob` frame is still live.
            let job = unsafe { job.cast::<TypedJob<F, R>>().as_ref() };
            // Safety: an executing job's closure has not been taken — only this
            // function takes it, and a job is claimed exactly once.
            let f = unsafe { job.stage.get().cast::<F>().read() };
            // Hand the closure a by-value scope at the executing worker's
            // current position; its forks balance out before it returns.
            let scope = Scope {
                worker: executing_scope.worker,
                job_tail: executing_scope.job_tail,
            };
            // Safety: the stage is ours until `is_ready` below hands it back to
            // the owner, and `f` was read out above, so the slot is dead.
            unsafe {
                job.stage.get().cast::<R>().write(f(scope));
            }

            // Our last touch of the *job*: the `fork_join` frame it lives in may
            // return, and die, the instant this lands. The owner's *header* outlives
            // that frame — it belongs to the `Worker`, which outlives the whole
            // scheduler (see `Worker::new`) — so the unpark below is still sound.
            job.is_ready.store(true, Ordering::Release);

            if !is_own {
                owning_worker.park.unpark();
            }
        }

        Self {
            job: Job {
                execute: execute::<F, R>,
                // `fork_join`'s push overwrites `prev` and relies on `next`
                // starting null (a pushed job is the new tail). A null `prev`
                // is also what makes a never-forked job read as not-queued.
                prev: Cell::new(ptr::null_mut()),
                next: Cell::new(ptr::null_mut()),
            },
            is_ready: AtomicBool::new(false),
            stage: UnsafeCell::new(Stage {
                f: ManuallyDrop::new(f),
            }),
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// Interval at which each worker is handed a heartbeat.
    const HEARTBEAT_INTERVAL: Duration = Duration::from_micros(100);

    struct Node {
        value: i64,
        left: Option<Box<Node>>,
        right: Option<Box<Node>>,
    }

    impl Node {
        /// Parent-first (pre-order) allocation, like the Zig example.
        fn balanced(from: i64, to: i64) -> Box<Node> {
            let value = from + (to - from) / 2;
            let mut node = Box::new(Node {
                value,
                left: None,
                right: None,
            });
            if value > from {
                node.left = Some(Self::balanced(from, value - 1));
            }
            if value < to {
                node.right = Some(Self::balanced(value + 1, to));
            }
            node
        }

        fn sum(&self) -> i64 {
            let mut res = self.value;
            if let Some(child) = &self.left {
                res += child.sum();
            }
            if let Some(child) = &self.right {
                res += child.sum();
            }
            res
        }
    }

    fn sum(mut s: Scope<'_, '_>, node: &Node) -> i64 {
        // Loaded before the forks so the load's latency hides behind the
        // subtree walk; see the note in `examples/binary_tree.rs`.
        let value = node.value;
        match (&node.left, &node.right) {
            (Some(left), Some(right)) => {
                let (l, r) = s.fork_join(|s| sum(s, left), |s| sum(s, right));
                value + l + r
            }
            (Some(child), None) | (None, Some(child)) => value + sum(s, child),
            (None, None) => value,
        }
    }

    fn std_park() -> Park {
        fn park(ptr: *const ()) {
            let me = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<thread::Thread>()) });
            debug_assert_eq!(me.id(), thread::current().id());

            thread::park();
        }
        fn unpark(ptr: *const ()) {
            let me = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<thread::Thread>()) });
            me.unpark();
        }
        fn drop(ptr: *const ()) {
            unsafe { Arc::decrement_strong_count(ptr.cast::<thread::Thread>()) };
        }

        static STD_PARK_VTABLE: ParkVTable = ParkVTable { park, unpark, drop };

        let state = Arc::new(thread::current());
        // Safety: `std::thread::park`/`unpark` is a sticky permit, exactly what
        // `ParkVTable` asks for, and the `Arc` keeps the handle alive until `drop`
        // releases it.
        unsafe { Park::new(Arc::into_raw(state).cast::<()>(), &STD_PARK_VTABLE) }
    }

    /// The trap [`WorkerHeader::in_idle_list`] exists for.
    ///
    /// `Links::is_linked` is `next.is_some() || prev.is_some()`, so the *only* node
    /// in a list reports itself as **unlinked**. A lone idle worker whose `park`
    /// returned on a stale token would therefore fail to take itself back off the
    /// idle list, and register a second time — which `List::push_back` catches with
    /// an assertion, but only once some interleaving actually produces it.
    ///
    /// Nothing here is concurrent: the bug is a state-machine bug, and a race is
    /// only what triggers it. So it is checked deterministically.
    #[test]
    fn a_lone_idle_worker_can_take_itself_back_off_the_list() {
        let sched = Scheduler::new();
        let stub = Job::stub();
        let worker = Worker::new(&sched, std_park(), &stub);

        let mut synced = sched.synced.lock();

        synced.push_idle(&worker.header);
        assert!(worker.header.in_idle_list.load(Ordering::Relaxed));
        // The trap: the sole node in the list still reports itself as unlinked.
        assert!(!worker.header.links.is_linked());

        // `main_loop` coming back from a park nobody dequeued it for. It has to find
        // itself and unregister, or the next pass registers an already-linked node.
        synced.remove_idle(&worker.header);
        assert!(!worker.header.in_idle_list.load(Ordering::Relaxed));

        // Panics inside `List::push_back` if the removal above did not happen.
        synced.push_idle(&worker.header);

        assert!(synced.pop_idle().is_some());
        assert!(synced.pop_idle().is_none());
    }

    #[test]
    fn tree_sum() {
        let harts = thread::available_parallelism().unwrap().get();

        let sched = Scheduler::new();
        let root = Node::balanced(1, 10_000_000);
        let expected = root.sum();

        // On real hardware each hart's timer ISR sets its own CPU-local heartbeat
        // flag. Here one thread plays the timer for every hart, so each worker has to
        // publish the address of its flag first.
        let flags: Mutex<Vec<usize>> = Mutex::new(Vec::new());

        thread::scope(|scope| {
            // The idle harts, running whatever gets shared with them.
            let idle: Vec<_> = (1..harts)
                .map(|_| {
                    scope.spawn(|| {
                        let stub = Job::stub();
                        let mut worker = Worker::new(&sched, std_park(), &stub);
                        flags
                            .lock()
                            .unwrap()
                            .push(ptr::from_ref(worker.heartbeat_flag()) as usize);

                        worker.main_loop();
                    })
                })
                .collect();

            // The "timer interrupt".
            let timer = scope.spawn(|| {
                while !sched.is_stopping() {
                    for &flag in flags.lock().unwrap().iter() {
                        // Safety: every worker outlives the joins below, and this loop
                        // ends once the scheduler is stopping — which is also the only
                        // way a worker returns.
                        unsafe { (*(flag as *const AtomicBool)).store(true, Ordering::Relaxed) };
                    }

                    thread::sleep(HEARTBEAT_INTERVAL);
                }
            });

            // The hart asking for the tree sum. The others only get anything to do
            // because this one's heartbeat shares its jobs.
            let stub = Job::stub();
            let mut worker = Worker::new(&sched, std_park(), &stub);
            flags
                .lock()
                .unwrap()
                .push(ptr::from_ref(worker.heartbeat_flag()) as usize);

            let got = worker.scope(|s| sum(s, &root));
            sched.stop();

            // A worker may not be dropped while another hart can still reach it: a
            // thief unparks its owner *after* publishing the result. Join everyone
            // before `worker` goes out of scope. See `Worker::new`.
            for hart in idle {
                hart.join().unwrap();
            }
            timer.join().unwrap();

            assert_eq!(got, expected);
        });
    }

    /// Promotion detaches a job from the list while its `fork_join` frame still
    /// holds the frame's tail, so later forks — in the same frame, or while
    /// helping — push onto a detached node. With the heartbeat flag held high
    /// continuously, *every* fork promotes, so every join takes the
    /// promoted-job path and (with no other worker to steal) runs its own job
    /// through `work_until`'s "help out" arm — the "empty local list"
    /// path (upstream's `Worker.begin()` assert).
    #[test]
    fn sequential_forks_survive_aggressive_promotion() {
        fn chain(mut s: Scope<'_, '_>, depth: u32) -> i64 {
            if depth == 0 {
                return 1;
            }
            // Two fork_joins per frame: the second pushes onto whatever the
            // first join left the frame's tail pointing at.
            let (a, b) = s.fork_join(|s| chain(s, depth - 1), |_| 1_i64);
            let (c, d) = s.fork_join(|_| 1_i64, |_| 1_i64);
            a + b + c + d
        }

        const DEPTH: u32 = 64;

        let sched = Scheduler::new();
        let stub = Job::stub();
        let mut worker = Worker::new(&sched, std_park(), &stub);
        let flag = ptr::from_ref(worker.heartbeat_flag()) as usize;

        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            // A "timer" that never stops beating: every fork sees the flag set.
            let timer = scope.spawn(|| {
                // Safety: `worker` outlives the join below, and this loop stops
                // before `thread::scope` lets it drop.
                let flag = flag as *const AtomicBool;
                while !done.load(Ordering::Acquire) {
                    unsafe { (*flag).store(true, Ordering::Relaxed) };
                    std::hint::spin_loop();
                }
            });

            let got = worker.scope(|s| chain(s, DEPTH));
            done.store(true, Ordering::Release);
            timer.join().unwrap();

            // Each level adds its own 1 + 1 (second fork_join) + 1 (b); the
            // deepest frame returns 1.
            assert_eq!(got, i64::from(DEPTH) * 3 + 1);
        });
    }
}
