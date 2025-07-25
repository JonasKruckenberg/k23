use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Pack;
use crate::cfg::{self, CfgPrivate};
use crate::clear::Clear;

pub(crate) mod slot;
mod stack;

use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;

pub(crate) use self::slot::Slot;

/// A page address encodes the location of a slot within a shard (the page
/// number and offset within that page) as a single linear value.
#[repr(transparent)]
pub(crate) struct Addr<C: cfg::Config = cfg::DefaultConfig> {
    addr: usize,
    _cfg: PhantomData<fn(C)>,
}

impl<C: cfg::Config> Addr<C> {
    const NULL: usize = Self::BITS + 1;

    pub(crate) fn index(self) -> usize {
        // Since every page is twice as large as the previous page, and all page sizes
        // are powers of two, we can determine the page index that contains a given
        // address by counting leading zeros, which tells us what power of two
        // the offset fits into.
        //
        // First, we must shift down to the smallest page size, so that the last
        // offset on the first page becomes 0.
        let shifted = (self.addr + C::INITIAL_SZ) >> C::ADDR_INDEX_SHIFT;
        // Now, we can  determine the number of twos places by counting the
        // number of leading  zeros (unused twos places) in the number's binary
        // representation, and subtracting that count from the total number of bits in a word.
        usize::BITS as usize - shifted.leading_zeros() as usize
    }

    pub(crate) fn offset(self) -> usize {
        self.addr
    }
}

pub(crate) trait FreeList<C> {
    fn push<T>(&self, new_head: usize, slot: &Slot<T, C>)
    where
        C: cfg::Config;
}

impl<C: cfg::Config> Pack<C> for Addr<C> {
    const LEN: usize = C::MAX_PAGES + C::ADDR_INDEX_SHIFT;

    type Prev = ();

    fn as_usize(&self) -> usize {
        self.addr
    }

    fn from_usize(addr: usize) -> Self {
        debug_assert!(addr <= Self::BITS);
        Self {
            addr,
            _cfg: PhantomData,
        }
    }
}

pub(crate) type Iter<'a, T, C> = core::iter::FilterMap<
    core::slice::Iter<'a, Slot<Option<T>, C>>,
    fn(&'a Slot<Option<T>, C>) -> Option<&'a T>,
>;

pub(crate) struct Local {
    /// Index of the first slot on the local free list
    head: UnsafeCell<usize>,
}

pub(crate) struct Shared<T, C> {
    /// The remote free list
    ///
    /// Slots freed from a remote thread are pushed onto this list.
    remote: stack::TransferStack<C>,
    // Total size of the page.
    //
    // If the head index of the local or remote free list is greater than the size of the
    // page, then that free list is empty. If the head of both free lists is greater than `size`
    // then there are no slots left in that page.
    size: usize,
    prev_sz: usize,
    slab: UnsafeCell<Option<Slots<T, C>>>,
}

type Slots<T, C> = Box<[Slot<T, C>]>;

impl Local {
    pub(crate) fn new() -> Self {
        Self {
            head: UnsafeCell::new(0),
        }
    }

    #[inline(always)]
    fn head(&self) -> usize {
        // Safety: TODO
        unsafe { *self.head.get() }
    }

    #[inline(always)]
    fn set_head(&self, new_head: usize) {
        // Safety: TODO
        unsafe {
            *self.head.get() = new_head;
        }
    }
}

impl<C: cfg::Config> FreeList<C> for Local {
    fn push<T>(&self, new_head: usize, slot: &Slot<T, C>) {
        slot.set_next(self.head());
        self.set_head(new_head);
    }
}

impl<T, C> Shared<T, C>
where
    C: cfg::Config,
{
    const NULL: usize = Addr::<C>::NULL;

    pub(crate) fn new(size: usize, prev_sz: usize) -> Self {
        Self {
            prev_sz,
            size,
            remote: stack::TransferStack::new(),
            slab: UnsafeCell::new(None),
        }
    }

    /// Return the head of the freelist
    ///
    /// If there is space on the local list, it returns the head of the local list. Otherwise, it
    /// pops all the slots from the global list and returns the head of that list
    ///
    /// *Note*: The local list's head is reset when setting the new state in the slot pointed to be
    /// `head` returned from this function
    #[inline]
    fn pop(&self, local: &Local) -> Option<usize> {
        let head = local.head();

        log::trace!("-> local head {:?}", head);

        // are there any items on the local free list? (fast path)
        let head = if head < self.size {
            head
        } else {
            // slow path: if the local free list is empty, pop all the items on
            // the remote free list.
            let head = self.remote.pop_all();

            log::trace!("-> remote head {:?}", head);
            head?
        };

        // if the head is still null, both the local and remote free lists are
        // empty --- we can't fit any more items on this page.
        if head == Self::NULL {
            log::trace!("-> NULL! {:?}", head);
            None
        } else {
            Some(head)
        }
    }

    /// Returns `true` if storage is currently allocated for this page, `false`
    /// otherwise.
    #[inline]
    fn is_unallocated(&self) -> bool {
        // Safety: TODO
        unsafe { (*self.slab.get()).is_none() }
    }

    #[inline]
    pub(crate) fn with_slot<'a, U>(
        &'a self,
        addr: Addr<C>,
        f: impl FnOnce(&'a Slot<T, C>) -> Option<U>,
    ) -> Option<U> {
        let poff = addr.offset() - self.prev_sz;

        log::trace!("-> offset {:?}", poff);

        // Safety: TODO
        let slot = unsafe { &*self.slab.get() }.as_ref()?.get(poff)?;
        f(slot)
    }

    #[inline(always)]
    pub(crate) fn free_list(&self) -> &(impl FreeList<C> + use<T, C>) {
        &self.remote
    }
}

impl<'a, T, C> Shared<Option<T>, C>
where
    C: cfg::Config + 'a,
{
    pub(crate) fn take<F>(
        &self,
        addr: Addr<C>,
        generation: slot::Generation<C>,
        free_list: &F,
    ) -> Option<T>
    where
        F: FreeList<C>,
    {
        let offset = addr.offset() - self.prev_sz;

        log::trace!("-> take: offset {:?}", offset);

        // Safety: TODO
        let slab = unsafe { &*self.slab.get() }.as_ref()?;
        let slot = slab.get(offset)?;
        slot.remove_value(generation, offset, free_list)
    }

    pub(crate) fn remove<F: FreeList<C>>(
        &self,
        addr: Addr<C>,
        generation: slot::Generation<C>,
        free_list: &F,
    ) -> bool {
        let offset = addr.offset() - self.prev_sz;

        log::trace!("-> offset {:?}", offset);

        // Safety: TODO
        let slab = unsafe { &*self.slab.get() }.as_ref();
        if let Some(slot) = slab.and_then(|slab| slab.get(offset)) {
            slot.try_remove_value(generation, offset, free_list)
        } else {
            false
        }
    }

    // Need this function separately, as we need to pass a function pointer to `filter_map` and
    // `Slot::value` just returns a `&T`, specifically a `&Option<T>` for this impl.
    fn make_ref(slot: &'a Slot<Option<T>, C>) -> Option<&'a T> {
        slot.value().as_ref()
    }

    pub(crate) fn iter(&self) -> Option<Iter<'a, T, C>> {
        // Safety: TODO
        let slab = unsafe { (*self.slab.get()).as_ref() };
        slab.map(|slab| {
            slab.iter()
                .filter_map(Shared::make_ref as fn(&'a Slot<Option<T>, C>) -> Option<&'a T>)
        })
    }
}

impl<T, C> Shared<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    pub(crate) fn init_with<U>(
        &self,
        local: &Local,
        init: impl FnOnce(usize, &Slot<T, C>) -> Option<U>,
    ) -> Option<U> {
        let head = self.pop(local)?;

        // do we need to allocate storage for this page?
        if self.is_unallocated() {
            self.allocate();
        }

        let index = head + self.prev_sz;

        let result = {
            // Safety: TODO
            let slab = unsafe { &*(self.slab.get()) }
                .as_ref()
                .expect("page must have been allocated to insert!");
            let slot = &slab[head];
            let result = init(index, slot)?;
            local.set_head(slot.next());
            result
        };

        log::trace!("-> init_with: insert at offset: {}", index);
        Some(result)
    }

    /// Allocates storage for the page's slots.
    #[cold]
    fn allocate(&self) {
        log::trace!("-> alloc new page ({})", self.size);
        debug_assert!(self.is_unallocated());

        let mut slab = Vec::with_capacity(self.size);
        slab.extend((1..self.size).map(Slot::new));
        slab.push(Slot::new(Self::NULL));
        // safety: this mut access is safe — it only occurs to initially allocate the page,
        // which only happens on this thread; if the page has not yet been allocated, other
        // threads will not try to access it yet.
        unsafe {
            *self.slab.get() = Some(slab.into_boxed_slice());
        }
    }

    pub(crate) fn mark_clear<F: FreeList<C>>(
        &self,
        addr: Addr<C>,
        generation: slot::Generation<C>,
        free_list: &F,
    ) -> bool {
        let offset = addr.offset() - self.prev_sz;

        log::trace!("-> offset {:?}", offset);

        // Safety: TODO
        let slab = unsafe { &*self.slab.get() }.as_ref();
        if let Some(slot) = slab.and_then(|slab| slab.get(offset)) {
            slot.try_clear_storage(generation, offset, free_list)
        } else {
            false
        }
    }

    pub(crate) fn clear<F: FreeList<C>>(
        &self,
        addr: Addr<C>,
        generation: slot::Generation<C>,
        free_list: &F,
    ) -> bool {
        let offset = addr.offset() - self.prev_sz;

        log::trace!("-> offset {:?}", offset);

        // Safety: TODO
        let slab = unsafe { &*self.slab.get() }.as_ref();
        if let Some(slot) = slab.and_then(|slab| slab.get(offset)) {
            slot.clear_storage(generation, offset, free_list)
        } else {
            false
        }
    }
}

impl fmt::Debug for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Safety: TODO
        let head = unsafe { *self.head.get() };
        f.debug_struct("Local")
            .field("head", &format_args!("{:#0x}", head))
            .finish()
    }
}

impl<C, T> fmt::Debug for Shared<C, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared")
            .field("remote", &self.remote)
            .field("prev_sz", &self.prev_sz)
            .field("size", &self.size)
            // .field("slab", &self.slab)
            .finish()
    }
}

impl<C: cfg::Config> fmt::Debug for Addr<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Addr")
            .field("addr", &format_args!("{:#0x}", &self.addr))
            .field("index", &self.index())
            .field("offset", &self.offset())
            .finish()
    }
}

impl<C: cfg::Config> PartialEq for Addr<C> {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr
    }
}

impl<C: cfg::Config> Eq for Addr<C> {}

impl<C: cfg::Config> PartialOrd for Addr<C> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<C: cfg::Config> Ord for Addr<C> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.addr.cmp(&other.addr)
    }
}

impl<C: cfg::Config> Clone for Addr<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: cfg::Config> Copy for Addr<C> {}

#[inline(always)]
pub(crate) fn indices<C: cfg::Config>(idx: usize) -> (Addr<C>, usize) {
    let addr = C::unpack_addr(idx);
    (addr, addr.index())
}
