// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use core::range::Range;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::{fmt, iter};

use cranelift_entity::packed_option::{PackedOption, ReservedValue};
use cranelift_entity::{PrimaryMap, SecondaryMap, iter_entity_range};
use hashbrown::HashSet;
use spin::RwLock;
use wasmtime_slab::Slab;

use crate::wasm::Engine;
use crate::wasm::indices::{
    CanonicalizedTypeIndex, ModuleInternedTypeIndex, RecGroupRelativeTypeIndex, VMSharedTypeIndex,
};
use crate::wasm::translate::{
    ModuleTypes, WasmCompositeType, WasmCompositeTypeInner, WasmRecGroup, WasmSubType,
};

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

    /// Trace all `VMSharedTypeIndex` edges, ignoring other edges.
    fn trace_engine_indices<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(VMSharedTypeIndex) -> Result<(), E>,
    {
        self.trace(&mut |idx| match idx {
            CanonicalizedTypeIndex::Engine(idx) => func(idx),
            CanonicalizedTypeIndex::Module(_) | CanonicalizedTypeIndex::RecGroup(_) => Ok(()),
        })
    }

    fn canonicalize_for_runtime_usage<F>(&mut self, module_to_engine: &mut F)
    where
        F: FnMut(ModuleInternedTypeIndex) -> VMSharedTypeIndex,
    {
        self.trace_mut::<_, ()>(&mut |idx| match *idx {
            CanonicalizedTypeIndex::Engine(_) => Ok(()),
            CanonicalizedTypeIndex::RecGroup(_) => unreachable!(),
            CanonicalizedTypeIndex::Module(module_index) => {
                let index = module_to_engine(module_index);
                *idx = CanonicalizedTypeIndex::Engine(index);
                Ok(())
            }
        })
        .unwrap();
    }

    fn is_canonicalized_for_runtime_usage(&self) -> bool {
        self.trace(&mut |idx| match idx {
            CanonicalizedTypeIndex::Engine(_) => Ok(()),
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
                CanonicalizedTypeIndex::Engine(_) => Ok(()),
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
                        CanonicalizedTypeIndex::Engine(module_to_engine(module_index))
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
            CanonicalizedTypeIndex::Engine(_) | CanonicalizedTypeIndex::RecGroup(_) => Ok(()),
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
    trampolines: SecondaryMap<VMSharedTypeIndex, PackedOption<ModuleInternedTypeIndex>>,
}

impl RuntimeTypeCollection {
    pub fn empty(engine: Engine) -> Self {
        Self {
            engine,
            rec_groups: vec![],
            types: PrimaryMap::default(),
            trampolines: SecondaryMap::default(),
        }
    }

    /// Gets the map from `ModuleInternedTypeIndex` to `VMSharedTypeIndex`
    pub fn type_map(&self) -> &PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex> {
        &self.types
    }

    /// Look up a shared type index by its module type index
    #[inline]
    pub fn lookup_shared_type(&self, index: ModuleInternedTypeIndex) -> Option<VMSharedTypeIndex> {
        self.types.get(index).copied()
    }

    #[inline]
    pub fn trampoline_type(&self, ty: VMSharedTypeIndex) -> Option<ModuleInternedTypeIndex> {
        let trampoline_ty = self.trampolines[ty].expand();
        tracing::trace!("TypeCollection::trampoline_type({ty:?}) -> {trampoline_ty:?}");
        trampoline_ty
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
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
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

    #[inline]
    pub fn debug_assert_contains(&self, index: VMSharedTypeIndex) {
        if cfg!(debug_assertions) {
            self.0.read().debug_assert_registered(index);
        }
    }

    /// Looks up a function type from a shared type index.
    ///
    /// This does *NOT* prevent the type from being unregistered while you are
    /// still using the resulting value! Use the `RegisteredType::root`
    /// constructor if you need to ensure that property and you don't have some
    /// other mechanism already keeping the type registered.
    pub fn borrow(&self, index: VMSharedTypeIndex) -> Option<Arc<WasmSubType>> {
        let id = shared_type_index_to_slab_id(index);
        let inner = self.0.read();
        inner.types.get(id).and_then(|ty| ty.clone())
    }

    pub fn register_module_types(
        &self,
        engine: &Engine,
        module_types: ModuleTypes,
    ) -> RuntimeTypeCollection {
        let (rec_groups, types) = self.0.write().register_module_types(&module_types);

        tracing::trace!("Begin building module's shared-to-module-trampoline-types map");
        let mut trampolines = SecondaryMap::with_capacity(types.len());
        for (module_ty, module_trampoline_ty) in module_types.trampoline_types() {
            let shared_ty = types[module_ty];
            let trampoline_shared_ty = self.get_trampoline_type(shared_ty);
            trampolines[trampoline_shared_ty] = Some(module_trampoline_ty).into();
            tracing::trace!(
                "--> shared_to_module_trampolines[{trampoline_shared_ty:?}] = {module_trampoline_ty:?}"
            );
        }
        tracing::trace!("Done building module's shared-to-module-trampoline-types map");

        RuntimeTypeCollection {
            engine: engine.clone(),
            rec_groups,
            types,
            trampolines,
        }
    }

    pub fn register_type(&self, engine: &Engine, ty: WasmSubType) -> RegisteredType {
        self.0.write().register_type(engine, ty)
    }

    // pub fn get_type(&self, engine: &Engine, index: VMSharedTypeIndex) -> Option<RegisteredType> {
    //     let id = shared_type_index_to_slab_id(index);
    //     let inner = self.0.read();
    //
    //     let ty = inner.types.get(id)?.clone().unwrap();
    //     let entry = inner.type_to_rec_group[index].clone().unwrap();
    //
    //     entry.incr_ref_count("TypeRegistry::get_type");
    //
    //     debug_assert!(entry.0.registrations.load(Acquire) != 0);
    //     Some(RegisteredType {
    //         engine: engine.clone(),
    //         entry,
    //         ty,
    //         index,
    //     })
    // }

    /// Get the trampoline type for the given function type index.
    ///
    /// Panics for non-function type indices.
    pub fn get_trampoline_type(&self, index: VMSharedTypeIndex) -> VMSharedTypeIndex {
        let id = shared_type_index_to_slab_id(index);
        let inner = self.0.read();

        let ty = inner.types[id].as_ref().unwrap();
        debug_assert!(
            ty.is_func(),
            "cannot get the trampoline type of a non-function type: {index:?} = {ty:?}"
        );

        let trampoline_ty = match inner.type_to_trampoline.get(index).and_then(|x| x.expand()) {
            Some(ty) => ty,
            None => {
                // The function type is its own trampoline type.
                index
            }
        };
        tracing::trace!("TypeRegistry::trampoline_type({index:?}) -> {trampoline_ty:?}");
        trampoline_ty
    }

    /// Create an owning handle to the given index's associated type.
    ///
    /// This will prevent the associated type from being unregistered as long as
    /// the returned `RegisteredType` is kept alive.
    ///
    /// Returns `None` if `index` is not registered in the given engine's
    /// registry.
    pub fn root(&self, engine: &Engine, index: VMSharedTypeIndex) -> Option<RegisteredType> {
        debug_assert!(!index.is_reserved_value());
        let (entry, ty) = {
            let id = shared_type_index_to_slab_id(index);
            let inner = self.0.read();

            let ty = inner.types.get(id)?.clone().unwrap();
            let entry = inner.type_to_rec_group[index].clone().unwrap();
            // let layout = inner.type_to_gc_layout.get(index).and_then(|l| l.clone());

            // NB: make sure to incref while the lock is held to prevent:
            //
            // * This thread: read locks registry, gets entry E, unlocks registry
            // * Other thread: drops `RegisteredType` for entry E, decref
            //   reaches zero, write locks registry, unregisters entry
            // * This thread: increfs entry, but it isn't in the registry anymore
            entry.incr_ref_count("TypeRegistry::root");

            (entry, ty)
        };

        debug_assert!(entry.0.registrations.load(Ordering::Acquire) != 0);
        Some(RegisteredType {
            engine: engine.clone(),
            entry,
            ty,
            index,
        })
    }

    /// Is type `sub` a subtype of `sup`?
    #[inline]
    pub fn is_subtype(&self, sub: VMSharedTypeIndex, sup: VMSharedTypeIndex) -> bool {
        if cfg!(debug_assertions) {
            self.0.read().debug_assert_registered(sub);
            self.0.read().debug_assert_registered(sup);
        }

        if sub == sup {
            return true;
        }

        self.is_subtype_slow(sub, sup)
    }

    fn is_subtype_slow(&self, sub: VMSharedTypeIndex, sup: VMSharedTypeIndex) -> bool {
        // Do the O(1) subtype checking trick:
        //
        // In a type system with single inheritance, the subtyping relationships
        // between all types form a set of trees. The root of each tree is a
        // type that has no supertype; each node's immediate children are the
        // types that directly subtype that node.
        //
        // For example, consider these types:
        //
        //     class Base {}
        //     class A subtypes Base {}
        //     class B subtypes Base {}
        //     class C subtypes A {}
        //     class D subtypes A {}
        //     class E subtypes C {}
        //
        // These types produce the following tree:
        //
        //                Base
        //               /    \
        //              A      B
        //             / \
        //            C   D
        //           /
        //          E
        //
        // Note the following properties:
        //
        // 1. If `sub` is a subtype of `sup` (either directly or transitively)
        //    then `sup` *must* be on the path from `sub` up to the root of
        //    `sub`'s tree.
        //
        // 2. Additionally, `sup` *must* be the `i`th node down from the root in
        //    that path, where `i` is the length of the path from `sup` to its
        //    tree's root.
        //
        // Therefore, if we have the path to the root for each type (we do) then
        // we can simply check if `sup` is at index `supertypes(sup).len()`
        // within `supertypes(sub)`.
        let inner = self.0.read();
        let sub_supertypes = inner.supertypes(sub);
        let sup_supertypes = inner.supertypes(sup);
        sub_supertypes.get(sup_supertypes.len()) == Some(&sup)
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
    // A hash map from a canonicalized-for-hash-consing rec group to its
    // `VMSharedTypeIndex`es.
    //
    // There is an entry in this map for every rec group we have already
    // registered. Before registering new rec groups, we first check this map to
    // see if we've already registered an identical rec group that we should
    // reuse instead.
    hash_consing_map: HashSet<RecGroupEntry>,
    // A map from `VMSharedTypeIndex::bits()` to the type index's associated
    // Wasm type.
    //
    // These types are always canonicalized for runtime usage.
    //
    // These are only `None` during the process of inserting a new rec group
    // into the registry, where we need registered `VMSharedTypeIndex`es for
    // forward type references within the rec group, but have not actually
    // inserted all the types within the rec group yet.
    types: Slab<Option<Arc<WasmSubType>>>,
    // A map that lets you walk backwards from a `VMSharedTypeIndex` to its
    // `RecGroupEntry`.
    type_to_rec_group: SecondaryMap<VMSharedTypeIndex, Option<RecGroupEntry>>,
    // A map from a registered type to its complete list of supertypes.
    //
    // The supertypes are ordered from super- to subtype, i.e. the immediate
    // parent supertype is the last element and the least-upper-bound of all
    // supertypes is the first element.
    //
    // Types without any supertypes are omitted from this map. This means that
    // we never allocate any backing storage for this map when Wasm GC is not in
    // use.
    type_to_supertypes: SecondaryMap<VMSharedTypeIndex, Option<Box<[VMSharedTypeIndex]>>>,
    // A map from each registered function type to its trampoline type.
    //
    // Note that when a function type is its own trampoline type, then we omit
    // the entry in this map as a memory optimization. This means that if only
    // core Wasm function types are ever used, then we will never allocate any
    // backing storage for this map. As a nice bonus, this also avoids cycles (a
    // function type referencing itself) that our naive reference counting
    // doesn't play well with.
    type_to_trampoline: SecondaryMap<VMSharedTypeIndex, PackedOption<VMSharedTypeIndex>>,
    // An explicit stack of entries that we are in the middle of dropping. Used
    // to avoid recursion when dropping a type that is holding the last
    // reference to another type, etc...
    drop_stack: Vec<RecGroupEntry>,
}

impl TypeRegistryInner {
    #[inline]
    #[track_caller]
    fn debug_assert_registered(&self, index: VMSharedTypeIndex) {
        debug_assert!(
            !index.is_reserved_value(),
            "should have an actual VMSharedTypeIndex, not the reserved value"
        );
        debug_assert!(
            self.types.contains(shared_type_index_to_slab_id(index)),
            "registry's slab should contain {index:?}",
        );
        debug_assert!(
            self.types[shared_type_index_to_slab_id(index)].is_some(),
            "registry's slab should actually contain a type for {index:?}",
        );
        debug_assert!(
            self.type_to_rec_group[index].is_some(),
            "{index:?} should have an associated rec group entry"
        );
    }

    #[inline]
    #[track_caller]
    fn debug_assert_all_registered(&self, indices: impl IntoIterator<Item = VMSharedTypeIndex>) {
        if cfg!(debug_assertions) {
            for index in indices {
                self.debug_assert_registered(index);
            }
        }
    }

    /// Is the given type canonicalized for runtime usage this registry?
    fn assert_canonicalized_for_runtime_usage_in_this_registry(&self, ty: &WasmSubType) {
        ty.trace::<_, ()>(&mut |index| match index {
            CanonicalizedTypeIndex::RecGroup(_) | CanonicalizedTypeIndex::Module(_) => {
                panic!("not canonicalized for runtime usage: {ty:?}")
            }
            CanonicalizedTypeIndex::Engine(idx) => {
                self.debug_assert_registered(idx);
                Ok(())
            }
        })
        .unwrap();
    }

    #[tracing::instrument(skip(self))]
    fn register_module_types(
        &mut self,
        types: &ModuleTypes,
    ) -> (
        Vec<RecGroupEntry>,
        PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex>,
    ) {
        // The engine's type registry entries for these module types.
        let mut entries = Vec::with_capacity(types.rec_groups().len());

        // The map from a module type index to an engine type index for these
        // module types.
        let mut map = PrimaryMap::<ModuleInternedTypeIndex, VMSharedTypeIndex>::with_capacity(
            types.wasm_types().len(),
        );

        for module_group in types.rec_groups().copied() {
            let entry = self.register_rec_group(
                &map,
                module_group,
                iter_entity_range(module_group.into())
                    .map(|ty| types.get_wasm_type(ty).unwrap().clone()),
            );

            // Update the module-to-engine map with this rec group's
            // newly-registered types.
            for (module_ty, engine_ty) in
                iter_entity_range(module_group.into()).zip(entry.0.shared_type_indices.iter())
            {
                let module_ty2 = map.push(*engine_ty);
                assert_eq!(module_ty, module_ty2);
            }

            entries.push(entry);
        }

        (entries, map)
    }

    fn register_type(&mut self, engine: &Engine, ty: WasmSubType) -> RegisteredType {
        let entry = self.register_singleton_rec_group(ty);

        let index = entry.0.shared_type_indices[0];
        let id = shared_type_index_to_slab_id(index);
        let ty = self.types[id].clone().unwrap();
        RegisteredType {
            engine: engine.clone(),
            entry,
            ty,
            index,
        }
    }

    fn register_rec_group(
        &mut self,
        map: &PrimaryMap<ModuleInternedTypeIndex, VMSharedTypeIndex>,
        range: Range<ModuleInternedTypeIndex>,
        types: impl ExactSizeIterator<Item = WasmSubType>,
    ) -> RecGroupEntry {
        debug_assert_eq!(iter_entity_range(range.into()).len(), types.len());

        // We need two different canonicalizations of this rec group: one for
        // hash-consing and another for runtime usage within this
        // engine. However, we only need the latter if this is a new rec group
        // that hasn't been registered before. Therefore, we only eagerly create
        // the hash-consing canonicalized version, and while we lazily
        // canonicalize for runtime usage in this engine, we must still eagerly
        // clone and set aside the original, non-canonicalized types for that
        // potential engine canonicalization eventuality.
        let mut non_canon_types = Vec::with_capacity(types.len());
        let hash_consing_key = WasmRecGroup(
            types
                .zip(iter_entity_range(range.into()))
                .map(|(mut ty, module_index)| {
                    non_canon_types.push((module_index, ty.clone()));
                    ty.canonicalize_for_hash_consing(range, &mut |idx| {
                        debug_assert!(idx < range.start);
                        map[idx]
                    });
                    ty
                })
                .collect::<Box<[_]>>(),
        );

        // Any references in the hash-consing key to types outside of this rec
        // group may only be to fully-registered types.
        if cfg!(debug_assertions) {
            hash_consing_key
                .trace_engine_indices::<_, ()>(&mut |index| {
                    self.debug_assert_registered(index);
                    Ok(())
                })
                .unwrap();
        }

        // If we've already registered this rec group before, reuse it.
        if let Some(entry) = self.hash_consing_map.get(&hash_consing_key) {
            tracing::trace!("hash-consing map hit: reusing {entry:?}");
            assert!(!entry.0.unregistered.load(Ordering::Acquire));
            self.debug_assert_all_registered(entry.0.shared_type_indices.iter().copied());
            entry.incr_ref_count("hash-consing map hit");
            return entry.clone();
        }

        tracing::trace!("hash-consing map miss: making new registration");

        // Inter-group edges: increment the referenced group's ref
        // count, because these other rec groups shouldn't be dropped
        // while this rec group is still alive.
        hash_consing_key
            .trace_engine_indices::<_, ()>(&mut |index| {
                self.debug_assert_registered(index);
                let other_entry = self.type_to_rec_group[index].as_ref().unwrap();
                assert!(!other_entry.0.unregistered.load(Ordering::Acquire));
                other_entry.incr_ref_count("new rec group's type references");
                Ok(())
            })
            .unwrap();

        // Register the individual types.
        //
        // Note that we can't update the reverse type-to-rec-group map until
        // after we've constructed the `RecGroupEntry`, since that map needs the
        // fully-constructed entry for its values.
        let module_rec_group_start = range.start;
        let shared_type_indices: Box<[_]> = non_canon_types
            .iter()
            .map(|(module_index, ty)| {
                let engine_index = slab_id_to_shared_type_index(self.types.alloc(None));
                tracing::trace!(
                    "reserved {engine_index:?} for {module_index:?} = non-canonical {ty:?}"
                );
                engine_index
            })
            .collect();
        for (engine_index, (module_index, mut ty)) in
            shared_type_indices.iter().copied().zip(non_canon_types)
        {
            tracing::trace!("canonicalizing {engine_index:?} for runtime usage");
            ty.canonicalize_for_runtime_usage(&mut |module_index| {
                if module_index < module_rec_group_start {
                    let engine_index = map[module_index];
                    tracing::trace!("    cross-group {module_index:?} becomes {engine_index:?}");
                    self.debug_assert_registered(engine_index);
                    engine_index
                } else {
                    assert!(module_index < range.end);
                    let rec_group_offset = module_index.as_u32() - module_rec_group_start.as_u32();
                    let rec_group_offset = usize::try_from(rec_group_offset).unwrap();
                    let engine_index = shared_type_indices[rec_group_offset];
                    tracing::trace!("    intra-group {module_index:?} becomes {engine_index:?}");
                    assert!(!engine_index.is_reserved_value());
                    assert!(
                        self.types
                            .contains(shared_type_index_to_slab_id(engine_index))
                    );
                    engine_index
                }
            });
            self.insert_one_type_from_rec_group(module_index, engine_index, ty);
        }

        // Although we haven't finished registering all their metadata, the
        // types themselves should all be filled in now.
        if cfg!(debug_assertions) {
            for index in &shared_type_indices {
                let id = shared_type_index_to_slab_id(*index);
                debug_assert!(self.types.contains(id));
                debug_assert!(self.types[id].is_some());
            }
        }
        debug_assert_eq!(
            shared_type_indices.len(),
            shared_type_indices
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            "should not have any duplicate type indices",
        );

        let entry = RecGroupEntry(Arc::new(RecGroupEntryInner {
            hash_consing_key,
            shared_type_indices,
            registrations: AtomicUsize::new(1),
            unregistered: AtomicBool::new(false),
        }));
        tracing::trace!("new {entry:?} -> count 1");

        let is_new_entry = self.hash_consing_map.insert(entry.clone());
        debug_assert!(is_new_entry);

        // Now that we've constructed the entry, we can update the reverse
        // type-to-rec-group map.
        for ty in entry.0.shared_type_indices.iter().copied() {
            debug_assert!(self.type_to_rec_group[ty].is_none());
            self.type_to_rec_group[ty] = Some(entry.clone());
        }
        self.debug_assert_all_registered(entry.0.shared_type_indices.iter().copied());

        // Finally, make sure to register the trampoline type for each function
        // type in the rec group.
        for shared_type_index in entry.0.shared_type_indices.iter().copied() {
            let slab_id = shared_type_index_to_slab_id(shared_type_index);
            let sub_ty = self.types[slab_id].as_ref().unwrap();
            if let Some(f) = sub_ty.as_func() {
                let trampoline = f.trampoline_type();
                match &trampoline {
                    Cow::Borrowed(_) if sub_ty.is_final && sub_ty.supertype.is_none() => {
                        // The function type is its own trampoline type. Leave
                        // its entry in `type_to_trampoline` empty to signal
                        // this.
                        tracing::trace!(
                            "trampoline_type({shared_type_index:?}) = {shared_type_index:?}",
                        );
                    }
                    Cow::Borrowed(_) | Cow::Owned(_) => {
                        // This will recursively call into rec group
                        // registration, but at most once since trampoline
                        // function types are their own trampoline type.
                        let trampoline_entry = self.register_singleton_rec_group(WasmSubType {
                            is_final: true,
                            supertype: None,
                            composite_type: WasmCompositeType {
                                shared: sub_ty.composite_type.shared,
                                inner: WasmCompositeTypeInner::Func(trampoline.into_owned()),
                            },
                        });
                        assert_eq!(trampoline_entry.0.shared_type_indices.len(), 1);
                        let trampoline_index = trampoline_entry.0.shared_type_indices[0];
                        tracing::trace!(
                            "trampoline_type({shared_type_index:?}) = {trampoline_index:?}",
                        );
                        self.debug_assert_registered(trampoline_index);
                        debug_assert_ne!(shared_type_index, trampoline_index);
                        self.type_to_trampoline[shared_type_index] = Some(trampoline_index).into();
                    }
                }
            }
        }

        entry
    }

    fn insert_one_type_from_rec_group(
        &mut self,
        module_index: ModuleInternedTypeIndex,
        engine_index: VMSharedTypeIndex,
        ty: WasmSubType,
    ) {
        // Despite being canonicalized for runtime usage, this type may still
        // have forward references to other types in the rec group we haven't
        // yet registered. Therefore, we can't use our usual
        // `assert_canonicalized_for_runtime_usage_in_this_registry` helper here
        // as that will see the forward references and think they must be
        // references to types in other registries.
        debug_assert!(
            ty.is_canonicalized_for_runtime_usage(),
            "type is not canonicalized for runtime usage: {ty:?}"
        );

        // Add the type to our slab.
        let id = shared_type_index_to_slab_id(engine_index);
        assert!(self.types.contains(id));
        assert!(self.types[id].is_none());
        self.types[id] = Some(Arc::new(ty));

        // Create the supertypes list for this type.
        if let Some(supertype) = self.types[id].as_ref().unwrap().supertype {
            let supertype = supertype.unwrap_engine_type_index();
            let supers_supertypes = self.supertypes(supertype);
            let mut supertypes = Vec::with_capacity(supers_supertypes.len() + 1);
            supertypes.extend(
                supers_supertypes
                    .iter()
                    .copied()
                    .chain(iter::once(supertype)),
            );
            self.type_to_supertypes[engine_index] = Some(supertypes.into_boxed_slice());
        }

        tracing::trace!(
            "finished registering type {module_index:?} as {engine_index:?} = runtime-canonical {:?}",
            self.types[id].as_ref().unwrap()
        );
    }

    /// Get the supertypes list for the given type.
    ///
    /// The supertypes are listed in super-to-sub order. `ty` itself is not
    /// included in the list.
    fn supertypes(&self, ty: VMSharedTypeIndex) -> &[VMSharedTypeIndex] {
        self.type_to_supertypes
            .get(ty)
            .and_then(|s| s.as_deref())
            .unwrap_or(&[])
    }

    /// Register a rec group consisting of a single type.
    ///
    /// The type must already be canonicalized for runtime usage in this
    /// registry.
    ///
    /// The returned entry will have already had its reference count incremented
    /// on behalf of callers.
    fn register_singleton_rec_group(&mut self, ty: WasmSubType) -> RecGroupEntry {
        self.assert_canonicalized_for_runtime_usage_in_this_registry(&ty);

        // This type doesn't have any module-level type references, since it is
        // already canonicalized for runtime usage in this registry, so an empty
        // map suffices.
        let map = PrimaryMap::default();

        // This must have `range.len() == 1`, even though we know this type
        // doesn't have any intra-group type references, to satisfy
        // `register_rec_group`'s preconditions.
        let range = Range::from(
            ModuleInternedTypeIndex::from_bits(u32::MAX - 1)
                ..ModuleInternedTypeIndex::from_bits(u32::MAX),
        );

        self.register_rec_group(&map, range, iter::once(ty))
    }

    #[tracing::instrument(skip(self))]
    fn unregister_type_collection(&mut self, collection: &RuntimeTypeCollection) {
        for entry in &collection.rec_groups {
            self.debug_assert_all_registered(entry.0.shared_type_indices.iter().copied());
            if entry.decr_ref_count("TypeRegistryInner::unregister_type_collection") {
                self.unregister_entry(entry.clone());
            }
        }
    }

    fn unregister_entry(&mut self, entry: RecGroupEntry) {
        tracing::trace!("Attempting to unregister {entry:?}");
        debug_assert!(self.drop_stack.is_empty());

        // There are two races to guard against before we can unregister the
        // entry, even though it was on the drop stack:
        //
        // 1. Although an entry has to reach zero registrations before it is
        //    enqueued in the drop stack, we need to double check whether the
        //    entry is *still* at zero registrations. This is because someone
        //    else can resurrect the entry in between when the
        //    zero-registrations count was first observed and when we actually
        //    acquire the lock to unregister it. In this example, we have
        //    threads A and B, an existing rec group entry E, and a rec group
        //    entry E' that is a duplicate of E:
        //
        //    Thread A                        | Thread B
        //    --------------------------------+-----------------------------
        //    acquire(type registry lock)     |
        //                                    |
        //                                    | decref(E) --> 0
        //                                    |
        //                                    | block_on(type registry lock)
        //                                    |
        //    register(E') == incref(E) --> 1 |
        //                                    |
        //    release(type registry lock)     |
        //                                    |
        //                                    | acquire(type registry lock)
        //                                    |
        //                                    | unregister(E)         !!!!!!
        //
        //    If we aren't careful, we can unregister a type while it is still
        //    in use!
        //
        //    The fix in this case is that we skip unregistering the entry if
        //    its reference count is non-zero, since that means it was
        //    concurrently resurrected and is now in use again.
        //
        // 2. In a slightly more convoluted version of (1), where an entry is
        //    resurrected but then dropped *again*, someone might attempt to
        //    unregister an entry a second time:
        //
        //    Thread A                        | Thread B
        //    --------------------------------|-----------------------------
        //    acquire(type registry lock)     |
        //                                    |
        //                                    | decref(E) --> 0
        //                                    |
        //                                    | block_on(type registry lock)
        //                                    |
        //    register(E') == incref(E) --> 1 |
        //                                    |
        //    release(type registry lock)     |
        //                                    |
        //    decref(E) --> 0                 |
        //                                    |
        //    acquire(type registry lock)     |
        //                                    |
        //    unregister(E)                   |
        //                                    |
        //    release(type registry lock)     |
        //                                    |
        //                                    | acquire(type registry lock)
        //                                    |
        //                                    | unregister(E)         !!!!!!
        //
        //    If we aren't careful, we can unregister a type twice, which leads
        //    to panics and registry corruption!
        //
        //    To detect this scenario and avoid the double-unregistration bug,
        //    we maintain an `unregistered` flag on entries. We set this flag
        //    once an entry is unregistered and therefore, even if it is
        //    enqueued in the drop stack multiple times, we only actually
        //    unregister the entry the first time.
        //
        // A final note: we don't need to worry about any concurrent
        // modifications during the middle of this function's execution, only
        // between (a) when we first observed a zero-registrations count and
        // decided to unregister the type, and (b) when we acquired the type
        // registry's lock so that we could perform that unregistration. This is
        // because this method has exclusive access to `&mut self` -- that is,
        // we have a write lock on the whole type registry -- and therefore no
        // one else can create new references to this zero-registration entry
        // and bring it back to life (which would require finding it in
        // `self.hash_consing_map`, which no one else has access to, because we
        // now have an exclusive lock on `self`).

        // Handle scenario (1) from above.
        let registrations = entry.0.registrations.load(Ordering::Acquire);
        if registrations != 0 {
            tracing::trace!(
                "    {entry:?} was concurrently resurrected and no longer has \
                 zero registrations (registrations -> {registrations})",
            );
            assert!(!entry.0.unregistered.load(Ordering::Acquire));
            return;
        }

        // Handle scenario (2) from above.
        if entry.0.unregistered.load(Ordering::Acquire) {
            tracing::trace!(
                "    {entry:?} was concurrently resurrected, dropped again, \
                 and already unregistered"
            );
            return;
        }

        // Okay, we are really going to unregister this entry. Enqueue it on the
        // drop stack.
        self.drop_stack.push(entry);

        // Keep unregistering entries until the drop stack is empty. This is
        // logically a recursive process where if we unregister a type that was
        // the only thing keeping another type alive, we then recursively
        // unregister that other type as well. However, we use this explicit
        // drop stack to avoid recursion and the potential stack overflows that
        // recursion implies.
        while let Some(entry) = self.drop_stack.pop() {
            tracing::trace!("Begin unregistering {entry:?}");
            self.debug_assert_all_registered(entry.0.shared_type_indices.iter().copied());

            // All entries on the drop stack should *really* be ready for
            // unregistration, since no one can resurrect entries once we've
            // locked the registry.
            assert_eq!(entry.0.registrations.load(Ordering::Acquire), 0);
            assert!(!entry.0.unregistered.load(Ordering::Acquire));

            // We are taking responsibility for unregistering this entry, so
            // prevent anyone else from attempting to do it again.
            entry.0.unregistered.store(true, Ordering::Release);

            // Decrement any other types that this type was shallowly
            // (i.e. non-transitively) referencing and keeping alive. If this
            // was the last thing keeping them registered, its okay to
            // unregister them as well now.
            debug_assert!(entry.0.hash_consing_key.is_canonicalized_for_hash_consing());
            entry
                .0
                .hash_consing_key
                .trace_engine_indices::<_, ()>(&mut |other_index| {
                    self.debug_assert_registered(other_index);
                    let other_entry = self.type_to_rec_group[other_index].as_ref().unwrap();
                    if other_entry.decr_ref_count("dropping rec group's type references") {
                        self.drop_stack.push(other_entry.clone());
                    }
                    Ok(())
                })
                .unwrap();

            // Remove the entry from the hash-consing map. If we register a
            // duplicate definition of this rec group again in the future, it
            // will be as if it is the first time it has ever been registered,
            // and it will be inserted into the hash-consing map again at that
            // time.
            let was_in_map = self.hash_consing_map.remove(&entry);
            debug_assert!(was_in_map);

            // Similarly, remove the rec group's types from the registry, as
            // well as their entries from the reverse type-to-rec-group
            // map. Additionally, stop holding a strong reference from each
            // function type in the rec group to that function type's trampoline
            // type.
            debug_assert_eq!(
                entry.0.shared_type_indices.len(),
                entry
                    .0
                    .shared_type_indices
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>()
                    .len(),
                "should not have any duplicate type indices",
            );
            for ty in entry.0.shared_type_indices.iter().copied() {
                tracing::trace!("removing {ty:?} from registry");

                let removed_entry = self.type_to_rec_group[ty].take();
                debug_assert_eq!(removed_entry.unwrap(), entry);

                // Remove the associated trampoline type, if any.
                if let Some(trampoline_ty) =
                    self.type_to_trampoline.get(ty).and_then(|x| x.expand())
                {
                    self.debug_assert_registered(trampoline_ty);
                    self.type_to_trampoline[ty] = None.into();
                    let trampoline_entry = self.type_to_rec_group[trampoline_ty].as_ref().unwrap();
                    if trampoline_entry
                        .decr_ref_count("dropping rec group's trampoline-type references")
                    {
                        self.drop_stack.push(trampoline_entry.clone());
                    }
                }

                // Remove the type's supertypes list, if any. Take care to guard
                // this assignment so that we don't accidentally force the
                // secondary map to allocate even when we never actually use
                // Wasm GC.
                if self.type_to_supertypes.get(ty).is_some() {
                    self.type_to_supertypes[ty] = None;
                }

                let id = shared_type_index_to_slab_id(ty);
                let deallocated_ty = self.types.dealloc(id);
                assert!(deallocated_ty.is_some());
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
            type_to_supertypes,
            type_to_trampoline,
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
            type_to_rec_group.is_empty() || type_to_rec_group.values().all(|x| x.is_none()),
            "type registry not empty: type-to-rec-group map is not empty: {type_to_rec_group:#?}"
        );
        assert!(
            type_to_supertypes.is_empty() || type_to_supertypes.values().all(|x| x.is_none()),
            "type registry not empty: type-to-supertypes map is not empty: {type_to_supertypes:#?}"
        );
        assert!(
            type_to_trampoline.is_empty() || type_to_trampoline.values().all(|x| x.is_none()),
            "type registry not empty: type-to-trampoline map is not empty: {type_to_trampoline:#?}"
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
    unregistered: AtomicBool,
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
