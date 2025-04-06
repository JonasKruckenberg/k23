// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::indices::{
    DataIndex, DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, DefinedTagIndex,
    ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TagIndex, VMSharedTypeIndex,
};
use crate::wasm::module::Module;
use crate::wasm::store::StoreOpaque;
use crate::wasm::translate::{
    IndexType, TableInitialValue, TableSegmentElements, TranslatedModule, WasmHeapTopType,
};
use crate::wasm::vm::const_eval::{ConstEvalContext, ConstExprEvaluator};
use crate::wasm::vm::instance_alloc::InstanceAllocation;
use crate::wasm::vm::memory::Memory;
use crate::wasm::vm::provenance::{VmPtr, VmSafe};
use crate::wasm::vm::table::{Table, TableElement, TableElementType};
use crate::wasm::vm::{
    CodeMemory, Imports, StaticVMShape, VMBuiltinFunctionsArray, VMContext, VMFuncRef,
    VMFunctionBody, VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMOpaqueContext, VMShape, VMStoreContext, VMTableDefinition,
    VMTableImport, VMTagDefinition, VMTagImport, VMCONTEXT_MAGIC,
};
use crate::wasm::Trap;
use anyhow::ensure;
use core::alloc::Layout;
use core::marker::PhantomPinned;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use core::{ptr};
use cranelift_entity::{EntityRef, EntitySet, PrimaryMap};

#[derive(Debug)]
pub struct InstanceHandle {
    instance: NonNull<Instance>,
}
unsafe impl Send for InstanceHandle {}
unsafe impl Sync for InstanceHandle {}

#[repr(C)] // ensure that the vmctx field is last.
pub struct Instance {
    module: Module,
    memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    tables: PrimaryMap<DefinedTableIndex, Table>,
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

impl Instance {
    pub unsafe fn new_unchecked(
        store: &mut StoreOpaque,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<InstanceHandle> {
        let InstanceAllocation {
            mut ptr,
            tables,
            memories,
        } = store.alloc().allocate_module(&module)?;

        let dropped_elements = module.translated().active_table_initializers.clone();
        let dropped_data = module.translated().active_memory_initializers.clone();

        unsafe {
            ptr.write(Instance {
                module: module.clone(),
                memories,
                tables,
                dropped_elements,
                dropped_data,
                vmctx_self_reference: ptr.add(1).cast(),
                store: None,
                vmctx: VMContext {
                    _marker: PhantomPinned,
                },
            });

            ptr.as_mut().initialize_vmctx(imports, &module);

            let mut ctx = ConstEvalContext::new(ptr.as_mut());
            ptr.as_mut()
                .initialize_tables(store, &mut ctx, const_eval, &module)?;
            ptr.as_mut()
                .initialize_memories(store, &mut ctx, const_eval, &module)?;
            ptr.as_mut()
                .initialize_globals(store, &mut ctx, const_eval, &module)?;
        }

        Ok(InstanceHandle { instance: ptr })
    }

    unsafe fn initialize_vmctx(&mut self, imports: Imports, module: &Module) {
        let vmshape = module.vmshape();

        unsafe {
            // initialize vmctx magic
            tracing::trace!("initializing vmctx magic");
            self.vmctx_plus_offset_mut(vmshape.vmctx_magic())
                .write(VMCONTEXT_MAGIC);

            // self.set_store(store.as_raw());
            //      vm_store_context: *const VMStoreContext,
            //      epoch_ptr: *mut AtomicU64,

            // Initialize the built-in functions
            tracing::trace!("initializing built-in functions array ptr");
            self.vmctx_plus_offset_mut(vmshape.vmctx_builtin_functions())
                .write(VMBuiltinFunctionsArray::INIT);

            // self.set_callee(None);

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
                self.vmctx_plus_offset_mut(vmshape.vmctx_imported_functions_begin())
                    .as_ptr(),
                imports.functions.len(),
            );

            tracing::trace!("initializing table imports");
            debug_assert_eq!(
                imports.tables.len(),
                self.translated_module().num_imported_tables as usize
            );
            ptr::copy_nonoverlapping(
                imports.tables.as_ptr(),
                self.vmctx_plus_offset_mut(vmshape.vmctx_imported_tables_begin())
                    .as_ptr(),
                imports.tables.len(),
            );

            tracing::trace!("initializing memory imports");
            debug_assert_eq!(
                imports.memories.len(),
                self.translated_module().num_imported_memories as usize
            );
            ptr::copy_nonoverlapping(
                imports.memories.as_ptr(),
                self.vmctx_plus_offset_mut(vmshape.vmctx_imported_memories_begin())
                    .as_ptr(),
                imports.memories.len(),
            );

            tracing::trace!("initializing global imports");
            debug_assert_eq!(
                imports.globals.len(),
                self.translated_module().num_imported_globals as usize
            );
            ptr::copy_nonoverlapping(
                imports.globals.as_ptr(),
                self.vmctx_plus_offset_mut(vmshape.vmctx_imported_globals_begin())
                    .as_ptr(),
                imports.globals.len(),
            );

            tracing::trace!("initializing tag imports");
            debug_assert_eq!(
                imports.tags.len(),
                self.translated_module().num_imported_tags as usize
            );
            ptr::copy_nonoverlapping(
                imports.tags.as_ptr(),
                self.vmctx_plus_offset_mut(vmshape.vmctx_imported_tags_begin())
                    .as_ptr(),
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
                    ptr.write(VmPtr::from(owned_ptr));
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

    #[tracing::instrument(skip(self))]
    unsafe fn initialize_vmfunc_refs(&mut self, imports: &Imports, module: &Module) {
        unsafe {
            let vmshape = module.vmshape();

            for (index, func) in module
                .translated()
                .functions
                .iter()
                .filter(|(_, f)| f.is_escaping())
            {
                let func_ref =
                    if let Some(def_index) = module.translated().defined_func_index(index) {
                        VMFuncRef {
                            array_call: VmPtr::from(
                                self.module().array_to_wasm_trampoline(def_index).unwrap(),
                            ),
                            wasm_call: Some(VmPtr::from(self.module.function(def_index))),
                            type_index: Default::default(),
                            vmctx: VmPtr::from(VMOpaqueContext::from_vmcontext(self.vmctx())),
                        }
                    } else {
                        let import = &imports.functions[index.index()];
                        VMFuncRef {
                            array_call: import.array_call,
                            wasm_call: import.wasm_call,
                            vmctx: import.vmctx,
                            type_index: func.signature.unwrap_engine_type_index(),
                        }
                    };

                self.vmctx_plus_offset_mut(vmshape.vmctx_vmfunc_ref(func.func_ref))
                    .write(func_ref);
            }
        }
    }

    #[tracing::instrument(skip(self, store, ctx, const_eval))]
    unsafe fn initialize_globals(
        &mut self,
        store: &mut StoreOpaque,
        ctx: &mut ConstEvalContext,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<()> {
        for (def_index, init) in module.translated().global_initializers.iter() {
            let vmval = const_eval
                .eval(store, ctx, init)
                .expect("const expression should be valid");
            let index = self.translated_module().global_index(def_index);
            let ty = self.translated_module().globals[index].content_type;

            unsafe {
                self.global_ptr(def_index)
                    .write(VMGlobalDefinition::from_vmval(store, ty, vmval)?);
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, store, ctx, const_eval))]
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
                    assert_eq!(shared, false);

                    let vmval = const_eval
                        .eval(store, ctx, expr)
                        .expect("const expression should be valid");

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

    #[tracing::instrument(skip(self, store, ctx, const_eval))]
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

            let memory = unsafe {
                self.defined_or_imported_memory(initializer.memory_index)
                    .as_mut()
            };

            let end = start.checked_add(initializer.data.len()).unwrap();
            ensure!(end <= memory.current_length(Ordering::Relaxed));

            unsafe {
                let src = &initializer.data;
                let dst = memory.base.as_ptr().add(start);
                ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }

        Ok(())
    }

    fn alloc_layout(offsets: &VMShape) -> Layout {
        let size = size_of::<Self>()
            .checked_add(usize::try_from(offsets.size_of_vmctx()).unwrap())
            .unwrap();
        let align = align_of::<Self>();
        Layout::from_size_align(size, align).unwrap()
    }

    pub fn module(&self) -> &Module {
        &self.module
    }
    pub fn code(&self) -> &CodeMemory {
        self.module.code()
    }
    pub fn translated_module(&self) -> &TranslatedModule {
        self.module.translated()
    }
    pub fn vmshape(&self) -> &VMShape {
        self.module.vmshape()
    }

    // pub fn get_exported_func(&mut self, index: FuncIndex) -> ExportedFunction {
    //     todo!()
    // }
    // pub fn get_exported_table(&mut self, index: TableIndex) -> ExportedTable {
    //     todo!()
    // }
    // pub fn get_exported_memory(&mut self, index: MemoryIndex) -> ExportedMemory {
    //     todo!()
    // }
    // pub fn get_exported_global(&mut self, index: GlobalIndex) -> ExportedGlobal {
    //     todo!()
    // }
    // pub fn get_exported_tag(&mut self, index: TableIndex) -> ExportedTable {
    //     todo!()
    // }

    /// Get the given memory's page size, in bytes.
    pub fn memory_page_size(&self, index: MemoryIndex) -> usize {
        todo!()
    }

    pub fn memory_grow(
        &mut self,
        store: &mut StoreOpaque,
        index: MemoryIndex,
        delta: u64,
    ) -> crate::Result<Option<usize>> {
        todo!()
    }

    pub fn memory_copy(
        &mut self,
        dst_index: MemoryIndex,
        dst: u64,
        src_index: MemoryIndex,
        src: u64,
        len: u64,
    ) -> Result<(), Trap> {
        todo!()
    }

    pub fn memory_fill(
        &mut self,
        memory_index: MemoryIndex,
        dst: u64,
        val: u8,
        len: u64,
    ) -> Result<(), Trap> {
        todo!()
    }

    pub fn memory_init(
        &mut self,
        memory_index: MemoryIndex,
        data_index: DataIndex,
        dst: u64,
        src: u32,
        len: u32,
    ) -> Result<(), Trap> {
        todo!()
    }

    pub fn data_drop(&mut self, data_index: DataIndex) {
        todo!()
    }

    pub fn table_element_type(&self, table_index: TableIndex) -> TableElementType {
        todo!()
    }

    pub fn table_grow(
        &mut self,
        store: &mut StoreOpaque,
        table_index: TableIndex,
        delta: u64,
        init_value: TableElement,
    ) -> crate::Result<Option<usize>> {
        todo!()
    }

    pub fn table_fill(
        &mut self,
        table_index: TableIndex,
        dst: u64,
        val: TableElement,
        len: u64,
    ) -> Result<(), Trap> {
        todo!()
    }

    pub fn table_copy(
        &mut self,
        dst_index: TableIndex,
        dst: u64,
        src_index: TableIndex,
        src: u64,
        len: u64,
    ) -> Result<(), Trap> {
        todo!()
    }

    pub fn table_init(
        &mut self,
        store: &mut StoreOpaque,
        table_index: TableIndex,
        elem_index: ElemIndex,
        dst: u64,
        src: u64,
        len: u64,
    ) -> Result<(), Trap> {
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
    ) -> Result<(), Trap> {
        let dst = usize::try_from(dst).map_err(|_| Trap::TableOutOfBounds)?;
        let src = usize::try_from(src).map_err(|_| Trap::TableOutOfBounds)?;
        let len = usize::try_from(len).map_err(|_| Trap::TableOutOfBounds)?;
        let table = unsafe { self.defined_or_imported_table(table_index).as_mut() };

        match elements {
            TableSegmentElements::Functions(funcs) => {
                let elements = funcs
                    .get(src..)
                    .and_then(|s| s.get(..len))
                    .ok_or(Trap::TableOutOfBounds)?;
                table.init_func(dst, elements.iter().map(|idx| self.get_func_ref(*idx)))?;
            }
            TableSegmentElements::Expressions(exprs) => {
                let exprs = exprs
                    .get(src..)
                    .and_then(|s| s.get(..len))
                    .ok_or(Trap::TableOutOfBounds)?;
                let (heap_top_ty, shared) = self.translated_module().tables[table_index]
                    .element_type
                    .heap_type
                    .top();
                assert_eq!(shared, false);

                let mut context = ConstEvalContext::new(self);

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
        todo!()
    }

    pub(crate) unsafe fn set_store(&mut self, store: Option<NonNull<StoreOpaque>>) {
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

    pub(crate) unsafe fn set_callee(&mut self, callee: Option<NonNull<VMFunctionBody>>) {
        let callee = callee.map(|p| VmPtr::from(p));
        self.vmctx_plus_offset_mut(StaticVMShape.vmctx_callee())
            .write(callee);
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
    unsafe fn vmctx_plus_offset_mut<T: VmSafe>(&mut self, offset: impl Into<u32>) -> NonNull<T> {
        // Safety: ensured by caller
        unsafe {
            self.vmctx()
                .byte_add(usize::try_from(offset.into()).unwrap())
                .cast()
        }
    }

    #[inline]
    pub fn vm_store_context(&mut self) -> NonNull<Option<VmPtr<VMStoreContext>>> {
        unsafe { self.vmctx_plus_offset_mut(StaticVMShape.vmctx_store_context()) }
    }

    /// Return a pointer to the global epoch counter used by this instance.
    #[cfg(target_has_atomic = "64")]
    pub fn epoch_ptr(&mut self) -> NonNull<Option<VmPtr<AtomicU64>>> {
        unsafe { self.vmctx_plus_offset_mut(StaticVMShape.vmctx_epoch_ptr()) }
    }

    /// Return a pointer to the GC heap base pointer.
    pub fn gc_heap_base(&mut self) -> NonNull<Option<VmPtr<u8>>> {
        unsafe { self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_base()) }
    }

    /// Return a pointer to the GC heap bound.
    pub fn gc_heap_bound(&mut self) -> NonNull<usize> {
        unsafe { self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_bound()) }
    }

    /// Return a pointer to the collector-specific heap data.
    pub fn gc_heap_data(&mut self) -> NonNull<Option<VmPtr<u8>>> {
        unsafe { self.vmctx_plus_offset_mut(StaticVMShape.vmctx_gc_heap_data()) }
    }

    /// Return the indexed `VMFunctionImport`.
    fn imported_function(&self, index: FuncIndex) -> &VMFunctionImport {
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmfunction_import(index)) }
    }
    /// Return the index `VMTable`.
    fn imported_table(&self, index: TableIndex) -> &VMTableImport {
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmtable_import(index)) }
    }
    /// Return the indexed `VMMemoryImport`.
    fn imported_memory(&self, index: MemoryIndex) -> &VMMemoryImport {
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmmemory_import(index)) }
    }
    /// Return the indexed `VMGlobalImport`.
    fn imported_global(&self, index: GlobalIndex) -> &VMGlobalImport {
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmglobal_import(index)) }
    }
    /// Return the indexed `VMTagImport`.
    fn imported_tag(&self, index: TagIndex) -> &VMTagImport {
        unsafe { &*self.vmctx_plus_offset(self.vmshape().vmctx_vmtag_import(index)) }
    }

    fn table_ptr(&mut self, index: DefinedTableIndex) -> NonNull<VMTableDefinition> {
        unsafe { self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmtable_definition(index)) }
    }
    fn memory_ptr(&mut self, index: DefinedMemoryIndex) -> NonNull<VMMemoryDefinition> {
        let ptr = unsafe {
            *self.vmctx_plus_offset::<VmPtr<_>>(self.vmshape().vmctx_vmmemory_pointer(index))
        };
        ptr.as_non_null()
    }
    fn global_ptr(&mut self, index: DefinedGlobalIndex) -> NonNull<VMGlobalDefinition> {
        unsafe { self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmglobal_definition(index)) }
    }
    fn tag_ptr(&mut self, index: DefinedTagIndex) -> NonNull<VMTagDefinition> {
        unsafe { self.vmctx_plus_offset_mut(self.vmshape().vmctx_vmtag_definition(index)) }
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
                    (table as *const VMTableDefinition)
                        .offset_from(self.table_ptr(DefinedTableIndex::new(0)).as_ptr()),
                )
                .unwrap(),
            );
            assert!(index.index() < self.tables.len());
            index
        }
    }
}

pub unsafe fn with_instance_and_store<R>(
    vmctx: NonNull<VMContext>,
    f: impl FnOnce(&mut StoreOpaque, &mut Instance) -> R,
) -> R {
    unsafe {
        Instance::from_vmctx(vmctx, |instance| {
            let store = &mut *instance.store.unwrap().as_ptr();
            f(store, instance)
        })
    }
}
