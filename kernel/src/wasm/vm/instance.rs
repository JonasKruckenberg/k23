// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::string::String;
use core::alloc::Layout;
use core::marker::PhantomPinned;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use core::{fmt, ptr, slice};

use anyhow::{bail, ensure};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntityRef, EntitySet, PrimaryMap};
use kmem::VirtualAddress;
use static_assertions::const_assert_eq;

use crate::wasm::TrapKind;
use crate::wasm::indices::{
    DataIndex, DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, DefinedTagIndex,
    ElemIndex, EntityIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TagIndex,
    VMSharedTypeIndex,
};
use crate::wasm::module::Module;
use crate::wasm::store::{StoreInner, StoreOpaque};
use crate::wasm::translate::{
    IndexType, MemoryInitializer, TableInitialValue, TableSegmentElements, TranslatedModule,
    WasmHeapTopType, WasmHeapTypeInner,
};
use crate::wasm::trap_handler::WasmFault;
use crate::wasm::vm::const_eval::{ConstEvalContext, ConstExprEvaluator};
use crate::wasm::vm::memory::Memory;
use crate::wasm::vm::provenance::{VmPtr, VmSafe};
use crate::wasm::vm::table::{Table, TableElement, TableElementType};
use crate::wasm::vm::{
    Export, ExportedFunction, ExportedGlobal, ExportedMemory, ExportedTable, ExportedTag, Imports,
    StaticVMShape, VMBuiltinFunctionsArray, VMCONTEXT_MAGIC, VMContext, VMFuncRef, VMFunctionBody,
    VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition, VMMemoryImport,
    VMOpaqueContext, VMShape, VMStoreContext, VMTableDefinition, VMTableImport, VMTagDefinition,
    VMTagImport,
};

#[derive(Debug)]
pub struct InstanceHandle {
    instance: Option<NonNull<Instance>>,
}
// Safety: TODO
unsafe impl Send for InstanceHandle {}
// Safety: TODO
unsafe impl Sync for InstanceHandle {}

#[repr(C)] // ensure that the vmctx field is last.
#[derive(Debug)]
pub struct Instance {
    module: Module,
    pub(in crate::wasm) memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    pub(in crate::wasm) tables: PrimaryMap<DefinedTableIndex, Table>,
    dropped_elements: EntitySet<ElemIndex>,
    dropped_data: EntitySet<DataIndex>,

    /// A pointer to the `vmctx` field at the end of the `Instance`.
    ///
    /// This pointer is created upon allocation with provenance that covers the *entire* instance
    /// and VMContext memory. Pointers to VMContext are derived from it inheriting this broader
    /// provenance. This is important for correctness.
    vmctx_self_reference: NonNull<VMContext>,
    /// Self-pointer back to `Store<T>` and its functions. Not present for
    /// the brief time that `Store<T>` is itself being created. Also not
    /// present for some niche uses that are disconnected from stores (e.g.
    /// cross-thread stuff used in `InstancePre`)
    store: Option<NonNull<StoreOpaque>>,
    /// Additional context used by compiled wasm code. This field is last, and
    /// represents a dynamically-sized array that extends beyond the nominal
    /// end of the struct (similar to a flexible array member).
    vmctx: VMContext,
}

impl InstanceHandle {
    /// Creates an "empty" instance handle which internally has a null pointer
    /// to an instance. Actually calling any methods on this `InstanceHandle` will always
    /// panic.
    pub fn null() -> InstanceHandle {
        InstanceHandle { instance: None }
    }

    pub fn initialize(
        &mut self,
        store: &mut StoreOpaque,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
        imports: Imports,
        is_bulk_memory: bool,
    ) -> crate::Result<()> {
        // Safety: we call the functions in the right order (initialize_vmctx) first
        unsafe {
            self.instance_mut().initialize_vmctx(store, imports, module);

            if !is_bulk_memory {
                // Safety: see? we called `initialize_vmctx` before calling `check_init_bounds`!
                check_init_bounds(store, self.instance_mut(), module)?;
            }

            let mut ctx = ConstEvalContext::new(self.instance.unwrap().as_mut());
            self.instance_mut()
                .initialize_tables(store, &mut ctx, const_eval, module)?;
            self.instance_mut()
                .initialize_memories(store, &mut ctx, const_eval, module)?;
            self.instance_mut()
                .initialize_globals(store, &mut ctx, const_eval, module)?;
        }

        Ok(())
    }

    pub fn debug_vmctx(&self) {
        struct Dbg<'a> {
            data: &'a Instance,
        }
        impl fmt::Debug for Dbg<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Safety: Reading from JIT-owned memory is inherently unsafe.
                unsafe {
                    f.debug_struct("VMContext")
                        .field(
                            "magic",
                            &*self
                                .data
                                .vmctx_plus_offset::<u32>(StaticVMShape.vmctx_magic()),
                        )
                        .field(
                            "vm_store_context",
                            &*self
                                .data
                                .vmctx_plus_offset::<Option<VmPtr<VMStoreContext>>>(
                                    StaticVMShape.vmctx_store_context(),
                                ),
                        )
                        .field(
                            "builtin_functions",
                            &*self
                                .data
                                .vmctx_plus_offset::<VmPtr<VMBuiltinFunctionsArray>>(
                                    StaticVMShape.vmctx_builtin_functions(),
                                ),
                        )
                        .field(
                            "callee",
                            &*self
                                .data
                                .vmctx_plus_offset::<Option<VmPtr<VMFunctionBody>>>(
                                    StaticVMShape.vmctx_callee(),
                                ),
                        )
                        .field(
                            "epoch_ptr",
                            &*self.data.vmctx_plus_offset::<Option<VmPtr<AtomicU64>>>(
                                StaticVMShape.vmctx_epoch_ptr(),
                            ),
                        )
                        .field(
                            "gc_heap_base",
                            &*self.data.vmctx_plus_offset::<Option<VmPtr<u8>>>(
                                StaticVMShape.vmctx_gc_heap_base(),
                            ),
                        )
                        .field(
                            "gc_heap_bound",
                            &*self
                                .data
                                .vmctx_plus_offset::<usize>(StaticVMShape.vmctx_gc_heap_bound()),
                        )
                        .field(
                            "gc_heap_data",
                            &*self.data.vmctx_plus_offset::<Option<VmPtr<u8>>>(
                                StaticVMShape.vmctx_gc_heap_data(),
                            ),
                        )
                        .field(
                            "type_ids",
                            &*self.data.vmctx_plus_offset::<VmPtr<VMSharedTypeIndex>>(
                                StaticVMShape.vmctx_type_ids_array(),
                            ),
                        )
                        .field(
                            "imported_memories",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMMemoryImport>(
                                    self.data.vmshape().vmctx_imported_memories_begin(),
                                ),
                                self.data.vmshape().num_imported_memories as usize,
                            ),
                        )
                        .field(
                            "memories",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VmPtr<VMMemoryDefinition>>(
                                    self.data.vmshape().vmctx_memories_begin(),
                                ),
                                self.data.vmshape().num_defined_memories as usize,
                            ),
                        )
                        .field(
                            "owned_memories",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMMemoryDefinition>(
                                    self.data.vmshape().vmctx_owned_memories_begin(),
                                ),
                                self.data.vmshape().num_owned_memories as usize,
                            ),
                        )
                        .field(
                            "imported_functions",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMFunctionImport>(
                                    self.data.vmshape().vmctx_imported_functions_begin(),
                                ),
                                self.data.vmshape().num_imported_functions as usize,
                            ),
                        )
                        .field(
                            "imported_tables",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMTableImport>(
                                    self.data.vmshape().vmctx_imported_tables_begin(),
                                ),
                                self.data.vmshape().num_imported_tables as usize,
                            ),
                        )
                        .field(
                            "imported_globals",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMGlobalImport>(
                                    self.data.vmshape().vmctx_imported_globals_begin(),
                                ),
                                self.data.vmshape().num_imported_globals as usize,
                            ),
                        )
                        .field(
                            "imported_tags",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMTagImport>(
                                    self.data.vmshape().vmctx_imported_tags_begin(),
                                ),
                                self.data.vmshape().num_imported_tags as usize,
                            ),
                        )
                        .field(
                            "tables",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMTableDefinition>(
                                    self.data.vmshape().vmctx_tables_begin(),
                                ),
                                self.data.vmshape().num_defined_tables as usize,
                            ),
                        )
                        .field(
                            "globals",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMGlobalDefinition>(
                                    self.data.vmshape().vmctx_globals_begin(),
                                ),
                                self.data.vmshape().num_defined_globals as usize,
                            ),
                        )
                        .field(
                            "tags",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMTagDefinition>(
                                    self.data.vmshape().vmctx_tags_begin(),
                                ),
                                self.data.vmshape().num_defined_tags as usize,
                            ),
                        )
                        .field(
                            "func_refs",
                            &slice::from_raw_parts(
                                self.data.vmctx_plus_offset::<VMFuncRef>(
                                    self.data.vmshape().vmctx_func_refs_begin(),
                                ),
                                self.data.vmshape().num_escaped_funcs as usize,
                            ),
                        )
                        .finish()
                }
            }
        }

        tracing::debug!(
            "{:#?}",
            Dbg {
                data: self.instance()
            }
        );
    }

    pub fn vmctx(&self) -> NonNull<VMContext> {
        self.instance().vmctx()
    }

    /// Return a reference to a module.
    pub fn module(&self) -> &Module {
        self.instance().module()
    }

    /// Lookup a table by index.
    pub fn get_exported_table(&mut self, export: TableIndex) -> ExportedTable {
        self.instance_mut().get_exported_table(export)
    }

    /// Lookup a memory by index.
    pub fn get_exported_memory(&mut self, export: MemoryIndex) -> ExportedMemory {
        self.instance_mut().get_exported_memory(export)
    }

    /// Lookup a function by index.
    pub fn get_exported_func(&mut self, export: FuncIndex) -> ExportedFunction {
        self.instance_mut().get_exported_func(export)
    }

    /// Lookup a global by index.
    pub fn get_exported_global(&mut self, export: GlobalIndex) -> ExportedGlobal {
        self.instance_mut().get_exported_global(export)
    }

    /// Lookup a tag by index.
    pub fn get_exported_tag(&mut self, export: TagIndex) -> ExportedTag {
        self.instance_mut().get_exported_tag(export)
    }

    /// Lookup an item with the given index.
    pub fn get_export_by_index(&mut self, export: EntityIndex) -> Export {
        match export {
            EntityIndex::Function(i) => Export::Function(self.get_exported_func(i)),
            EntityIndex::Global(i) => Export::Global(self.get_exported_global(i)),
            EntityIndex::Table(i) => Export::Table(self.get_exported_table(i)),
            EntityIndex::Memory(i) => Export::Memory(self.get_exported_memory(i)),
            EntityIndex::Tag(i) => Export::Tag(self.get_exported_tag(i)),
        }
    }

    /// Return an iterator over the exports of this instance.
    ///
    /// Specifically, it provides access to the key-value pairs, where the keys
    /// are export names, and the values are export declarations which can be
    /// resolved `lookup_by_declaration`.
    pub fn exports(&self) -> wasmparser::collections::index_map::Iter<'_, String, EntityIndex> {
        self.instance().translated_module().exports.iter()
    }

    pub fn as_non_null(&self) -> NonNull<Instance> {
        self.instance.unwrap()
    }

    /// Return a reference to the contained `Instance`.
    #[inline]
    pub fn instance(&self) -> &Instance {
        // Safety: the constructor ensures the instance is correctly initialized
        unsafe { self.instance.unwrap().as_ref() }
    }

    #[inline]
    pub fn instance_mut(&mut self) -> &mut Instance {
        // Safety: the constructor ensures the instance is correctly initialized
        unsafe { self.instance.unwrap().as_mut() }
    }

    /// Attempts to convert from the host `addr` specified to a WebAssembly
    /// based address recorded in `WasmFault`.
    ///
    /// This method will check all linear memories that this instance contains
    /// to see if any of them contain `addr`. If one does then `Some` is
    /// returned with metadata about the wasm fault. Otherwise `None` is
    /// returned and `addr` doesn't belong to this instance.
    pub fn wasm_fault(&self, faulting_addr: VirtualAddress) -> Option<WasmFault> {
        self.instance().wasm_fault(faulting_addr)
    }
}

impl Instance {
    /// # Safety
    ///
    /// The caller must ensure that `instance: NonNull<Instance>` got allocated using the
    /// `Instance::alloc_layout` to ensure it is the right size for the VMContext
    pub unsafe fn from_parts(
        module: Module,
        instance: NonNull<Instance>,
        tables: PrimaryMap<DefinedTableIndex, Table>,
        memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    ) -> InstanceHandle {
        let dropped_elements = module.translated().active_table_initializers.clone();
        let dropped_data = module.translated().active_memory_initializers.clone();

        // Safety: we have to trust the caller that `NonNull<Instance>` got allocated using the correct
        // `Instance::alloc_layout` and therefore has the right-sized vmctx memory
        unsafe {
            instance.write(Instance {
                module: module.clone(),
                memories,
                tables,
                dropped_elements,
                dropped_data,
                vmctx_self_reference: instance.add(1).cast(),
                store: None,
                vmctx: VMContext {
                    _marker: PhantomPinned,
                },
            });
        }

        InstanceHandle {
            instance: Some(instance),
        }
    }

    pub fn alloc_layout(offsets: &VMShape) -> Layout {
        let size = size_of::<Self>()
            .checked_add(usize::try_from(offsets.size_of_vmctx()).unwrap())
            .unwrap();
        let align = align_of::<Self>();
        Layout::from_size_align(size, align).unwrap()
    }

    pub fn module(&self) -> &Module {
        &self.module
    }
    pub fn translated_module(&self) -> &TranslatedModule {
        self.module.translated()
    }
    pub fn vmshape(&self) -> &VMShape {
        self.module.vmshape()
    }

    fn wasm_fault(&self, addr: VirtualAddress) -> Option<WasmFault> {
        let mut fault = None;

        for (_, memory) in &self.memories {
            let accessible = memory.wasm_accessible();
            if accessible.start <= addr && addr < accessible.end {
                // All linear memories should be disjoint so assert that no
                // prior fault has been found.
                assert!(fault.is_none());
                fault = Some(WasmFault {
                    memory_size: memory.byte_size(),
                    wasm_address: u64::try_from(addr.checked_sub_addr(accessible.start).unwrap())
                        .unwrap(),
                });
            }
        }

        fault
    }

    pub fn get_exported_func(&mut self, index: FuncIndex) -> ExportedFunction {
        ExportedFunction {
            func_ref: self.get_func_ref(index).unwrap(),
        }
    }
    pub fn get_exported_table(&mut self, index: TableIndex) -> ExportedTable {
        let (definition, vmctx) =
            if let Some(def_index) = self.translated_module().defined_table_index(index) {
                (self.table_ptr(def_index), self.vmctx())
            } else {
                let import = self.imported_table(index);
                (import.from.as_non_null(), import.vmctx.as_non_null())
            };

        ExportedTable {
            definition,
            vmctx,
            table: self.translated_module().tables[index].clone(),
        }
    }
    pub fn get_exported_memory(&mut self, index: MemoryIndex) -> ExportedMemory {
        let (definition, vmctx, def_index) =
            if let Some(def_index) = self.translated_module().defined_memory_index(index) {
                (self.memory_ptr(def_index), self.vmctx(), def_index)
            } else {
                let import = self.imported_memory(index);
                (
                    import.from.as_non_null(),
                    import.vmctx.as_non_null(),
                    import.index,
                )
            };

        ExportedMemory {
            definition,
            vmctx,
            index: def_index,
            memory: self.translated_module().memories[index].clone(),
        }
    }
    pub fn get_exported_global(&mut self, index: GlobalIndex) -> ExportedGlobal {
        ExportedGlobal {
            definition: if let Some(def_index) =
                self.translated_module().defined_global_index(index)
            {
                self.global_ptr(def_index)
            } else {
                self.imported_global(index).from.as_non_null()
            },
            vmctx: Some(self.vmctx()),
            global: self.translated_module().globals[index].clone(),
        }
    }
    pub fn get_exported_tag(&mut self, index: TagIndex) -> ExportedTag {
        ExportedTag {
            definition: if let Some(def_index) = self.translated_module().defined_tag_index(index) {
                self.tag_ptr(def_index)
            } else {
                self.imported_tag(index).from.as_non_null()
            },
            tag: self.translated_module().tags[index],
        }
    }

    /// Get the given memory's page size, in bytes.
    pub fn memory_page_size(&self, index: MemoryIndex) -> u64 {
        self.translated_module().memories[index].page_size()
    }

    #[expect(unused, reason = "TODO")]
    pub fn memory_grow(
        &mut self,
        store: &mut StoreOpaque,
        index: MemoryIndex,
        delta: u64,
    ) -> crate::Result<Option<u64>> {
        todo!()
    }

    #[expect(unused, reason = "TODO")]
    pub fn memory_copy(
        &mut self,
        dst_index: MemoryIndex,
        dst: u64,
        src_index: MemoryIndex,
        src: u64,
        len: u64,
    ) -> Result<(), TrapKind> {
        todo!()
    }

    #[expect(unused, reason = "TODO")]
    pub fn memory_fill(
        &mut self,
        memory_index: MemoryIndex,
        dst: u64,
        val: u8,
        len: u64,
    ) -> Result<(), TrapKind> {
        todo!()
    }

    #[expect(unused, reason = "TODO")]
    pub fn memory_init(
        &mut self,
        memory_index: MemoryIndex,
        data_index: DataIndex,
        dst: u64,
        src: u32,
        len: u32,
    ) -> Result<(), TrapKind> {
        todo!()
    }

    pub fn data_drop(&mut self, data_index: DataIndex) {
        self.dropped_data.insert(data_index);
    }

    pub fn table_element_type(&self, table_index: TableIndex) -> TableElementType {
        match self.translated_module().tables[table_index]
            .element_type
            .heap_type
            .inner
        {
            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => TableElementType::Func,
            WasmHeapTypeInner::Extern
            | WasmHeapTypeInner::NoExtern
            | WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => TableElementType::GcRef,

            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => {
                todo!("exception-handling proposal")
            }
            WasmHeapTypeInner::Cont
            | WasmHeapTypeInner::ConcreteCont(_)
            | WasmHeapTypeInner::NoCont => todo!("stack switching proposal"),
        }
    }

    pub fn table_grow(
        &mut self,
        table_index: TableIndex,
        delta: u64,
        init_value: TableElement,
    ) -> crate::Result<Option<usize>> {
        let res = self
            .with_defined_table_index_and_instance(table_index, |def_index, instance| {
                instance.tables[def_index].grow(delta, init_value)
            })?;

        Ok(res)
    }

    pub fn table_fill(
        &mut self,
        table_index: TableIndex,
        dst: u64,
        val: TableElement,
        len: u64,
    ) -> Result<(), TrapKind> {
        self.with_defined_table_index_and_instance(table_index, |def_index, instance| {
            instance.tables[def_index].fill(dst, val, len)
        })
    }

    pub fn table_init(
        &mut self,
        store: &mut StoreOpaque,
        table_index: TableIndex,
        elem_index: ElemIndex,
        dst: u64,
        src: u64,
        len: u64,
    ) -> Result<(), TrapKind> {
        let module = self.module().clone(); // FIXME this clone is here to workaround lifetime issues. remove
        let elements = &module.translated().passive_table_initializers[&elem_index];
        // TODO reuse this const_eval across calls
        let mut const_eval = ConstExprEvaluator::default();
        self.table_init_segment(store, &mut const_eval, table_index, elements, dst, src, len)
    }

    fn table_init_segment(
        &mut self,
        store: &mut StoreOpaque,
        const_eval: &mut ConstExprEvaluator,
        table_index: TableIndex,
        elements: &TableSegmentElements,
        dst: u64,
        src: u64,
        len: u64,
    ) -> Result<(), TrapKind> {
        let src = usize::try_from(src).map_err(|_| TrapKind::TableOutOfBounds)?;
        let len = usize::try_from(len).map_err(|_| TrapKind::TableOutOfBounds)?;

        // Safety: the implementation promises that vmctx is correctly initialized
        let table = unsafe { self.defined_or_imported_table(table_index).as_mut() };

        match elements {
            TableSegmentElements::Functions(funcs) => {
                let elements = funcs
                    .get(src..)
                    .and_then(|s| s.get(..len))
                    .ok_or(TrapKind::TableOutOfBounds)?;
                table.init_func(dst, elements.iter().map(|idx| self.get_func_ref(*idx)))?;
            }
            TableSegmentElements::Expressions(exprs) => {
                let exprs = exprs
                    .get(src..)
                    .and_then(|s| s.get(..len))
                    .ok_or(TrapKind::TableOutOfBounds)?;
                let (heap_top_ty, shared) = self.translated_module().tables[table_index]
                    .element_type
                    .heap_type
                    .top();
                assert!(!shared);

                // Safety: the implementation promises that vmctx is correctly initialized
                let mut context = unsafe { ConstEvalContext::new(self) };

                match heap_top_ty {
                    WasmHeapTopType::Func => table.init_func(
                        dst,
                        exprs.iter().map(|expr| {
                            NonNull::new(
                                const_eval
                                    .eval(store, &mut context, expr)
                                    .expect("const expr should be valid")
                                    .get_funcref()
                                    .cast(),
                            )
                        }),
                    )?,
                    WasmHeapTopType::Extern | WasmHeapTopType::Any => todo!("gc proposal"),
                    WasmHeapTopType::Exn => todo!("exception-handling proposal"),
                    WasmHeapTopType::Cont => todo!("continuation proposal"),
                }
            }
        }

        Ok(())
    }

    pub fn elem_drop(&mut self, elem_index: ElemIndex) {
        self.dropped_elements.insert(elem_index);
    }

    pub fn get_func_ref(&mut self, index: FuncIndex) -> Option<NonNull<VMFuncRef>> {
        if index == FuncIndex::reserved_value() {
            return None;
        }

        // Safety: we have a `&mut self`, so we have exclusive access
        // to this Instance.
        unsafe {
            let func = &self.translated_module().functions[index];
            let func_ref: *mut VMFuncRef = self
                .vmctx_plus_offset_mut::<VMFuncRef>(self.vmshape().vmctx_vmfunc_ref(func.func_ref));

            Some(NonNull::new(func_ref).unwrap())
        }
    }

    pub(crate) fn set_store(&mut self, store: Option<NonNull<StoreOpaque>>) {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            self.store = store;

            if let Some(mut store) = store {
                let store = store.as_mut();

                self.vm_store_context()
                    .write(Some(VmPtr::from(store.vm_store_context_ptr())));
                #[cfg(target_has_atomic = "64")]
                self.epoch_ptr().write(Some(VmPtr::from(NonNull::from(
                    store.engine().epoch_counter(),
                ))));

                // if self.env_module().needs_gc_heap {
                //     self.set_gc_heap(Some(store.gc_store_mut().expect(
                //         "if we need a GC heap, then `Instance::new_raw` should have already \
                //          allocated it for us",
                //     )));
                // } else {
                //     self.set_gc_heap(None);
                // }
            } else {
                self.vm_store_context().write(None);
                #[cfg(target_has_atomic = "64")]
                self.epoch_ptr().write(None);
                // self.set_gc_heap(None);
            }
        }
    }

    // unsafe fn set_gc_heap(&mut self, gc_store: Option<&mut StoreOpaque>) {
    //     if let Some(gc_store) = gc_store {
    //         let heap = gc_store.gc_heap.heap_slice_mut();
    //         self.gc_heap_bound().write(heap.len());
    //         self.gc_heap_base()
    //             .write(Some(NonNull::from(heap).cast().into()));
    //         self.gc_heap_data()
    //             .write(Some(gc_store.gc_heap.vmctx_gc_heap_data().into()));
    //     } else {
    //         self.gc_heap_bound().write(0);
    //         self.gc_heap_base().write(None);
    //         self.gc_heap_data().write(None);
    //     }
    // }

    pub(crate) fn set_callee(&mut self, callee: Option<NonNull<VMFunctionBody>>) {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            let callee = callee.map(VmPtr::from);
            self.vmctx_plus_offset_mut::<Option<VmPtr<VMFunctionBody>>>(
                StaticVMShape.vmctx_callee(),
            )
            .write(callee);
        }
    }

    // VMContext accessors

    #[inline]
    pub unsafe fn from_vmctx<R>(
        vmctx: NonNull<VMContext>,
        f: impl FnOnce(&mut Instance) -> R,
    ) -> R {
        // Safety: ensured by caller
        unsafe {
            let mut ptr = vmctx.byte_sub(size_of::<Instance>()).cast::<Instance>();
            f(ptr.as_mut())
        }
    }

    /// Return a reference to the vmctx used by compiled wasm code.
    #[inline]
    pub fn vmctx(&self) -> NonNull<VMContext> {
        let addr = &raw const self.vmctx;
        let ret = self.vmctx_self_reference.as_ptr().with_addr(addr.addr());
        NonNull::new(ret).unwrap()
    }

    /// Helper function to access various locations offset from our `*mut
    /// VMContext` object.
    ///
    /// # Safety
    ///
    /// This method is unsafe because the `offset` must be within bounds of the
    /// `VMContext` object trailing this instance.
    unsafe fn vmctx_plus_offset<T: VmSafe>(&self, offset: impl Into<u32>) -> *const T {
        // Safety: ensured by caller
        unsafe {
            self.vmctx()
                .as_ptr()
                .byte_add(usize::try_from(offset.into()).unwrap())
                .cast()
        }
    }
    /// Dual of `vmctx_plus_offset`, but for mutability.
    unsafe fn vmctx_plus_offset_mut<T: VmSafe>(&mut self, offset: impl Into<u32>) -> *mut T {
        // Safety: ensured by caller
        unsafe {
            self.vmctx()
                .as_ptr()
                .byte_add(usize::try_from(offset.into()).unwrap())
                .cast()
        }
    }

    #[inline]
    pub fn vm_store_context(&mut self) -> NonNull<Option<VmPtr<VMStoreContext>>> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(StaticVMShape.vmctx_store_context())).unwrap()
        }
    }

    /// Return a pointer to the global epoch counter used by this instance.
    #[cfg(target_has_atomic = "64")]
    pub fn epoch_ptr(&mut self) -> NonNull<Option<VmPtr<AtomicU64>>> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(StaticVMShape.vmctx_epoch_ptr())).unwrap()
        }
    }

    /// Return a pointer to the GC heap base pointer.
    pub fn gc_heap_base(&mut self) -> NonNull<Option<VmPtr<u8>>> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_base())).unwrap()
        }
    }

    /// Return a pointer to the GC heap bound.
    pub fn gc_heap_bound(&mut self) -> NonNull<usize> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_bound())).unwrap()
        }
    }

    /// Return a pointer to the collector-specific heap data.
    pub fn gc_heap_data(&mut self) -> NonNull<Option<VmPtr<u8>>> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_data())).unwrap()
        }
    }

    /// Return the indexed `VMFunctionImport`.
    fn imported_function(&self, index: FuncIndex) -> &VMFunctionImport {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmfunction_import(index)) }
    }
    /// Return the index `VMTable`.
    fn imported_table(&self, index: TableIndex) -> &VMTableImport {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmtable_import(index)) }
    }
    /// Return the indexed `VMMemoryImport`.
    fn imported_memory(&self, index: MemoryIndex) -> &VMMemoryImport {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmmemory_import(index)) }
    }
    /// Return the indexed `VMGlobalImport`.
    fn imported_global(&self, index: GlobalIndex) -> &VMGlobalImport {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmglobal_import(index)) }
    }
    /// Return the indexed `VMTagImport`.
    fn imported_tag(&self, index: TagIndex) -> &VMTagImport {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmtag_import(index)) }
    }

    fn table_ptr(&mut self, index: DefinedTableIndex) -> NonNull<VMTableDefinition> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmtable_definition(index)))
                .unwrap()
        }
    }
    fn memory_ptr(&mut self, index: DefinedMemoryIndex) -> NonNull<VMMemoryDefinition> {
        // Safety: the implementation promises that vmctx is correctly initialized
        let ptr = unsafe {
            *self.vmctx_plus_offset::<VmPtr<_>>(self.vmshape().vmctx_vmmemory_pointer(index))
        };
        ptr.as_non_null()
    }
    fn global_ptr(&mut self, index: DefinedGlobalIndex) -> NonNull<VMGlobalDefinition> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(
                self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmglobal_definition(index)),
            )
            .unwrap()
        }
    }
    fn tag_ptr(&mut self, index: DefinedTagIndex) -> NonNull<VMTagDefinition> {
        // Safety: the implementation promises that vmctx is correctly initialized
        unsafe {
            NonNull::new(self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmtag_definition(index)))
                .unwrap()
        }
    }

    pub fn get_defined_table(&mut self, index: DefinedTableIndex) -> NonNull<Table> {
        NonNull::from(&mut self.tables[index])
    }

    pub(super) fn defined_or_imported_table(&mut self, table_index: TableIndex) -> NonNull<Table> {
        self.with_defined_table_index_and_instance(table_index, |idx, instance| {
            NonNull::from(instance.tables.get(idx).unwrap())
        })
    }

    fn with_defined_table_index_and_instance<R>(
        &mut self,
        index: TableIndex,
        f: impl FnOnce(DefinedTableIndex, &mut Instance) -> R,
    ) -> R {
        if let Some(defined_table_index) = self.translated_module().defined_table_index(index) {
            f(defined_table_index, self)
        } else {
            let import = self.imported_table(index);
            // Safety: the VMTableImport needs should be correct. TODO test & verify
            unsafe {
                Instance::from_vmctx(import.vmctx.as_non_null(), |foreign_instance| {
                    let foreign_table_def = import.from.as_ptr();
                    let foreign_table_index = foreign_instance.table_index(&*foreign_table_def);
                    f(foreign_table_index, foreign_instance)
                })
            }
        }
    }

    pub(super) fn defined_or_imported_memory(
        &mut self,
        index: MemoryIndex,
    ) -> NonNull<VMMemoryDefinition> {
        if let Some(defined_index) = self.translated_module().defined_memory_index(index) {
            self.memory_ptr(defined_index)
        } else {
            let import = self.imported_memory(index);
            import.from.as_non_null()
        }
    }

    pub(super) fn defined_or_imported_global(
        &mut self,
        index: GlobalIndex,
    ) -> NonNull<VMGlobalDefinition> {
        if let Some(index) = self.translated_module().defined_global_index(index) {
            self.global_ptr(index)
        } else {
            self.imported_global(index).from.as_non_null()
        }
    }

    pub unsafe fn table_index(&mut self, table: &VMTableDefinition) -> DefinedTableIndex {
        // Safety: ensured by caller
        unsafe {
            let index = DefinedTableIndex::new(
                usize::try_from(
                    ptr::from_ref::<VMTableDefinition>(table)
                        .offset_from(self.table_ptr(DefinedTableIndex::new(0)).as_ptr()),
                )
                .unwrap(),
            );
            assert!(index.index() < self.tables.len());
            index
        }
    }

    pub unsafe fn memory_index(&mut self, table: &VMMemoryDefinition) -> DefinedMemoryIndex {
        // Safety: ensured by caller
        unsafe {
            let index = DefinedMemoryIndex::new(
                usize::try_from(
                    ptr::from_ref::<VMMemoryDefinition>(table)
                        .offset_from(self.memory_ptr(DefinedMemoryIndex::new(0)).as_ptr()),
                )
                .unwrap(),
            );
            assert!(index.index() < self.memories.len());
            index
        }
    }

    #[tracing::instrument(level = "debug", skip(self, store, module))]
    unsafe fn initialize_vmctx(
        &mut self,
        store: &mut StoreOpaque,
        imports: Imports,
        module: &Module,
    ) {
        let vmshape = module.vmshape();

        // Safety: there is no safety, we just have to trust that the entire vmctx memory range
        // we need was correctly allocated
        unsafe {
            // initialize vmctx magic
            tracing::trace!("initializing vmctx magic");
            self.vmctx_plus_offset_mut::<u32>(vmshape.vmctx_magic())
                .write(VMCONTEXT_MAGIC);

            tracing::trace!("initializing store-related fields");
            self.set_store(Some(NonNull::from(store)));

            tracing::trace!("initializing built-in functions array ptr");
            self.vmctx_plus_offset_mut::<VmPtr<VMBuiltinFunctionsArray>>(
                vmshape.vmctx_builtin_functions(),
            )
            .write(VmPtr::from(NonNull::from(&VMBuiltinFunctionsArray::INIT)));

            tracing::trace!("initializing callee");
            self.set_callee(None);

            //      gc_heap_base: *mut u8,
            //      gc_heap_bound: *mut u8,
            //      gc_heap_data: *mut T, //! Collector-specific pointer

            self.vmctx_plus_offset_mut::<VmPtr<VMSharedTypeIndex>>(vmshape.vmctx_type_ids_array())
                .write(VmPtr::from(NonNull::from(self.module.type_ids()).cast()));

            // initialize imports
            tracing::trace!("initializing function imports");
            debug_assert_eq!(
                imports.functions.len(),
                self.translated_module().num_imported_functions as usize
            );
            ptr::copy_nonoverlapping(
                imports.functions.as_ptr(),
                self.vmctx_plus_offset_mut::<VMFunctionImport>(
                    vmshape.vmctx_imported_functions_begin(),
                ),
                imports.functions.len(),
            );

            tracing::trace!("initializing table imports");
            debug_assert_eq!(
                imports.tables.len(),
                self.translated_module().num_imported_tables as usize
            );
            ptr::copy_nonoverlapping(
                imports.tables.as_ptr(),
                self.vmctx_plus_offset_mut::<VMTableImport>(vmshape.vmctx_imported_tables_begin()),
                imports.tables.len(),
            );

            tracing::trace!("initializing memory imports");
            debug_assert_eq!(
                imports.memories.len(),
                self.translated_module().num_imported_memories as usize
            );
            ptr::copy_nonoverlapping(
                imports.memories.as_ptr(),
                self.vmctx_plus_offset_mut::<VMMemoryImport>(
                    vmshape.vmctx_imported_memories_begin(),
                ),
                imports.memories.len(),
            );

            tracing::trace!("initializing global imports");
            debug_assert_eq!(
                imports.globals.len(),
                self.translated_module().num_imported_globals as usize
            );
            ptr::copy_nonoverlapping(
                imports.globals.as_ptr(),
                self.vmctx_plus_offset_mut::<VMGlobalImport>(
                    vmshape.vmctx_imported_globals_begin(),
                ),
                imports.globals.len(),
            );

            tracing::trace!("initializing tag imports");
            debug_assert_eq!(
                imports.tags.len(),
                self.translated_module().num_imported_tags as usize
            );
            ptr::copy_nonoverlapping(
                imports.tags.as_ptr(),
                self.vmctx_plus_offset_mut::<VMTagImport>(vmshape.vmctx_imported_tags_begin()),
                imports.tags.len(),
            );

            // initialize defined tables
            tracing::trace!("initializing defined tables");
            for def_index in module
                .translated()
                .tables
                .keys()
                .filter_map(|index| module.translated().defined_table_index(index))
            {
                let def = self.tables[def_index].as_vmtable_definition();
                self.table_ptr(def_index).write(def);
            }

            // Initialize the defined memories. This fills in both the
            // `defined_memories` table and the `owned_memories` table at the same
            // time. Entries in `defined_memories` hold a pointer to a definition
            // (all memories) whereas the `owned_memories` hold the actual
            // definitions of memories owned (not shared) in the module.
            tracing::trace!("initializing defined memories");
            for (def_index, desc) in
                module
                    .translated()
                    .memories
                    .iter()
                    .filter_map(|(index, desc)| {
                        Some((module.translated().defined_memory_index(index)?, desc))
                    })
            {
                let ptr = self.vmctx_plus_offset_mut::<VmPtr<VMMemoryDefinition>>(
                    vmshape.vmctx_vmmemory_pointer(def_index),
                );

                if desc.shared {
                    // let def_ptr = self.memories[def_index]
                    //     .as_shared_memory()
                    //     .unwrap()
                    //     .vmmemory_ptr();
                    // ptr.write(VmPtr::from(def_ptr));

                    todo!()
                } else {
                    let owned_index = self.translated_module().owned_memory_index(def_index);
                    let owned_ptr = self.vmctx_plus_offset_mut::<VMMemoryDefinition>(
                        vmshape.vmctx_vmmemory_definition(owned_index),
                    );

                    owned_ptr.write(self.memories[def_index].vmmemory_definition());
                    ptr.write(VmPtr::from(NonNull::new(owned_ptr).unwrap()));
                }
            }

            // Zero-initialize the globals so that nothing is uninitialized memory
            // after this function returns. The globals are actually initialized
            // with their const expression initializers after the instance is fully
            // allocated.
            tracing::trace!("initializing defined globals");
            for (index, _init) in &module.translated().global_initializers {
                self.global_ptr(index).write(VMGlobalDefinition::new());
            }

            tracing::trace!("initializing defined tags");
            for (def_index, tag) in
                module.translated().tags.iter().filter_map(|(index, ty)| {
                    Some((module.translated().defined_tag_index(index)?, ty))
                })
            {
                self.tag_ptr(def_index).write(VMTagDefinition::new(
                    tag.signature.unwrap_engine_type_index(),
                ));
            }

            tracing::trace!("initializing func refs array");
            self.initialize_vmfunc_refs(&imports, module);
        }
    }

    /// # Safety
    ///
    /// among other things the caller has to ensure that this is only ever called **after**
    /// calling `Instance::initialize_vmctx`
    #[tracing::instrument(level = "debug", skip(self, module))]
    unsafe fn initialize_vmfunc_refs(&mut self, imports: &Imports, module: &Module) {
        // Safety: the caller pinky-promised that the vmctx is correctly initialized
        unsafe {
            let vmshape = module.vmshape();

            for (index, func) in module
                .translated()
                .functions
                .iter()
                .filter(|(_, f)| f.is_escaping())
            {
                let type_index = {
                    let base: *const VMSharedTypeIndex = (*self
                        .vmctx_plus_offset_mut::<VmPtr<VMSharedTypeIndex>>(
                            StaticVMShape.vmctx_type_ids_array(),
                        ))
                    .as_ptr();
                    *base.add(func.signature.unwrap_module_type_index().index())
                };

                let func_ref =
                    if let Some(def_index) = module.translated().defined_func_index(index) {
                        VMFuncRef {
                            array_call: self.module().array_to_wasm_trampoline(def_index).expect(
                                "should have array-to-Wasm trampoline for escaping function",
                            ),
                            wasm_call: Some(VmPtr::from(self.module.function(def_index))),
                            type_index,
                            vmctx: VmPtr::from(VMOpaqueContext::from_vmcontext(self.vmctx())),
                        }
                    } else {
                        let import = &imports.functions[index.index()];
                        VMFuncRef {
                            array_call: import.array_call,
                            wasm_call: Some(import.wasm_call),
                            vmctx: import.vmctx,
                            type_index,
                        }
                    };

                self.vmctx_plus_offset_mut::<VMFuncRef>(vmshape.vmctx_vmfunc_ref(func.func_ref))
                    .write(func_ref);
            }
        }
    }

    /// # Safety
    ///
    /// among other things the caller has to ensure that this is only ever called **after**
    /// calling `Instance::initialize_vmctx`
    #[tracing::instrument(level = "debug", skip(self, store, ctx, const_eval, module))]
    unsafe fn initialize_globals(
        &mut self,
        store: &mut StoreOpaque,
        ctx: &mut ConstEvalContext,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<()> {
        for (def_index, init) in &module.translated().global_initializers {
            let vmval = const_eval
                .eval(store, ctx, init)
                .expect("const expression should be valid");
            let index = self.translated_module().global_index(def_index);
            let ty = self.translated_module().globals[index].content_type;

            // Safety: the caller pinky-promised that the vmctx is correctly initialized
            unsafe {
                self.global_ptr(def_index)
                    .write(VMGlobalDefinition::from_vmval(store, ty, vmval)?);
            }
        }

        Ok(())
    }

    /// # Safety
    ///
    /// among other things the caller has to ensure that this is only ever called **after**
    /// calling `Instance::initialize_vmctx`
    #[tracing::instrument(level = "debug", skip(self, store, ctx, const_eval, module))]
    unsafe fn initialize_tables(
        &mut self,
        store: &mut StoreOpaque,
        ctx: &mut ConstEvalContext,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<()> {
        // update initial values
        for (def_index, init) in &module.translated().table_initializers.initial_values {
            match init {
                TableInitialValue::RefNull => {}
                TableInitialValue::ConstExpr(expr) => {
                    let index = self.translated_module().table_index(def_index);
                    let (heap_top_ty, shared) = self.translated_module().tables[index]
                        .element_type
                        .heap_type
                        .top();
                    assert!(!shared);

                    let vmval = const_eval
                        .eval(store, ctx, expr)
                        .expect("const expression should be valid");

                    // Safety: the caller pinky-promised that the vmctx is correctly initialized
                    let table = unsafe { self.get_defined_table(def_index).as_mut() };

                    match heap_top_ty {
                        WasmHeapTopType::Func => {
                            let funcref = NonNull::new(vmval.get_funcref().cast::<VMFuncRef>());
                            let items = (0..table.size()).map(|_| funcref);
                            table.init_func(0, items)?;
                        }
                        WasmHeapTopType::Extern | WasmHeapTopType::Any => todo!("gc proposal"),
                        WasmHeapTopType::Exn => todo!("exception-handling proposal"),
                        WasmHeapTopType::Cont => todo!("continuation proposal"),
                    }
                }
            }
        }

        // run active elements
        for segment in &module.translated().table_initializers.segments {
            let start = const_eval
                .eval(store, ctx, &segment.offset)
                .expect("const expression should be valid");

            ctx.instance.table_init_segment(
                store,
                const_eval,
                segment.table_index,
                &segment.elements,
                start.get_u64(),
                0,
                segment.elements.len(),
            )?;
        }

        Ok(())
    }

    /// # Safety
    ///
    /// among other things the caller has to ensure that this is only ever called **after**
    /// calling `Instance::initialize_vmctx`
    #[tracing::instrument(level = "debug", skip(self, store, ctx, const_eval, module))]
    unsafe fn initialize_memories(
        &mut self,
        store: &mut StoreOpaque,
        ctx: &mut ConstEvalContext,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<()> {
        for initializer in &module.translated().memory_initializers {
            let start: usize = {
                let vmval = const_eval
                    .eval(store, ctx, &initializer.offset)
                    .expect("const expression should be valid");

                match self.translated_module().memories[initializer.memory_index].index_type {
                    IndexType::I32 => usize::try_from(vmval.get_u32()).unwrap(),
                    IndexType::I64 => usize::try_from(vmval.get_u64()).unwrap(),
                }
            };

            // Safety: the caller pinky-promised that the vmctx is correctly initialized
            let memory = unsafe {
                self.defined_or_imported_memory(initializer.memory_index)
                    .as_mut()
            };

            let end = start.checked_add(initializer.data.len()).unwrap();
            ensure!(end <= memory.current_length(Ordering::Relaxed));

            // Safety: we did all the checking we could above
            unsafe {
                let src = &initializer.data;
                let dst = memory.base.as_ptr().add(start);
                ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }

        Ok(())
    }
}

#[repr(transparent)]
pub struct InstanceAndStore {
    instance: Instance,
}

impl InstanceAndStore {
    #[inline]
    pub(crate) unsafe fn from_vmctx<R>(
        vmctx: NonNull<VMContext>,
        f: impl for<'a> FnOnce(&'a mut Self) -> R,
    ) -> R {
        const_assert_eq!(size_of::<InstanceAndStore>(), size_of::<Instance>());
        // Safety: the instance is always directly before the vmctx in memory
        unsafe {
            let mut ptr = vmctx
                .byte_sub(size_of::<Instance>())
                .cast::<InstanceAndStore>();

            f(ptr.as_mut())
        }
    }

    #[inline]
    pub(crate) fn unpack_mut(&mut self) -> (&mut Instance, &mut StoreOpaque) {
        // Safety: this is fine
        unsafe {
            let store = self.instance.store.unwrap().as_mut();
            (&mut self.instance, store)
        }
    }

    #[inline]
    pub(crate) unsafe fn unpack_with_state_mut<T>(
        &mut self,
    ) -> (&mut Instance, &'_ mut StoreInner<T>) {
        let mut store_ptr = self.instance.store.unwrap().cast::<StoreInner<T>>();
        (
            &mut self.instance,
            // Safety: ensured by caller
            unsafe { store_ptr.as_mut() },
        )
    }
}

/// # Safety
///
/// The caller must ensure this function is only ever called **after** `Instance::initialize_vmctx`
unsafe fn check_init_bounds(
    store: &mut StoreOpaque,
    instance: &mut Instance,
    module: &Module,
) -> crate::Result<()> {
    // Safety: ensured by caller
    unsafe {
        check_table_init_bounds(store, instance, module)?;
        check_memory_init_bounds(store, instance, &module.translated().memory_initializers)?;
    }
    Ok(())
}

/// # Safety
///
/// The caller must ensure this function is only ever called **after** `Instance::initialize_vmctx`
unsafe fn check_table_init_bounds(
    store: &mut StoreOpaque,
    instance: &mut Instance,
    module: &Module,
) -> crate::Result<()> {
    // Safety: the caller pinky-promised to have called initialize_vmctx before calling this function
    // so the VMTableDefinitions are all properly initialized
    unsafe {
        let mut const_evaluator = ConstExprEvaluator::default();

        for segment in &module.translated().table_initializers.segments {
            let table = instance
                .defined_or_imported_table(segment.table_index)
                .as_ref();
            let mut context = ConstEvalContext::new(instance);
            let start = const_evaluator
                .eval(store, &mut context, &segment.offset)
                .expect("const expression should be valid");
            let start = usize::try_from(start.get_u32()).unwrap();
            let end = start.checked_add(usize::try_from(segment.elements.len()).unwrap());

            match end {
                Some(end) if end <= table.size() => {
                    // Initializer is in bounds
                }
                _ => {
                    bail!("table out of bounds: elements segment does not fit")
                }
            }
        }
        Ok(())
    }
}

/// # Safety
///
/// The caller must ensure this function is only ever called **after** `Instance::initialize_vmctx`
unsafe fn check_memory_init_bounds(
    store: &mut StoreOpaque,
    instance: &mut Instance,
    initializers: &[MemoryInitializer],
) -> crate::Result<()> {
    // Safety: the caller pinky-promised to have called initialize_vmctx before calling this function
    // so the VMMemoryDefinitions are all properly initialized
    unsafe {
        for init in initializers {
            let memory = instance
                .defined_or_imported_memory(init.memory_index)
                .as_ref();
            let start = get_memory_init_start(store, init, instance)?;
            let end = usize::try_from(start)
                .ok()
                .and_then(|start| start.checked_add(init.data.len()));

            match end {
                Some(end) if end <= memory.current_length(Ordering::Relaxed) => {
                    // Initializer is in bounds
                }
                _ => {
                    bail!("memory out of bounds: data segment does not fit")
                }
            }
        }

        Ok(())
    }
}

/// # Safety
///
/// The caller must ensure this function is only ever called **after** `Instance::initialize_vmctx`
unsafe fn get_memory_init_start(
    store: &mut StoreOpaque,
    init: &MemoryInitializer,
    instance: &mut Instance,
) -> crate::Result<u64> {
    // Safety: the caller pinky-promised that the vmctx is correctly initialized
    let mut context = unsafe { ConstEvalContext::new(instance) };
    let mut const_evaluator = ConstExprEvaluator::default();
    const_evaluator
        .eval(store, &mut context, &init.offset)
        .map(
            |v| match instance.translated_module().memories[init.memory_index].index_type {
                IndexType::I32 => v.get_u32().into(),
                IndexType::I64 => v.get_u64(),
            },
        )
}
