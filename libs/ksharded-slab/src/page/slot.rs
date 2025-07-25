use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{fmt, mem, ptr};

use spin::Backoff;

use super::FreeList;
use crate::clear::Clear;
use crate::{Pack, Tid, cfg};

pub(crate) struct Slot<T, C> {
    lifecycle: AtomicUsize,
    /// The offset of the next item on the free list.
    next: UnsafeCell<usize>,
    /// The data stored in the slot.
    item: UnsafeCell<T>,
    _cfg: PhantomData<fn(C)>,
}

#[derive(Debug)]
pub(crate) struct Guard<T, C: cfg::Config = cfg::DefaultConfig> {
    slot: ptr::NonNull<Slot<T, C>>,
}

#[derive(Debug)]
pub(crate) struct InitGuard<T, C: cfg::Config = cfg::DefaultConfig> {
    slot: ptr::NonNull<Slot<T, C>>,
    curr_lifecycle: usize,
    released: bool,
}

#[repr(transparent)]
pub(crate) struct Generation<C = cfg::DefaultConfig> {
    value: usize,
    _cfg: PhantomData<fn(C)>,
}

#[repr(transparent)]
pub(crate) struct RefCount<C = cfg::DefaultConfig> {
    value: usize,
    _cfg: PhantomData<fn(C)>,
}

pub(crate) struct Lifecycle<C> {
    state: State,
    _cfg: PhantomData<fn(C)>,
}
struct LifecycleGen<C>(Generation<C>);

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
#[repr(usize)]
enum State {
    Present = 0b00,
    Marked = 0b01,
    Removing = 0b11,
}

impl<C: cfg::Config> Pack<C> for Generation<C> {
    /// Use all the remaining bits in the word for the generation counter, minus
    /// any bits reserved by the user.
    const LEN: usize = (usize::BITS as usize - C::RESERVED_BITS) - Self::SHIFT;

    type Prev = Tid<C>;

    #[inline(always)]
    fn from_usize(u: usize) -> Self {
        debug_assert!(u <= Self::BITS);
        Self::new(u)
    }

    #[inline(always)]
    fn as_usize(&self) -> usize {
        self.value
    }
}

impl<C: cfg::Config> Generation<C> {
    fn new(value: usize) -> Self {
        Self {
            value,
            _cfg: PhantomData,
        }
    }
}

// Slot methods which should work across all trait bounds
impl<T, C> Slot<T, C>
where
    C: cfg::Config,
{
    #[inline(always)]
    pub(super) fn next(&self) -> usize {
        // Safety: TODO
        unsafe { *self.next.get() }
    }

    #[inline(always)]
    pub(crate) fn value(&self) -> &T {
        // Safety: TODO
        unsafe { &*self.item.get() }
    }

    #[inline(always)]
    pub(super) fn set_next(&self, next: usize) {
        // Safety: TODO
        unsafe {
            *self.next.get() = next;
        }
    }

    #[inline(always)]
    pub(crate) fn get(&self, generation: Generation<C>) -> Option<Guard<T, C>> {
        let mut lifecycle = self.lifecycle.load(Ordering::Acquire);
        loop {
            // Unpack the current state.
            let state = Lifecycle::<C>::from_packed(lifecycle);
            let current_gen = LifecycleGen::<C>::from_packed(lifecycle).0;
            let refs = RefCount::<C>::from_packed(lifecycle);

            log::trace!(
                "-> get {:?}; current_gen={:?}; lifecycle={:#x}; state={:?}; refs={:?};",
                generation,
                current_gen,
                lifecycle,
                state,
                refs,
            );

            // Is it okay to access this slot? The accessed generation must be
            // current, and the slot must not be in the process of being
            // removed. If we can no longer access the slot at the given
            // generation, return `None`.
            if generation != current_gen || state != Lifecycle::PRESENT {
                log::trace!("-> get: no longer exists!");
                return None;
            }

            // Try to increment the slot's ref count by one.
            let new_refs = refs.incr()?;
            match self.lifecycle.compare_exchange(
                lifecycle,
                new_refs.pack(lifecycle),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    log::trace!("-> {:?}", new_refs);
                    return Some(Guard {
                        slot: ptr::NonNull::from(self),
                    });
                }
                Err(actual) => {
                    // Another thread modified the slot's state before us! We
                    // need to retry with the new state.
                    //
                    // Since the new state may mean that the accessed generation
                    // is no longer valid, we'll check again on the next
                    // iteration of the loop.
                    log::trace!("-> get: retrying; lifecycle={:#x};", actual);
                    lifecycle = actual;
                }
            };
        }
    }

    /// Marks this slot to be released, returning `true` if the slot can be
    /// mutated *now* and `false` otherwise.
    ///
    /// This method checks if there are any references to this slot. If there _are_ valid
    /// references, it just marks them for modification and returns and the next thread calling
    /// either `clear_storage` or `remove_value` will try and modify the storage
    fn mark_release(&self, generation: Generation<C>) -> Option<bool> {
        let mut lifecycle = self.lifecycle.load(Ordering::Acquire);
        let mut curr_gen;

        // Try to advance the slot's state to "MARKED", which indicates that it
        // should be removed when it is no longer concurrently accessed.
        loop {
            curr_gen = LifecycleGen::from_packed(lifecycle).0;
            log::trace!(
                "-> mark_release; gen={:?}; current_gen={:?};",
                generation,
                curr_gen
            );

            // Is the slot still at the generation we are trying to remove?
            if generation != curr_gen {
                return None;
            }

            let state = Lifecycle::<C>::from_packed(lifecycle).state;
            log::trace!("-> mark_release; state={:?};", state);
            match state {
                State::Removing => {
                    log::trace!("--> mark_release; cannot release (already removed!)");
                    return None;
                }
                State::Marked => {
                    log::trace!("--> mark_release; already marked;");
                    break;
                }
                State::Present => {}
            };

            // Set the new state to `MARKED`.
            let new_lifecycle = Lifecycle::<C>::MARKED.pack(lifecycle);
            log::trace!(
                "-> mark_release; old_lifecycle={:#x}; new_lifecycle={:#x};",
                lifecycle,
                new_lifecycle
            );

            match self.lifecycle.compare_exchange(
                lifecycle,
                new_lifecycle,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => {
                    log::trace!("-> mark_release; retrying");
                    lifecycle = actual;
                }
            }
        }

        // Unpack the current reference count to see if we can remove the slot now.
        let refs = RefCount::<C>::from_packed(lifecycle);
        log::trace!("-> mark_release: marked; refs={:?};", refs);

        // Are there currently outstanding references to the slot? If so, it
        // will have to be removed when those references are dropped.
        Some(refs.value == 0)
    }

    /// Mutates this slot.
    ///
    /// This method spins until no references to this slot are left, and calls the mutator
    fn release_with<F, M, R>(
        &self,
        generation: Generation<C>,
        offset: usize,
        free: &F,
        mutator: M,
    ) -> R
    where
        F: FreeList<C>,
        M: FnOnce(Option<&mut T>) -> R,
    {
        let mut lifecycle = self.lifecycle.load(Ordering::Acquire);
        let mut advanced = false;
        let mut boff = Backoff::new();

        let next_gen = generation.advance();
        loop {
            let current_gen = LifecycleGen::from_packed(lifecycle).0;
            log::trace!(
                "-> release_with; lifecycle={:#x}; expected_gen={:?}; current_gen={:?}; next_gen={:?};",
                lifecycle,
                generation,
                current_gen,
                next_gen
            );

            // First, make sure we are actually able to remove the value.
            // If we're going to remove the value, the generation has to match
            // the value that `remove_value` was called with...unless we've
            // already stored the new generation.
            if (!advanced) && generation != current_gen {
                log::trace!("-> already removed!");
                return mutator(None);
            }

            match self.lifecycle.compare_exchange(
                lifecycle,
                LifecycleGen(next_gen).pack(lifecycle),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(actual) => {
                    // If we're in this state, we have successfully advanced to
                    // the next generation.
                    advanced = true;

                    // Make sure that there are no outstanding references.
                    let refs = RefCount::<C>::from_packed(actual);
                    log::trace!("-> advanced gen; lifecycle={:#x}; refs={:?};", actual, refs);
                    if refs.value == 0 {
                        log::trace!("-> ok to remove!");
                        // safety: we've modified the generation of this slot and any other thread
                        // calling this method will exit out at the generation check above in the
                        // next iteraton of the loop.
                        let value = mutator(Some(unsafe { &mut *self.item.get() }));
                        free.push(offset, self);
                        return value;
                    }

                    // Otherwise, a reference must be dropped before we can
                    // remove the value. Spin here until there are no refs remaining...
                    log::trace!("-> refs={:?}; spin...", refs);

                    // Back off
                    boff.spin();
                }
                Err(actual) => {
                    log::trace!("-> retrying; lifecycle={:#x};", actual);
                    lifecycle = actual;
                    // The state changed; reset the spin backoff.
                    boff.reset();
                }
            }
        }
    }

    /// Initialize a slot
    ///
    /// This method initializes and sets up the state for a slot. When being used in `Pool`, we
    /// only need to ensure that the `Slot` is in the right `state, while when being used in a
    /// `Slab` we want to insert a value into it, as the memory is not initialized
    pub(crate) fn init(&self) -> Option<InitGuard<T, C>> {
        // Load the current lifecycle state.
        let lifecycle = self.lifecycle.load(Ordering::Acquire);
        let generation = LifecycleGen::<C>::from_packed(lifecycle).0;
        let refs = RefCount::<C>::from_packed(lifecycle);

        log::trace!(
            "-> initialize_state; state={:?}; gen={:?}; refs={:?};",
            Lifecycle::<C>::from_packed(lifecycle),
            generation,
            refs,
        );

        if refs.value != 0 {
            log::trace!("-> initialize while referenced! cancelling");
            return None;
        }

        Some(InitGuard {
            slot: ptr::NonNull::from(self),
            curr_lifecycle: lifecycle,
            released: false,
        })
    }
}

// Slot impl which _needs_ an `Option` for self.item, this is for `Slab` to use.
impl<T, C> Slot<Option<T>, C>
where
    C: cfg::Config,
{
    fn is_empty(&self) -> bool {
        // Safety: TODO
        unsafe { (*self.item.get()).is_none() }
    }

    /// Insert a value into a slot
    ///
    /// We first initialize the state and then insert the passed in value into the slot.
    #[inline]
    pub(crate) fn insert(&self, value: &mut Option<T>) -> Option<Generation<C>> {
        debug_assert!(self.is_empty(), "inserted into full slot");
        debug_assert!(value.is_some(), "inserted twice");

        let mut guard = self.init()?;
        let generation = guard.generation();
        // Safety: Accessing the value of an `InitGuard` is unsafe because
        // it has a pointer to a slot which may dangle. Here, we know the
        // pointed slot is alive because we have a reference to it in scope,
        // and the `InitGuard` will be dropped when this function returns.
        unsafe {
            mem::swap(guard.value_mut(), value);
            guard.release();
        };
        log::trace!("-> inserted at {:?}", generation);

        Some(generation)
    }

    /// Tries to remove the value in the slot, returning `true` if the value was
    /// removed.
    ///
    /// This method tries to remove the value in the slot. If there are existing references, then
    /// the slot is marked for removal and the next thread calling either this method or
    /// `remove_value` will do the work instead.
    #[inline]
    pub(super) fn try_remove_value<F: FreeList<C>>(
        &self,
        generation: Generation<C>,
        offset: usize,
        free: &F,
    ) -> bool {
        // If `mark_release` returns `Some`, a value exists at this
        // generation. The bool inside this option indicates whether or not
        // _we're_ allowed to remove the value.
        let Some(should_remove) = self.mark_release(generation) else {
            // Otherwise, the generation we tried to remove has already expired,
            // and we did not mark anything for removal.
            log::trace!(
                "-> try_remove_value; nothing exists at generation={:?}",
                generation
            );
            return false;
        };

        log::trace!("-> try_remove_value; marked!");

        if should_remove {
            // We're allowed to remove the slot now!
            log::trace!("-> try_remove_value; can remove now");
            self.remove_value(generation, offset, free);
        }

        true
    }

    #[inline]
    pub(super) fn remove_value<F: FreeList<C>>(
        &self,
        generation: Generation<C>,
        offset: usize,
        free: &F,
    ) -> Option<T> {
        self.release_with(generation, offset, free, |item| item.and_then(Option::take))
    }
}

// These impls are specific to `Pool`
impl<T, C> Slot<T, C>
where
    T: Default + Clear,
    C: cfg::Config,
{
    pub(in crate::page) fn new(next: usize) -> Self {
        Self {
            lifecycle: AtomicUsize::new(Lifecycle::<C>::REMOVING.as_usize()),
            item: UnsafeCell::new(T::default()),
            next: UnsafeCell::new(next),
            _cfg: PhantomData,
        }
    }

    /// Try to clear this slot's storage
    ///
    /// If there are references to this slot, then we mark this slot for clearing and let the last
    /// thread do the work for us.
    #[inline]
    pub(super) fn try_clear_storage<F: FreeList<C>>(
        &self,
        generation: Generation<C>,
        offset: usize,
        free: &F,
    ) -> bool {
        // If `mark_release` returns `Some`, a value exists at this
        // generation. The bool inside this option indicates whether or not
        // _we're_ allowed to clear the value.
        let Some(should_clear) = self.mark_release(generation) else {
            // Otherwise, the generation we tried to remove has already expired,
            // and we did not mark anything for removal.
            log::trace!(
                "-> try_clear_storage; nothing exists at generation={:?}",
                generation
            );
            return false;
        };

        log::trace!("-> try_clear_storage; marked!");

        if should_clear {
            // We're allowed to remove the slot now!
            log::trace!("-> try_remove_value; can clear now");
            return self.clear_storage(generation, offset, free);
        }

        true
    }

    /// Clear this slot's storage
    ///
    /// This method blocks until all references have been dropped and clears the storage.
    pub(super) fn clear_storage<F: FreeList<C>>(
        &self,
        generation: Generation<C>,
        offset: usize,
        free: &F,
    ) -> bool {
        // release_with will _always_ wait until it can release the slot or just return if the slot
        // has already been released.
        self.release_with(generation, offset, free, |item| {
            let cleared = item.map(|inner| Clear::clear(inner)).is_some();
            log::trace!("-> cleared: {}", cleared);
            cleared
        })
    }
}

impl<T, C: cfg::Config> Slot<T, C> {
    fn release(&self) -> bool {
        let mut lifecycle = self.lifecycle.load(Ordering::Acquire);
        loop {
            let refs = RefCount::<C>::from_packed(lifecycle);
            let state = Lifecycle::<C>::from_packed(lifecycle).state;
            let generation = LifecycleGen::<C>::from_packed(lifecycle).0;

            // Are we the last guard, and is the slot marked for removal?
            let dropping = refs.value == 1 && state == State::Marked;
            let new_lifecycle = if dropping {
                // If so, we want to advance the state to "removing".
                // Also, reset the ref count to 0.
                LifecycleGen(generation).pack(State::Removing as usize)
            } else {
                // Otherwise, just subtract 1 from the ref count.
                refs.decr().pack(lifecycle)
            };

            log::trace!(
                "-> drop guard: state={:?}; gen={:?}; refs={:?}; lifecycle={:#x}; new_lifecycle={:#x}; dropping={:?}",
                state,
                generation,
                refs,
                lifecycle,
                new_lifecycle,
                dropping
            );
            match self.lifecycle.compare_exchange(
                lifecycle,
                new_lifecycle,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    log::trace!("-> drop guard: done;  dropping={:?}", dropping);
                    return dropping;
                }
                Err(actual) => {
                    log::trace!("-> drop guard; retry, actual={:#x}", actual);
                    lifecycle = actual;
                }
            }
        }
    }
}

impl<T, C: cfg::Config> fmt::Debug for Slot<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lifecycle = self.lifecycle.load(Ordering::Relaxed);
        f.debug_struct("Slot")
            .field("lifecycle", &format_args!("{:#x}", lifecycle))
            .field("state", &Lifecycle::<C>::from_packed(lifecycle).state)
            .field("gen", &LifecycleGen::<C>::from_packed(lifecycle).0)
            .field("refs", &RefCount::<C>::from_packed(lifecycle))
            .field("next", &self.next())
            .finish_non_exhaustive()
    }
}

// === impl Generation ===

impl<C> fmt::Debug for Generation<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Generation").field(&self.value).finish()
    }
}

impl<C: cfg::Config> Generation<C> {
    fn advance(self) -> Self {
        Self::from_usize((self.value + 1) % Self::BITS)
    }
}

impl<C: cfg::Config> PartialEq for Generation<C> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<C: cfg::Config> Eq for Generation<C> {}

impl<C: cfg::Config> PartialOrd for Generation<C> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<C: cfg::Config> Ord for Generation<C> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<C: cfg::Config> Clone for Generation<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: cfg::Config> Copy for Generation<C> {}

// === impl Guard ===

impl<T, C: cfg::Config> Guard<T, C> {
    /// Releases the guard, returning `true` if the slot should be cleared.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `Guard` does not outlive the slab that contains
    /// the pointed slot. Failure to do so means this pointer may dangle.
    #[inline]
    pub(crate) unsafe fn release(&self) -> bool {
        // Safety: ensured by caller
        unsafe { self.slot() }.release()
    }

    /// Returns a borrowed reference to the slot.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `Guard` does not outlive the slab that contains
    /// the pointed slot. Failure to do so means this pointer may dangle.
    #[inline]
    pub(crate) unsafe fn slot(&self) -> &Slot<T, C> {
        // Safety: ensured by caller
        unsafe { self.slot.as_ref() }
    }

    /// Returns a borrowed reference to the slot's value.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `Guard` does not outlive the slab that contains
    /// the pointed slot. Failure to do so means this pointer may dangle.
    #[inline(always)]
    pub(crate) unsafe fn value(&self) -> &T {
        // Safety: ensured by caller
        unsafe { &*self.slot().item.get() }
    }
}

// === impl Lifecycle ===

impl<C: cfg::Config> Lifecycle<C> {
    const MARKED: Self = Self {
        state: State::Marked,
        _cfg: PhantomData,
    };
    const REMOVING: Self = Self {
        state: State::Removing,
        _cfg: PhantomData,
    };
    const PRESENT: Self = Self {
        state: State::Present,
        _cfg: PhantomData,
    };
}

impl<C: cfg::Config> Pack<C> for Lifecycle<C> {
    const LEN: usize = 2;
    type Prev = ();

    fn from_usize(u: usize) -> Self {
        Self {
            state: match u & Self::MASK {
                0b00 => State::Present,
                0b01 => State::Marked,
                0b11 => State::Removing,
                bad => unreachable!("weird lifecycle {:#b}", bad),
            },
            _cfg: PhantomData,
        }
    }

    fn as_usize(&self) -> usize {
        self.state as usize
    }
}

impl<C> PartialEq for Lifecycle<C> {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

impl<C> Eq for Lifecycle<C> {}

impl<C> fmt::Debug for Lifecycle<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Lifecycle").field(&self.state).finish()
    }
}

// === impl RefCount ===

impl<C: cfg::Config> Pack<C> for RefCount<C> {
    const LEN: usize = usize::BITS as usize - (Lifecycle::<C>::LEN + Generation::<C>::LEN);
    type Prev = Lifecycle<C>;

    fn from_usize(value: usize) -> Self {
        debug_assert!(value <= Self::BITS);
        Self {
            value,
            _cfg: PhantomData,
        }
    }

    fn as_usize(&self) -> usize {
        self.value
    }
}

impl<C: cfg::Config> RefCount<C> {
    pub(crate) const MAX: usize = Self::BITS - 1;

    #[inline]
    fn incr(self) -> Option<Self> {
        if self.value >= Self::MAX {
            log::trace!("-> get: {}; MAX={}", self.value, RefCount::<C>::MAX);
            return None;
        }

        Some(Self::from_usize(self.value + 1))
    }

    #[inline]
    fn decr(self) -> Self {
        Self::from_usize(self.value - 1)
    }
}

impl<C> fmt::Debug for RefCount<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RefCount").field(&self.value).finish()
    }
}

impl<C: cfg::Config> PartialEq for RefCount<C> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<C: cfg::Config> Eq for RefCount<C> {}

impl<C: cfg::Config> PartialOrd for RefCount<C> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<C: cfg::Config> Ord for RefCount<C> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<C: cfg::Config> Clone for RefCount<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: cfg::Config> Copy for RefCount<C> {}

// === impl LifecycleGen ===

impl<C: cfg::Config> Pack<C> for LifecycleGen<C> {
    const LEN: usize = Generation::<C>::LEN;
    type Prev = RefCount<C>;

    fn from_usize(value: usize) -> Self {
        Self(Generation::from_usize(value))
    }

    fn as_usize(&self) -> usize {
        self.0.as_usize()
    }
}

impl<T, C: cfg::Config> InitGuard<T, C> {
    pub(crate) fn generation(&self) -> Generation<C> {
        LifecycleGen::<C>::from_packed(self.curr_lifecycle).0
    }

    /// Returns a borrowed reference to the slot's value.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `InitGuard` does not outlive the slab that
    /// contains the pointed slot. Failure to do so means this pointer may
    /// dangle.
    pub(crate) unsafe fn value(&self) -> &T {
        // Safety: ensured by caller
        unsafe { &*self.slot.as_ref().item.get() }
    }

    /// Returns a mutably borrowed reference to the slot's value.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `InitGuard` does not outlive the slab that
    /// contains the pointed slot. Failure to do so means this pointer may
    /// dangle.
    ///
    /// It's safe to reference the slot mutably, though, because creating an
    /// `InitGuard` ensures there are no outstanding immutable references.
    pub(crate) unsafe fn value_mut(&mut self) -> &mut T {
        // Safety: ensured by caller
        unsafe { &mut *self.slot.as_ref().item.get() }
    }

    /// Releases the guard, returning `true` if the slot should be cleared.
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `InitGuard` does not outlive the slab that
    /// contains the pointed slot. Failure to do so means this pointer may
    /// dangle.
    pub(crate) unsafe fn release(&mut self) -> bool {
        // Safety: ensured by caller
        unsafe { self.release2(0) }
    }

    /// Downgrades the guard to an immutable guard
    ///
    /// ## Safety
    ///
    /// This dereferences a raw pointer to the slot. The caller is responsible
    /// for ensuring that the `InitGuard` does not outlive the slab that
    /// contains the pointed slot. Failure to do so means this pointer may
    /// dangle.
    pub(crate) unsafe fn downgrade(&mut self) -> Guard<T, C> {
        // Safety: ensured by caller
        let _ = unsafe { self.release2(RefCount::<C>::from_usize(1).pack(0)) };
        Guard { slot: self.slot }
    }

    unsafe fn release2(&mut self, new_refs: usize) -> bool {
        log::trace!(
            "InitGuard::release; curr_lifecycle={:?}; downgrading={}",
            Lifecycle::<C>::from_packed(self.curr_lifecycle),
            new_refs != 0,
        );
        if self.released {
            log::trace!("-> already released!");
            return false;
        }
        self.released = true;
        let mut curr_lifecycle = self.curr_lifecycle;
        // Safety: ensured by caller
        let slot = unsafe { self.slot.as_ref() };
        let new_lifecycle = LifecycleGen::<C>::from_packed(self.curr_lifecycle)
            .pack(Lifecycle::<C>::PRESENT.pack(new_refs));

        match slot.lifecycle.compare_exchange(
            curr_lifecycle,
            new_lifecycle,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                log::trace!("--> advanced to PRESENT; done");
                return false;
            }
            Err(actual) => {
                log::trace!(
                    "--> lifecycle changed; actual={:?}",
                    Lifecycle::<C>::from_packed(actual)
                );
                curr_lifecycle = actual;
            }
        }

        // if the state was no longer the prior state, we are now responsible
        // for releasing the slot.
        loop {
            let refs = RefCount::<C>::from_packed(curr_lifecycle);
            let state = Lifecycle::<C>::from_packed(curr_lifecycle).state;

            log::trace!(
                "-> InitGuard::release; lifecycle={:#x}; state={:?}; refs={:?};",
                curr_lifecycle,
                state,
                refs,
            );

            debug_assert!(
                state == State::Marked,
                "state was not MARKED; someone else has removed the slot while we have exclusive access!\nactual={:?}",
                state
            );
            debug_assert!(
                refs.value == 0,
                "ref count was not 0; someone else has referenced the slot while we have exclusive access!\nactual={:?}",
                refs
            );

            let new_lifecycle = LifecycleGen(self.generation()).pack(State::Removing as usize);

            match slot.lifecycle.compare_exchange(
                curr_lifecycle,
                new_lifecycle,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    log::trace!("-> InitGuard::RELEASE: done!");
                    return true;
                }
                Err(actual) => {
                    log::trace!("-> InitGuard::release; retry, actual={:#x}", actual);
                    // panic!("we should not have to retry this CAS!");
                    curr_lifecycle = actual;
                }
            }
        }
    }
}
