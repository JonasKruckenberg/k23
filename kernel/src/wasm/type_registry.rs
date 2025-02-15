use crate::wasm::indices::{
    CanonicalizedTypeIndex, ModuleInternedTypeIndex, RecGroupRelativeTypeIndex, VMSharedTypeIndex,
};
use crate::wasm::translate::{ModuleTypes, WasmRecGroup, WasmSubType};
use crate::wasm::Engine;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt;
use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use core::ops::Range;
use core::sync::atomic::Ordering::Acquire;
use core::sync::atomic::{AtomicUsize, Ordering};
use cranelift_entity::{iter_entity_range, PrimaryMap, SecondaryMap};
use hashbrown::HashSet;
use sync::RwLock;
use wasmtime_slab::Slab;

pub trait TypeTrace {
    /// Visit each edge.
    ///
    /// The function can break out of tracing by returning `Err(E)`.
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>;

    /// Visit each edge, mutably.
    ///
    /// Allows updating edges.
    ///
    /// The function can break out of tracing by returning `Err(E)`.
    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>;

    fn canonicalize_for_runtime_usage<F>(&mut self, module_to_engine: &mut F)
    where
        F: FnMut(ModuleInternedTypeIndex) -> VMSharedTypeIndex,
    {
        self.trace_mut::<_, ()>(&mut |idx| match *idx {
            CanonicalizedTypeIndex::Shared(_) => Ok(()),
            CanonicalizedTypeIndex::RecGroup(_) => unreachable!(),
            CanonicalizedTypeIndex::Module(module_index) => {
                let index = module_to_engine(module_index);
                *idx = CanonicalizedTypeIndex::Shared(index);
                Ok(())
            }
        })
        .unwrap();
    }

    fn is_canonicalized_for_runtime_usage(&self) -> bool {
        self.trace(&mut |idx| match idx {
            CanonicalizedTypeIndex::Shared(_) => Ok(()),
            CanonicalizedTypeIndex::Module(_) | CanonicalizedTypeIndex::RecGroup(_) => Err(()),
        })
        .is_ok()
    }

    /// Canonicalize `self` by rewriting all type references inside `self` from
    /// module-level interned type indices to either engine-level interned type
    /// indices or recgroup-relative indices.
    fn canonicalize_for_hash_consing<F>(
        &mut self,
        rec_group_range: Range<ModuleInternedTypeIndex>,
        module_to_engine: &mut F,
    ) where
        F: FnMut(ModuleInternedTypeIndex) -> VMSharedTypeIndex,
    {
        self.trace_mut::<_, ()>(&mut |idx| {
            match *idx {
                CanonicalizedTypeIndex::Shared(_) => Ok(()),
                CanonicalizedTypeIndex::RecGroup(_) => unreachable!(),
                CanonicalizedTypeIndex::Module(module_index) => {
                    *idx = if rec_group_range.start <= module_index {
                        // Any module index within the recursion group gets
                        // translated into a recgroup-relative index.
                        debug_assert!(module_index < rec_group_range.end);
                        let relative = module_index.as_u32() - rec_group_range.start.as_u32();
                        let relative = RecGroupRelativeTypeIndex::from_u32(relative);
                        CanonicalizedTypeIndex::RecGroup(relative)
                    } else {
                        // Cross-group indices are translated directly into
                        // `VMSharedTypeIndex`es.
                        debug_assert!(module_index < rec_group_range.start);
                        CanonicalizedTypeIndex::Shared(module_to_engine(module_index))
                    };
                    Ok(())
                }
            }
        })
        .unwrap();
    }

    /// Is this type canonicalized for hash consing?
    fn is_canonicalized_for_hash_consing(&self) -> bool {
        self.trace(&mut |idx| match idx {
            CanonicalizedTypeIndex::Shared(_) | CanonicalizedTypeIndex::RecGroup(_) => Ok(()),
            CanonicalizedTypeIndex::Module(_) => Err(()),
        })
        .is_ok()
    }
}

#[derive(Debug)]
pub struct RuntimeTypeCollection {
    engine: Engine,
    rec_groups: Vec<RecGroupEntry>,
    types: PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex>,
}

impl RuntimeTypeCollection {
    /// Gets the map from `ModuleInternedTypeIndex` to `VMSharedTypeIndex`
    pub fn type_map(&self) -> &PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex> {
        &self.types
    }

    /// Look up a shared type index by its module type index
    #[inline]
    pub fn lookup_shared_type(&self, index: ModuleInternedTypeIndex) -> Option<VMSharedTypeIndex> {
        self.types.get(index).copied()
    }
}

impl Drop for RuntimeTypeCollection {
    fn drop(&mut self) {
        if !self.rec_groups.is_empty() {
            self.engine
                .type_registry()
                .0
                .write()
                .unregister_type_collection(self);
        }
    }
}

pub struct RegisteredType {
    engine: Engine,
    entry: RecGroupEntry,
    ty: Arc<WasmSubType>,
    index: VMSharedTypeIndex,
}

impl RegisteredType {
    pub fn index(&self) -> VMSharedTypeIndex {
        self.index
    }
}

impl Debug for RegisteredType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RegisteredType {
            engine: _,
            entry: _,
            ty,
            index,
        } = self;
        f.debug_struct("RegisteredType")
            .field("index", index)
            .field("ty", ty)
            .finish_non_exhaustive()
    }
}

impl Clone for RegisteredType {
    fn clone(&self) -> Self {
        self.entry.incr_ref_count("cloning RegisteredType");
        RegisteredType {
            engine: self.engine.clone(),
            entry: self.entry.clone(),
            ty: self.ty.clone(),
            index: self.index,
        }
    }
}

impl Drop for RegisteredType {
    fn drop(&mut self) {
        if self.entry.decr_ref_count("dropping RegisteredType") {
            self.engine
                .type_registry()
                .0
                .write()
                .unregister_entry(self.entry.clone());
        }
    }
}

impl core::ops::Deref for RegisteredType {
    type Target = WasmSubType;

    fn deref(&self) -> &Self::Target {
        &self.ty
    }
}

impl PartialEq for RegisteredType {
    fn eq(&self, other: &Self) -> bool {
        let eq = Arc::ptr_eq(&self.entry.0, &other.entry.0);

        if cfg!(debug_assertions) {
            if eq {
                assert!(Engine::same(&self.engine, &other.engine));
                assert_eq!(self.ty, other.ty);
            } else {
                assert!(self.ty != other.ty || !Engine::same(&self.engine, &other.engine));
            }
        }

        eq
    }
}

impl Eq for RegisteredType {}

impl Hash for RegisteredType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let ptr = Arc::as_ptr(&self.entry.0);
        ptr.hash(state);
    }
}

#[derive(Debug, Default)]
pub struct TypeRegistry(RwLock<TypeRegistryInner>);

impl TypeRegistry {
    /// Creates a new shared type registry.
    pub fn new() -> Self {
        Self::default()
    }

    #[expect(tail_expr_drop_order, reason = "")]
    pub fn register_module_types(
        &self,
        engine: &Engine,
        types: ModuleTypes,
    ) -> RuntimeTypeCollection {
        let (rec_groups, types) = self.0.write().register_module_types(types);

        RuntimeTypeCollection {
            engine: engine.clone(),
            rec_groups,
            types,
        }
    }

    #[expect(tail_expr_drop_order, reason = "")]
    pub fn get_type(&self, engine: &Engine, index: VMSharedTypeIndex) -> Option<RegisteredType> {
        let id = shared_type_index_to_slab_id(index);
        let inner = self.0.read();

        let ty = inner.types.get(id)?.clone();
        let entry = inner.type_to_rec_group[index].clone().unwrap();

        entry.incr_ref_count("TypeRegistry::get_type");

        debug_assert!(entry.0.registrations.load(Acquire) != 0);
        Some(RegisteredType {
            engine: engine.clone(),
            entry,
            ty,
            index,
        })
    }
}

#[inline]
fn shared_type_index_to_slab_id(index: VMSharedTypeIndex) -> wasmtime_slab::Id {
    wasmtime_slab::Id::from_raw(index.as_u32())
}

#[inline]
fn slab_id_to_shared_type_index(id: wasmtime_slab::Id) -> VMSharedTypeIndex {
    VMSharedTypeIndex::from_u32(id.into_raw())
}

#[derive(Debug, Default)]
struct TypeRegistryInner {
    hash_consing_map: HashSet<RecGroupEntry>,
    type_to_rec_group: SecondaryMap<VMSharedTypeIndex, Option<RecGroupEntry>>,
    types: Slab<Arc<WasmSubType>>,
    // An explicit stack of entries that we are in the middle of dropping. Used
    // to avoid recursion when dropping a type that is holding the last
    // reference to another type, etc...
    drop_stack: Vec<RecGroupEntry>,
}

impl TypeRegistryInner {
    fn register_module_types(
        &mut self,
        types: ModuleTypes,
    ) -> (
        Vec<RecGroupEntry>,
        PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex>,
    ) {
        let mut entries = Vec::with_capacity(types.rec_groups().len());
        let mut map = PrimaryMap::<ModuleInternedTypeIndex, VMSharedTypeIndex>::with_capacity(
            types.wasm_types().len(),
        );

        for module_group in types.rec_groups() {
            let entry = self.register_rec_group(
                &map,
                module_group.clone(),
                iter_entity_range(module_group.clone())
                    .map(|ty| types.get_wasm_type(ty).unwrap().clone()),
            );

            for (module_ty, engine_ty) in
                iter_entity_range(module_group.clone()).zip(entry.0.shared_type_indices.iter())
            {
                let module_ty2 = map.push(*engine_ty);
                assert_eq!(module_ty, module_ty2);
            }

            entries.push(entry);
        }
        (entries, map)
    }

    fn register_rec_group(
        &mut self,
        map: &PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex>,
        range: Range<ModuleInternedTypeIndex>,
        types: impl ExactSizeIterator<Item = WasmSubType>,
    ) -> RecGroupEntry {
        debug_assert_eq!(iter_entity_range(range.clone()).len(), types.len());

        let mut non_canon_types = Vec::with_capacity(types.len());
        let hash_consing_key = WasmRecGroup(
            types
                .zip(iter_entity_range(range.clone()))
                .map(|(mut ty, module_index)| {
                    non_canon_types.push((module_index, ty.clone()));
                    ty.canonicalize_for_hash_consing(range.clone(), &mut |idx| {
                        debug_assert!(idx < range.clone().start);
                        map[idx]
                    });
                    ty
                })
                .collect::<Box<[_]>>(),
        );

        if let Some(entry) = self.hash_consing_map.get(&hash_consing_key) {
            entry.incr_ref_count(
                "hash consed to already-registered type in `TypeRegistryInner::register_rec_group`",
            );
            return entry.clone();
        }

        // increase the ref of referenced groups, they must remain alive as
        // long as this rec group lives.
        hash_consing_key
            .trace::<_, ()>(&mut |index| {
                if let CanonicalizedTypeIndex::Shared(index) = index {
                    let entry = self.type_to_rec_group[index].as_ref().unwrap();
                    entry.incr_ref_count(
                        "new cross-group type reference to existing type in `register_rec_group`",
                    );
                }
                Ok(())
            })
            .unwrap();

        // Register the individual types.
        // This will also canonicalize them for runtime use
        let module_rec_group_start = range.start;
        let engine_rec_group_start = u32::try_from(self.types.len()).unwrap();
        let engine_type_indices: Box<[_]> = non_canon_types
            .into_iter()
            .map(|(module_index, mut ty)| {
                ty.canonicalize_for_runtime_usage(&mut |idx| {
                    if idx < module_rec_group_start {
                        map[idx]
                    } else {
                        let rec_group_offset = idx.as_u32() + module_rec_group_start.as_u32();
                        VMSharedTypeIndex::from_u32(engine_rec_group_start + rec_group_offset)
                    }
                });
                self.insert_one_type_from_rec_group(module_index, ty)
            })
            .collect();
        let entry = RecGroupEntry(Arc::new(RecGroupEntryInner {
            hash_consing_key,
            shared_type_indices: engine_type_indices,
            registrations: AtomicUsize::new(1),
        }));
        tracing::trace!("create new entry {entry:?} (registrations -> 1)");

        let is_new_entry = self.hash_consing_map.insert(entry.clone());
        debug_assert!(is_new_entry);

        // Now that we've constructed the entry, we can update the reverse
        // type-to-rec-group map.
        for ty in entry.0.shared_type_indices.iter().copied() {
            debug_assert!(self.type_to_rec_group[ty].is_none());
            self.type_to_rec_group[ty] = Some(entry.clone());
        }

        entry
    }

    fn insert_one_type_from_rec_group(
        &mut self,
        module_index: ModuleInternedTypeIndex,
        ty: WasmSubType,
    ) -> VMSharedTypeIndex {
        debug_assert!(
            ty.is_canonicalized_for_runtime_usage(),
            "type is not canonicalized for runtime usage: {ty:?}"
        );

        // Add the type to our slab.
        let id = self.types.alloc(Arc::new(ty));
        let engine_index = slab_id_to_shared_type_index(id);
        tracing::trace!(
            "registered type {module_index:?} as {engine_index:?} = {:?}",
            &self.types[id]
        );

        engine_index
    }

    fn unregister_type_collection(&mut self, collection: &RuntimeTypeCollection) {
        for entry in &collection.rec_groups {
            if entry.decr_ref_count("TypeRegistryInner::unregister_type_collection") {
                self.unregister_entry(entry.clone());
            }
        }
    }

    /// Remove a zero-refcount entry from the registry.
    ///
    /// This does *not* decrement the entry's registration count, it should
    /// instead be invoked only after a previous decrement operation observed
    /// zero remaining registrations.
    fn unregister_entry(&mut self, entry: RecGroupEntry) {
        debug_assert!(self.drop_stack.is_empty());
        self.drop_stack.push(entry);

        while let Some(entry) = self.drop_stack.pop() {
            tracing::trace!("Start unregistering {entry:?}");

            // We need to double check whether the entry is still at zero
            // registrations: Between the time that we observed a zero and
            // acquired the lock to call this function, another thread could
            // have registered the type and found the 0-registrations entry in
            // `self.map` and incremented its count.
            //
            // We don't need to worry about any concurrent increments during
            // this function's invocation after we check for zero because we
            // have exclusive access to `&mut self` and therefore no one can
            // create a new reference to this entry and bring it back to life.
            let registrations = entry.0.registrations.load(Acquire);
            if registrations != 0 {
                tracing::trace!(
                    "{entry:?} was concurrently resurrected and no longer has \
                     zero registrations (registrations -> {registrations})",
                );
                continue;
            }

            // Decrement any other types that this type was shallowly
            // (i.e. non-transitively) referencing and keeping alive. If this
            // was the last thing keeping them registered, its okay to
            // unregister them as well now.
            debug_assert!(entry.0.hash_consing_key.is_canonicalized_for_hash_consing());
            entry
                .0
                .hash_consing_key
                .trace::<_, ()>(&mut |other_index| {
                    if let CanonicalizedTypeIndex::Shared(other_index) = other_index {
                        let other_entry = self.type_to_rec_group[other_index].as_ref().unwrap();
                        if other_entry.decr_ref_count(
                            "referenced by dropped entry in \
                         `TypeCollection::unregister_entry`",
                        ) {
                            self.drop_stack.push(other_entry.clone());
                        }
                    }

                    Ok(())
                })
                .unwrap();

            // Remove the entry from the hash-consing map. If we register a
            // duplicate definition of this rec group again in the future, it
            // will be as if it is the first time it has ever been registered,
            // and it will be inserted into the hash-consing map again at that
            // time.
            self.hash_consing_map.remove(&entry);

            // Similarly, remove the rec group's types from the registry, as
            // well as their entries from the reverse type-to-rec-group
            // map.
            for index in entry.0.shared_type_indices.iter().copied() {
                tracing::trace!("removing {index:?} from registry");

                let removed_entry = self.type_to_rec_group[index].take();
                debug_assert_eq!(removed_entry.unwrap(), entry);

                let id = shared_type_index_to_slab_id(index);
                self.types.dealloc(id);
            }

            tracing::trace!("End unregistering {entry:?}");
        }
    }
}

// `TypeRegistryInner` implements `Drop` in debug builds to assert that
// all types have been unregistered for the registry.
#[cfg(debug_assertions)]
impl Drop for TypeRegistryInner {
    fn drop(&mut self) {
        tracing::trace!("Dropping type registry: {self:#?}");
        let TypeRegistryInner {
            hash_consing_map,
            types,
            type_to_rec_group,
            drop_stack,
        } = self;
        assert!(
            hash_consing_map.is_empty(),
            "type registry not empty: hash consing map is not empty: {hash_consing_map:#?}"
        );
        assert!(
            types.is_empty(),
            "type registry not empty: types slab is not empty: {types:#?}"
        );
        assert!(
            type_to_rec_group.is_empty() || type_to_rec_group.values().all(Option::is_none),
            "type registry not empty: type-to-rec-group map is not empty: {type_to_rec_group:#?}"
        );
        assert!(
            drop_stack.is_empty(),
            "type registry not empty: drop stack is not empty: {drop_stack:#?}"
        );
    }
}

#[derive(Debug, Clone)]
struct RecGroupEntry(Arc<RecGroupEntryInner>);

#[derive(Debug)]
struct RecGroupEntryInner {
    hash_consing_key: WasmRecGroup,
    shared_type_indices: Box<[VMSharedTypeIndex]>,
    registrations: AtomicUsize,
}

impl PartialEq for RecGroupEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.hash_consing_key == other.0.hash_consing_key
    }
}

impl Eq for RecGroupEntry {}

impl Hash for RecGroupEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash_consing_key.hash(state);
    }
}

impl Borrow<WasmRecGroup> for RecGroupEntry {
    fn borrow(&self) -> &WasmRecGroup {
        &self.0.hash_consing_key
    }
}

impl RecGroupEntry {
    fn incr_ref_count(&self, why: &str) {
        let old_count = self.0.registrations.fetch_add(1, Ordering::AcqRel);
        let new_count = old_count + 1;
        tracing::trace!(
            "increment registration count for {self:?} (registrations -> {new_count}): {why}",
        );
    }

    #[must_use = "caller must remove entry from registry if `decref` returns `true`"]
    fn decr_ref_count(&self, why: &str) -> bool {
        let old_count = self.0.registrations.fetch_sub(1, Ordering::AcqRel);
        let new_count = old_count - 1;
        tracing::trace!(
            "decrement registration count for {self:?} (registrations -> {new_count}): {why}",
        );
        old_count == 1
    }
}
