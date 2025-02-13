//! A lock-free concurrent object pool.
//!
//! See the [`Pool` type's documentation][pool] for details on the object pool API and how
//! it differs from the [`Slab`] API.
//!
//! [pool]: ../struct.Pool.html
//! [`Slab`]: ../struct.Slab.html
use crate::{
    cfg::{self, CfgPrivate, DefaultConfig},
    clear::Clear,
    page, shard,
    tid::Tid,
    Pack, Shard,
};

use alloc::sync::Arc;
use core::{fmt, marker::PhantomData};

/// A lock-free concurrent object pool.
///
/// Slabs provide pre-allocated storage for many instances of a single type. But, when working with
/// heap allocated objects, the advantages of a slab are lost, as the memory allocated for the
/// object is freed when the object is removed from the slab. With a pool, we can instead reuse
/// this memory for objects being added to the pool in the future, therefore reducing memory
/// fragmentation and avoiding additional allocations.
///
/// This type implements a lock-free concurrent pool, indexed by `usize`s. The items stored in this
/// type need to implement [`Clear`] and `Default`.
///
/// The `Pool` type shares similar semantics to [`Slab`] when it comes to sharing across threads
/// and storing mutable shared data. The biggest difference is there are no [`Slab::insert`] and
/// [`Slab::take`] analogues for the `Pool` type. Instead new items are added to the pool by using
/// the [`Pool::create`] method, and marked for clearing by the [`Pool::clear`] method.
///
/// # Examples
///
/// Add an entry to the pool, returning an index:
/// ```
/// # use sharded_slab::Pool;
/// let pool: Pool<String> = Pool::new();
///
/// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
/// assert_eq!(pool.get(key).unwrap(), String::from("hello world"));
/// ```
///
/// Create a new pooled item, returning a guard that allows mutable access:
/// ```
/// # use sharded_slab::Pool;
/// let pool: Pool<String> = Pool::new();
///
/// let mut guard = pool.create().unwrap();
/// let key = guard.key();
/// guard.push_str("hello world");
///
/// drop(guard); // release the guard, allowing immutable access.
/// assert_eq!(pool.get(key).unwrap(), String::from("hello world"));
/// ```
///
/// Pool entries can be cleared by calling [`Pool::clear`]. This marks the entry to
/// be cleared when the guards referencing to it are dropped.
/// ```
/// # use sharded_slab::Pool;
/// let pool: Pool<String> = Pool::new();
///
/// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
///
/// // Mark this entry to be cleared.
/// pool.clear(key);
///
/// // The cleared entry is no longer available in the pool
/// assert!(pool.get(key).is_none());
/// ```
/// # Configuration
///
/// Both `Pool` and [`Slab`] share the same configuration mechanism. See [crate level documentation][config-doc]
/// for more details.
///
/// [`Slab::take`]: crate::Slab::take
/// [`Slab::insert`]: crate::Slab::insert
/// [`Pool::create`]: Pool::create
/// [`Pool::clear`]: Pool::clear
/// [config-doc]: crate#configuration
/// [`Clear`]: crate::Clear
/// [`Slab`]: crate::Slab
pub struct Pool<T, C = DefaultConfig>
where
    T: Clear + Default,
    C: cfg::Config,
{
    shards: shard::Array<T, C>,
    _cfg: PhantomData<C>,
}

/// A guard that allows access to an object in a pool.
///
/// While the guard exists, it indicates to the pool that the item the guard references is
/// currently being accessed. If the item is removed from the pool while the guard exists, the
/// removal will be deferred until all guards are dropped.
pub struct Ref<'a, T, C = DefaultConfig>
where
    T: Clear + Default,
    C: cfg::Config,
{
    inner: page::slot::Guard<T, C>,
    shard: &'a Shard<T, C>,
    key: usize,
}

/// A guard that allows exclusive mutable access to an object in a pool.
///
/// While the guard exists, it indicates to the pool that the item the guard
/// references is currently being accessed. If the item is removed from the pool
/// while a guard exists, the removal will be deferred until the guard is
/// dropped. The slot cannot be accessed by other threads while it is accessed
/// mutably.
pub struct RefMut<'a, T, C = DefaultConfig>
where
    T: Clear + Default,
    C: cfg::Config,
{
    inner: page::slot::InitGuard<T, C>,
    shard: &'a Shard<T, C>,
    key: usize,
}

/// An owned guard that allows shared immutable access to an object in a pool.
///
/// While the guard exists, it indicates to the pool that the item the guard references is
/// currently being accessed. If the item is removed from the pool while the guard exists, the
/// removal will be deferred until all guards are dropped.
///
/// Unlike [`Ref`], which borrows the pool, an `OwnedRef` clones the `Arc`
/// around the pool. Therefore, it keeps the pool from being dropped until all
/// such guards have been dropped. This means that an `OwnedRef` may be held for
/// an arbitrary lifetime.
///
///
/// # Examples
///
/// ```
/// # use sharded_slab::Pool;
/// # extern crate alloc;
/// use alloc::sync::Arc;
///
/// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
/// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
///
/// // Look up the created `Key`, returning an `OwnedRef`.
/// let value = pool.clone().get_owned(key).unwrap();
///
/// // Now, the original `Arc` clone of the pool may be dropped, but the
/// // returned `OwnedRef` can still access the value.
/// assert_eq!(value, String::from("hello world"));
/// ```
///
/// Unlike [`Ref`], an `OwnedRef` may be stored in a struct which must live
/// for the `'static` lifetime:
///
/// ```
/// # use sharded_slab::Pool;
/// # extern crate alloc;
/// use sharded_slab::pool::OwnedRef;
/// use alloc::sync::Arc;
///
/// pub struct MyStruct {
///     pool_ref: OwnedRef<String>,
///     // ... other fields ...
/// }
///
/// // Suppose this is some arbitrary function which requires a value that
/// // lives for the 'static lifetime...
/// fn function_requiring_static<T: 'static>(t: &T) {
///     // ... do something extremely important and interesting ...
/// }
///
/// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
/// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
///
/// // Look up the created `Key`, returning an `OwnedRef`.
/// let pool_ref = pool.clone().get_owned(key).unwrap();
/// let my_struct = MyStruct {
///     pool_ref,
///     // ...
/// };
///
/// // We can use `my_struct` anywhere where it is required to have the
/// // `'static` lifetime:
/// function_requiring_static(&my_struct);
/// ```
///
/// [`Ref`]: crate::pool::Ref
pub struct OwnedRef<T, C = DefaultConfig>
where
    T: Clear + Default,
    C: cfg::Config,
{
    inner: page::slot::Guard<T, C>,
    pool: Arc<Pool<T, C>>,
    key: usize,
}

/// An owned guard that allows exclusive, mutable access to an object in a pool.
///
/// An `OwnedRefMut<T>` functions more or less identically to an owned
/// `Box<T>`: it can be passed to functions, stored in structure fields, and
/// borrowed mutably or immutably, and can be owned for arbitrary lifetimes.
/// The difference is that, unlike a `Box<T>`, the memory allocation for the
/// `T` lives in the `Pool`; when an `OwnedRefMut` is created, it may reuse
/// memory that was allocated for a previous pooled object that has been
/// cleared. Additionally, the `OwnedRefMut` may be [downgraded] to an
/// [`OwnedRef`] which may be shared freely, essentially turning the `Box`
/// into an `Arc`.
///
/// This is returned by [`Pool::create_owned`].
///
/// While the guard exists, it indicates to the pool that the item the guard
/// references is currently being accessed. If the item is removed from the pool
/// while the guard exists, theremoval will be deferred until all guards are
/// dropped.
///
/// Unlike [`RefMut`], which borrows the pool, an `OwnedRefMut` clones the `Arc`
/// around the pool. Therefore, it keeps the pool from being dropped until all
/// such guards have been dropped. This means that an `OwnedRefMut` may be held for
/// an arbitrary lifetime.
///
/// # Examples
///
/// ```rust
/// # use sharded_slab::Pool;
/// # extern crate alloc;
/// use core::sync::Arc;
///
/// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
///
/// // Create a new pooled item, returning an owned guard that allows mutable
/// // access to the new item.
/// let mut item = pool.clone().create_owned().unwrap();
/// // Return a key that allows indexing the created item once the guard
/// // has been dropped.
/// let key = item.key();
///
/// // Mutate the item.
/// item.push_str("Hello");
/// // Drop the guard, releasing mutable access to the new item.
/// drop(item);
/// ```
///
/// ```rust
/// # use sharded_slab::Pool;
/// # extern crate alloc;
/// use core::sync::Arc;
///
/// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
///
/// // Create a new item, returning an owned, mutable guard.
/// let mut value = pool.clone().create_owned().unwrap();
///
/// // Now, the original `Arc` clone of the pool may be dropped, but the
/// // returned `OwnedRefMut` can still access the value.
/// drop(pool);
///
/// value.push_str("hello world");
/// assert_eq!(value, String::from("hello world"));
/// ```
///
/// Unlike [`RefMut`], an `OwnedRefMut` may be stored in a struct which must live
/// for the `'static` lifetime:
///
/// ```
/// # use sharded_slab::Pool;
/// # extern crate alloc;
/// use sharded_slab::pool::OwnedRefMut;
/// use core::sync::Arc;
///
/// pub struct MyStruct {
///     pool_ref: OwnedRefMut<String>,
///     // ... other fields ...
/// }
///
/// // Suppose this is some arbitrary function which requires a value that
/// // lives for the 'static lifetime...
/// fn function_requiring_static<T: 'static>(t: &T) {
///     // ... do something extremely important and interesting ...
/// }
///
/// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
///
/// // Create a new item, returning a mutable owned reference.
/// let pool_ref = pool.clone().create_owned().unwrap();
///
/// let my_struct = MyStruct {
///     pool_ref,
///     // ...
/// };
///
/// // We can use `my_struct` anywhere where it is required to have the
/// // `'static` lifetime:
/// function_requiring_static(&my_struct);
/// ```
///
/// [`Pool::create_owned`]: crate::Pool::create_owned
/// [`RefMut`]: crate::pool::RefMut
/// [`OwnedRefMut`]: crate::pool::OwnedRefMut
/// [downgraded]: crate::pool::OwnedRefMut::downgrade
pub struct OwnedRefMut<T, C = DefaultConfig>
where
    T: Clear + Default,
    C: cfg::Config,
{
    inner: page::slot::InitGuard<T, C>,
    pool: Arc<Pool<T, C>>,
    key: usize,
}

impl<T> Pool<T>
where
    T: Clear + Default,
{
    /// Returns a new `Pool` with the default configuration parameters.
    pub fn new() -> Self {
        Self::new_with_config()
    }

    /// Returns a new `Pool` with the provided configuration parameters.
    pub fn new_with_config<C: cfg::Config>() -> Pool<T, C> {
        C::validate();
        Pool {
            shards: shard::Array::new(),
            _cfg: PhantomData,
        }
    }
}

impl<T, C> Pool<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    /// The number of bits in each index which are used by the pool.
    ///
    /// If other data is packed into the `usize` indices returned by
    /// [`Pool::create`], user code is free to use any bits higher than the
    /// `USED_BITS`-th bit freely.
    ///
    /// This is determined by the [`Config`] type that configures the pool's
    /// parameters. By default, all bits are used; this can be changed by
    /// overriding the [`Config::RESERVED_BITS`][res] constant.
    ///
    /// [`Config`]: trait.Config.html
    /// [res]: trait.Config.html#associatedconstant.RESERVED_BITS
    /// [`Slab::insert`]: struct.Slab.html#method.insert
    pub const USED_BITS: usize = C::USED_BITS;

    /// Creates a new object in the pool, returning an [`RefMut`] guard that
    /// may be used to mutate the new object.
    ///
    /// If this function returns `None`, then the shard for the current thread is full and no items
    /// can be added until some are removed, or the maximum number of shards has been reached.
    ///
    /// # Examples
    /// ```rust
    /// # use sharded_slab::Pool;
    /// let pool: Pool<String> = Pool::new();
    ///
    /// // Create a new pooled item, returning a guard that allows mutable
    /// // access to the new item.
    /// let mut item = pool.create().unwrap();
    /// // Return a key that allows indexing the created item once the guard
    /// // has been dropped.
    /// let key = item.key();
    ///
    /// // Mutate the item.
    /// item.push_str("Hello");
    /// // Drop the guard, releasing mutable access to the new item.
    /// drop(item);
    /// ```
    ///
    /// [`RefMut`]: crate::pool::RefMut
    pub fn create(&self) -> Option<RefMut<'_, T, C>> {
        let (tid, shard) = self.shards.current();
        log::trace!("pool: create {:?}", tid);
        let (key, inner) = shard.init_with(|idx, slot| {
            let guard = slot.init()?;
            let generation = guard.generation();
            Some((generation.pack(idx), guard))
        })?;
        Some(RefMut {
            inner,
            key: tid.pack(key),
            shard,
        })
    }

    /// Creates a new object in the pool, returning an [`OwnedRefMut`] guard that
    /// may be used to mutate the new object.
    ///
    /// If this function returns `None`, then the shard for the current thread
    /// is full and no items can be added until some are removed, or the maximum
    /// number of shards has been reached.
    ///
    /// Unlike [`create`], which borrows the pool, this method _clones_ the `Arc`
    /// around the pool if a value exists for the given key. This means that the
    /// returned [`OwnedRefMut`] can be held for an arbitrary lifetime. However,
    /// this method requires that the pool itself be wrapped in an `Arc`.
    ///
    /// An `OwnedRefMut<T>` functions more or less identically to an owned
    /// `Box<T>`: it can be passed to functions, stored in structure fields, and
    /// borrowed mutably or immutably, and can be owned for arbitrary lifetimes.
    /// The difference is that, unlike a `Box<T>`, the memory allocation for the
    /// `T` lives in the `Pool`; when an `OwnedRefMut` is created, it may reuse
    /// memory that was allocated for a previous pooled object that has been
    /// cleared. Additionally, the `OwnedRefMut` may be [downgraded] to an
    /// [`OwnedRef`] which may be shared freely, essentially turning the `Box`
    /// into an `Arc`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// use core::sync::Arc;
    ///
    /// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
    ///
    /// // Create a new pooled item, returning an owned guard that allows mutable
    /// // access to the new item.
    /// let mut item = pool.clone().create_owned().unwrap();
    /// // Return a key that allows indexing the created item once the guard
    /// // has been dropped.
    /// let key = item.key();
    ///
    /// // Mutate the item.
    /// item.push_str("Hello");
    /// // Drop the guard, releasing mutable access to the new item.
    /// drop(item);
    /// ```
    ///
    /// ```rust
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// use core::sync::Arc;
    ///
    /// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
    ///
    /// // Create a new item, returning an owned, mutable guard.
    /// let mut value = pool.clone().create_owned().unwrap();
    ///
    /// // Now, the original `Arc` clone of the pool may be dropped, but the
    /// // returned `OwnedRefMut` can still access the value.
    /// drop(pool);
    ///
    /// value.push_str("hello world");
    /// assert_eq!(value, String::from("hello world"));
    /// ```
    ///
    /// Unlike [`RefMut`], an `OwnedRefMut` may be stored in a struct which must live
    /// for the `'static` lifetime:
    ///
    /// ```
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// use sharded_slab::pool::OwnedRefMut;
    /// use core::sync::Arc;
    ///
    /// pub struct MyStruct {
    ///     pool_ref: OwnedRefMut<String>,
    ///     // ... other fields ...
    /// }
    ///
    /// // Suppose this is some arbitrary function which requires a value that
    /// // lives for the 'static lifetime...
    /// fn function_requiring_static<T: 'static>(t: &T) {
    ///     // ... do something extremely important and interesting ...
    /// }
    ///
    /// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
    ///
    /// // Create a new item, returning a mutable owned reference.
    /// let pool_ref = pool.clone().create_owned().unwrap();
    ///
    /// let my_struct = MyStruct {
    ///     pool_ref,
    ///     // ...
    /// };
    ///
    /// // We can use `my_struct` anywhere where it is required to have the
    /// // `'static` lifetime:
    /// function_requiring_static(&my_struct);
    /// ```
    ///
    /// [`create`]: Pool::create
    /// [`OwnedRef`]: crate::pool::OwnedRef
    /// [`RefMut`]: crate::pool::RefMut
    /// [`OwnedRefMut`]: crate::pool::OwnedRefMut
    /// [downgraded]: crate::pool::OwnedRefMut::downgrade
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn create_owned(self: Arc<Self>) -> Option<OwnedRefMut<T, C>> {
        let (tid, shard) = self.shards.current();
        log::trace!("pool: create_owned {:?}", tid);
        let (inner, key) = shard.init_with(|idx, slot| {
            let inner = slot.init()?;
            let generation = inner.generation();
            Some((inner, tid.pack(generation.pack(idx))))
        })?;
        Some(OwnedRefMut {
            inner,
            pool: self,
            key,
        })
    }

    /// Creates a new object in the pool with the provided initializer,
    /// returning a key that may be used to access the new object.
    ///
    /// If this function returns `None`, then the shard for the current thread is full and no items
    /// can be added until some are removed, or the maximum number of shards has been reached.
    ///
    /// # Examples
    /// ```rust
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// let pool: Pool<String> = Pool::new();
    ///
    /// // Create a new pooled item, returning its integer key.
    /// let key = pool.create_with(|s| s.push_str("Hello")).unwrap();
    /// // Other threads may now (immutably) access the item using the key.
    /// ```
    pub fn create_with(&self, init: impl FnOnce(&mut T)) -> Option<usize> {
        log::trace!("pool: create_with");
        let mut guard = self.create()?;
        init(&mut guard);
        Some(guard.key())
    }

    /// Return a borrowed reference to the value associated with the given key.
    ///
    /// If the pool does not contain a value for the given key, `None` is returned instead.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use sharded_slab::Pool;
    /// let pool: Pool<String> = Pool::new();
    /// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
    ///
    /// assert_eq!(pool.get(key).unwrap(), String::from("hello world"));
    /// assert!(pool.get(12345).is_none());
    /// ```
    pub fn get(&self, key: usize) -> Option<Ref<'_, T, C>> {
        let tid = C::unpack_tid(key);

        log::trace!("pool: get{:?}; current={:?}", tid, Tid::<C>::current());
        let shard = self.shards.get(tid.as_usize())?;
        let inner = shard.with_slot(key, |slot| slot.get(C::unpack_gen(key)))?;
        Some(Ref { inner, shard, key })
    }

    /// Return an owned reference to the value associated with the given key.
    ///
    /// If the pool does not contain a value for the given key, `None` is
    /// returned instead.
    ///
    /// Unlike [`get`], which borrows the pool, this method _clones_ the `Arc`
    /// around the pool if a value exists for the given key. This means that the
    /// returned [`OwnedRef`] can be held for an arbitrary lifetime. However,
    /// this method requires that the pool itself be wrapped in an `Arc`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// use core::sync::Arc;
    ///
    /// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
    /// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
    ///
    /// // Look up the created `Key`, returning an `OwnedRef`.
    /// let value = pool.clone().get_owned(key).unwrap();
    ///
    /// // Now, the original `Arc` clone of the pool may be dropped, but the
    /// // returned `OwnedRef` can still access the value.
    /// assert_eq!(value, String::from("hello world"));
    /// ```
    ///
    /// Unlike [`Ref`], an `OwnedRef` may be stored in a struct which must live
    /// for the `'static` lifetime:
    ///
    /// ```
    /// # use sharded_slab::Pool;
    /// # extern crate alloc;
    /// use sharded_slab::pool::OwnedRef;
    /// use core::sync::Arc;
    ///
    /// pub struct MyStruct {
    ///     pool_ref: OwnedRef<String>,
    ///     // ... other fields ...
    /// }
    ///
    /// // Suppose this is some arbitrary function which requires a value that
    /// // lives for the 'static lifetime...
    /// fn function_requiring_static<T: 'static>(t: &T) {
    ///     // ... do something extremely important and interesting ...
    /// }
    ///
    /// let pool: Arc<Pool<String>> = Arc::new(Pool::new());
    /// let key = pool.create_with(|item| item.push_str("hello world")).unwrap();
    ///
    /// // Look up the created `Key`, returning an `OwnedRef`.
    /// let pool_ref = pool.clone().get_owned(key).unwrap();
    /// let my_struct = MyStruct {
    ///     pool_ref,
    ///     // ...
    /// };
    ///
    /// // We can use `my_struct` anywhere where it is required to have the
    /// // `'static` lifetime:
    /// function_requiring_static(&my_struct);
    /// ```
    ///
    /// [`get`]: Pool::get
    /// [`OwnedRef`]: crate::pool::OwnedRef
    /// [`Ref`]: crate::pool::Ref
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn get_owned(self: Arc<Self>, key: usize) -> Option<OwnedRef<T, C>> {
        let tid = C::unpack_tid(key);

        log::trace!("pool: get{:?}; current={:?}", tid, Tid::<C>::current());
        let shard = self.shards.get(tid.as_usize())?;
        let inner = shard.with_slot(key, |slot| slot.get(C::unpack_gen(key)))?;
        Some(OwnedRef {
            inner,
            pool: self.clone(),
            key,
        })
    }

    /// Remove the value using the storage associated with the given key from the pool, returning
    /// `true` if the value was removed.
    ///
    /// This method does _not_ block the current thread until the value can be
    /// cleared. Instead, if another thread is currently accessing that value, this marks it to be
    /// cleared by that thread when it is done accessing that value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use sharded_slab::Pool;
    /// let pool: Pool<String> = Pool::new();
    ///
    /// // Check out an item from the pool.
    /// let mut item = pool.create().unwrap();
    /// let key = item.key();
    /// item.push_str("hello world");
    /// drop(item);
    ///
    /// assert_eq!(pool.get(key).unwrap(), String::from("hello world"));
    ///
    /// pool.clear(key);
    /// assert!(pool.get(key).is_none());
    /// ```
    ///
    /// ```
    /// # use sharded_slab::Pool;
    /// let pool: Pool<String> = Pool::new();
    ///
    /// let key = pool.create_with(|item| item.push_str("Hello world!")).unwrap();
    ///
    /// // Clearing a key that doesn't exist in the `Pool` will return `false`
    /// assert_eq!(pool.clear(key + 69420), false);
    ///
    /// // Clearing a key that does exist returns `true`
    /// assert!(pool.clear(key));
    ///
    /// // Clearing a key that has previously been cleared will return `false`
    /// assert_eq!(pool.clear(key), false);
    /// ```
    /// [`clear`]: #method.clear
    pub fn clear(&self, key: usize) -> bool {
        let tid = C::unpack_tid(key);

        let shard = self.shards.get(tid.as_usize());
        if tid.is_current() {
            shard
                .map(|shard| shard.mark_clear_local(key))
                .unwrap_or(false)
        } else {
            shard
                .map(|shard| shard.mark_clear_remote(key))
                .unwrap_or(false)
        }
    }
}

// Safety: TODO
unsafe impl<T, C> Send for Pool<T, C>
where
    T: Send + Clear + Default,
    C: cfg::Config,
{
}

// Safety: TODO
unsafe impl<T, C> Sync for Pool<T, C>
where
    T: Sync + Clear + Default,
    C: cfg::Config,
{
}

impl<T> Default for Pool<T>
where
    T: Clear + Default,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, C> fmt::Debug for Pool<T, C>
where
    T: fmt::Debug + Clear + Default,
    C: cfg::Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("shards", &self.shards)
            .field("config", &C::debug())
            .finish()
    }
}

// === impl Ref ===

impl<T, C> Ref<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    /// Returns the key used to access this guard
    pub fn key(&self) -> usize {
        self.key
    }

    #[inline]
    fn value(&self) -> &T {
        // Safety: calling `slot::Guard::value` is unsafe, since the `Guard`
        // value contains a pointer to the slot that may outlive the slab
        // containing that slot. Here, the `Ref` has a borrowed reference to
        // the shard containing that slot, which ensures that the slot will
        // not be dropped while this `Guard` exists.
        unsafe { self.inner.value() }
    }
}

impl<T, C> core::ops::Deref for Ref<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

impl<T, C> Drop for Ref<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn drop(&mut self) {
        log::trace!("drop Ref: try clearing data");
        // Safety: calling `slot::Guard::release` is unsafe, since the
        // `Guard` value contains a pointer to the slot that may outlive the
        // slab containing that slot. Here, the `Ref` guard owns a
        // borrowed reference to the shard containing that slot, which
        // ensures that the slot will not be dropped while this `Ref`
        // exists.
        let should_clear = unsafe { self.inner.release() };
        if should_clear {
            self.shard.clear_after_release(self.key);
        }
    }
}

impl<T, C> fmt::Debug for Ref<'_, T, C>
where
    T: fmt::Debug + Clear + Default,
    C: cfg::Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.value(), f)
    }
}

impl<T, C> PartialEq<T> for Ref<'_, T, C>
where
    T: PartialEq<T> + Clear + Default,
    C: cfg::Config,
{
    fn eq(&self, other: &T) -> bool {
        *self.value() == *other
    }
}

// === impl GuardMut ===

impl<'a, T, C: cfg::Config> RefMut<'a, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    /// Returns the key used to access the guard.
    pub fn key(&self) -> usize {
        self.key
    }

    /// Downgrades the mutable guard to an immutable guard, allowing access to
    /// the pooled value from other threads.
    pub fn downgrade(mut self) -> Ref<'a, T, C> {
        // Safety: This method consumes self
        let inner = unsafe { self.inner.downgrade() };
        Ref {
            inner,
            shard: self.shard,
            key: self.key,
        }
    }

    #[inline]
    fn value(&self) -> &T {
        // Safety: we are holding a reference to the shard which keeps the
        // pointed slot alive. The returned reference will not outlive
        // `self`.
        unsafe { self.inner.value() }
    }
}

impl<T, C: cfg::Config> core::ops::Deref for RefMut<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

impl<T, C> core::ops::DerefMut for RefMut<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: we are holding a reference to the shard which keeps the
        // pointed slot alive. The returned reference will not outlive `self`.
        unsafe { self.inner.value_mut() }
    }
}

impl<T, C> Drop for RefMut<'_, T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn drop(&mut self) {
        log::trace!(" -> drop RefMut: try clearing data");
        // Safety: we are holding a reference to the shard which keeps the
        // pointed slot alive. The returned reference will not outlive `self`.
        let should_clear = unsafe { self.inner.release() };
        if should_clear {
            self.shard.clear_after_release(self.key);
        }
    }
}

impl<T, C> fmt::Debug for RefMut<'_, T, C>
where
    T: fmt::Debug + Clear + Default,
    C: cfg::Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.value(), f)
    }
}

impl<T, C> PartialEq<T> for RefMut<'_, T, C>
where
    T: PartialEq<T> + Clear + Default,
    C: cfg::Config,
{
    fn eq(&self, other: &T) -> bool {
        self.value().eq(other)
    }
}

// === impl OwnedRef ===

impl<T, C> OwnedRef<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    /// Returns the key used to access this guard
    pub fn key(&self) -> usize {
        self.key
    }

    #[inline]
    fn value(&self) -> &T {
        // Safety: calling `slot::Guard::value` is unsafe, since the `Guard`
        // value contains a pointer to the slot that may outlive the slab
        // containing that slot. Here, the `Ref` has a borrowed reference to
        // the shard containing that slot, which ensures that the slot will
        // not be dropped while this `Guard` exists.
        unsafe { self.inner.value() }
    }
}

impl<T, C> core::ops::Deref for OwnedRef<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

impl<T, C> Drop for OwnedRef<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn drop(&mut self) {
        log::trace!("drop OwnedRef: try clearing data");
        // Safety: calling `slot::Guard::release` is unsafe, since the
        // `Guard` value contains a pointer to the slot that may outlive the
        // slab containing that slot. Here, the `OwnedRef` owns an `Arc`
        // clone of the pool, which keeps it alive as long as the `OwnedRef`
        // exists.
        let should_clear = unsafe { self.inner.release() };
        if should_clear {
            let shard_idx = Tid::<C>::from_packed(self.key);
            log::trace!("-> shard={:?}", shard_idx);
            if let Some(shard) = self.pool.shards.get(shard_idx.as_usize()) {
                shard.clear_after_release(self.key);
            } else {
                log::trace!("-> shard={:?} does not exist! THIS IS A BUG", shard_idx);
                // debug_assert!(panic_unwind::panicking(), "[internal error] tried to drop an `OwnedRef` to a slot on a shard that never existed!");
            }
        }
    }
}

impl<T, C> fmt::Debug for OwnedRef<T, C>
where
    T: fmt::Debug + Clear + Default,
    C: cfg::Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.value(), f)
    }
}

impl<T, C> PartialEq<T> for OwnedRef<T, C>
where
    T: PartialEq<T> + Clear + Default,
    C: cfg::Config,
{
    fn eq(&self, other: &T) -> bool {
        *self.value() == *other
    }
}

// Safety: TODO
unsafe impl<T, C> Sync for OwnedRef<T, C>
where
    T: Sync + Clear + Default,
    C: cfg::Config,
{
}

// Safety: TODO
unsafe impl<T, C> Send for OwnedRef<T, C>
where
    T: Sync + Clear + Default,
    C: cfg::Config,
{
}

// === impl OwnedRefMut ===

impl<T, C> OwnedRefMut<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    /// Returns the key used to access this guard
    pub fn key(&self) -> usize {
        self.key
    }

    /// Downgrades the owned mutable guard to an owned immutable guard, allowing
    /// access to the pooled value from other threads.
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn downgrade(mut self) -> OwnedRef<T, C> {
        // Safety: this method consumes self
        let inner = unsafe { self.inner.downgrade() };
        OwnedRef {
            inner,
            pool: self.pool.clone(),
            key: self.key,
        }
    }

    fn shard(&self) -> Option<&Shard<T, C>> {
        let shard_idx = Tid::<C>::from_packed(self.key);
        log::trace!("-> shard={:?}", shard_idx);
        self.pool.shards.get(shard_idx.as_usize())
    }

    #[inline]
    fn value(&self) -> &T {
        // Safety: calling `slot::InitGuard::value` is unsafe, since the `Guard`
        // value contains a pointer to the slot that may outlive the slab
        // containing that slot. Here, the `OwnedRefMut` has an `Arc` clone of
        // the shard containing that slot, which ensures that the slot will
        // not be dropped while this `Guard` exists.
        unsafe { self.inner.value() }
    }
}

impl<T, C> core::ops::Deref for OwnedRefMut<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

impl<T, C> core::ops::DerefMut for OwnedRefMut<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: calling `slot::InitGuard::value_mut` is unsafe, since the
        // `Guard`  value contains a pointer to the slot that may outlive
        // the slab   containing that slot. Here, the `OwnedRefMut` has an
        // `Arc` clone of the shard containing that slot, which ensures that
        // the slot will not be dropped while this `Guard` exists.
        unsafe { self.inner.value_mut() }
    }
}

impl<T, C> Drop for OwnedRefMut<T, C>
where
    T: Clear + Default,
    C: cfg::Config,
{
    fn drop(&mut self) {
        log::trace!("drop OwnedRefMut: try clearing data");
        // Safety: calling `slot::Guard::release` is unsafe, since the
        // `Guard` value contains a pointer to the slot that may outlive the
        // slab containing that slot. Here, the `OwnedRefMut` owns an `Arc`
        // clone of the pool, which keeps it alive as long as the
        // `OwnedRefMut` exists.
        let should_clear = unsafe { self.inner.release() };
        if should_clear {
            if let Some(shard) = self.shard() {
                shard.clear_after_release(self.key);
            } else {
                log::trace!("-> shard does not exist! THIS IS A BUG");
                // debug_assert!(panic_unwind::panicking(), "[internal error] tried to drop an `OwnedRefMut` to a slot on a shard that never existed!");
            }
        }
    }
}

impl<T, C> fmt::Debug for OwnedRefMut<T, C>
where
    T: fmt::Debug + Clear + Default,
    C: cfg::Config,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.value(), f)
    }
}

impl<T, C> PartialEq<T> for OwnedRefMut<T, C>
where
    T: PartialEq<T> + Clear + Default,
    C: cfg::Config,
{
    fn eq(&self, other: &T) -> bool {
        *self.value() == *other
    }
}

// Safety: TODO
unsafe impl<T, C> Sync for OwnedRefMut<T, C>
where
    T: Sync + Clear + Default,
    C: cfg::Config,
{
}

// Safety: TODO
unsafe impl<T, C> Send for OwnedRefMut<T, C>
where
    T: Sync + Clear + Default,
    C: cfg::Config,
{
}
