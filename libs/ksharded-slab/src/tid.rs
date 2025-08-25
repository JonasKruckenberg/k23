use alloc::collections::VecDeque;
use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

use cpu_local::cpu_local;
use spin::{LazyLock, Mutex};

use crate::cfg::{self, CfgPrivate};
use crate::{Pack, page};

/// Uniquely identifies a thread.
pub(crate) struct Tid<C> {
    id: usize,
    _not_send: PhantomData<UnsafeCell<()>>,
    _cfg: PhantomData<fn(C)>,
}

#[derive(Debug)]
struct Registration(Cell<Option<usize>>);

struct Registry {
    next: AtomicUsize,
    free: Mutex<VecDeque<usize>>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    next: AtomicUsize::new(0),
    free: Mutex::new(VecDeque::new()),
});

cpu_local! {
    static REGISTRATION: Registration = Registration::new();
}

// === impl Tid ===

impl<C: cfg::Config> Pack<C> for Tid<C> {
    const LEN: usize = C::MAX_SHARDS.trailing_zeros() as usize + 1;

    type Prev = page::Addr<C>;

    #[inline(always)]
    fn as_usize(&self) -> usize {
        self.id
    }

    #[inline(always)]
    fn from_usize(id: usize) -> Self {
        Self {
            id,
            _not_send: PhantomData,
            _cfg: PhantomData,
        }
    }
}

impl<C: cfg::Config> Tid<C> {
    #[inline]
    pub(crate) fn current() -> Self {
        REGISTRATION.current()
    }

    pub(crate) fn is_current(self) -> bool {
        self == REGISTRATION.current::<C>()
    }

    #[inline(always)]
    pub fn new(id: usize) -> Self {
        Self::from_usize(id)
    }
}

impl<C> fmt::Debug for Tid<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Tid")
            .field(&format_args!("{}", self.id))
            .finish()
    }
}

impl<C> PartialEq for Tid<C> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<C> Eq for Tid<C> {}

impl<C: cfg::Config> Clone for Tid<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: cfg::Config> Copy for Tid<C> {}

// === impl Registration ===

impl Registration {
    const fn new() -> Self {
        Self(Cell::new(None))
    }

    #[inline(always)]
    fn current<C: cfg::Config>(&self) -> Tid<C> {
        if let Some(tid) = self.0.get().map(Tid::new) {
            return tid;
        }

        self.register()
    }

    #[cold]
    fn register<C: cfg::Config>(&self) -> Tid<C> {
        let mut free = REGISTRY.free.lock();

        let id = if free.len() > 1 {
            free.pop_front()
        } else {
            None
        };

        let id = id.unwrap_or_else(|| {
            let id = REGISTRY.next.fetch_add(1, Ordering::AcqRel);

            assert!(
                id <= Tid::<C>::BITS,
                "creating a new thread ID ({}) would exceed the \
                    maximum number of thread ID bits specified in {} \
                    ({})",
                id,
                core::any::type_name::<C>(),
                Tid::<C>::BITS
            );

            id
        });

        self.0.set(Some(id));
        Tid::new(id)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(id) = self.0.get() {
            let mut free_list = REGISTRY.free.lock();
            free_list.push_back(id);
        }
    }
}
