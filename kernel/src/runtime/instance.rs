//! Temporary Instantiation Code
//! 
//! Nothing of this is final 

use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use crate::runtime::compile::{CompiledFunctionInfo, CompiledModule};
use crate::runtime::translate::Module;
use crate::runtime::vmcontext::{VMContextOffsets, VMCONTEXT_MAGIC};
use crate::runtime::Engine;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::alloc::{AllocError, GlobalAlloc, Layout};
use core::arch::asm;
use core::fmt::{Debug, Formatter};
use core::mem;
use core::ops::Range;
use core::ptr::{addr_of, NonNull};
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::TextSectionBuilder;
use cranelift_wasm::DefinedFuncIndex;
use linked_list_allocator::LockedHeap;
use vmm::{AddressRangeExt, EntryFlags, Flush, Mapper, Mode, VirtualAddress};

pub fn test(engine: &Engine, compiled_module: CompiledModule) {
    let store =
        unsafe { Store::new_in_kernel_space(VirtualAddress::new(0x1000), kconfig::PAGE_SIZE * 16) };

    let mut instance = store.allocate_module(engine, compiled_module);
    instance.initialize();

    let funcref = instance
        .get_func_ref(DefinedFuncIndex::from_u32(0))
        .unwrap();

    #[thread_local]
    static mut KERNEL_STACK_PTR: usize = 0;

    log::debug!("entering WASM {funcref:?}");
    unsafe {
        let ret: usize;
        asm!(
            "csrrw sp, sscratch, sp",

            "mv  sp, {wasm_stack_ptr}",
            "mv  a0, {vmctx_ptr}",
            "li  a1, 7",
            "jalr {func}",

            "csrrw sp, sscratch, sp",

            wasm_stack_ptr = in(reg) funcref.stack.as_stack_ptr(),
            vmctx_ptr = in(reg) funcref.vmctx,
            func = in(reg) funcref.ptr,
            out("a0") ret,
        );
        log::trace!("wasm ret {ret}");
    }
    log::debug!("exited WASM");
}

#[derive(Clone)]
pub struct Store(Arc<StoreInner>);

struct StoreInner {
    asid: usize,
    root_table: VirtualAddress,
    virt_offset: VirtualAddress,
    alloc: LockedHeap,

    stack_limit: usize,
}

impl Store {
    pub unsafe fn new_in_kernel_space(virt_offset: VirtualAddress, stack_limit: usize) -> Self {
        let root_table = kconfig::MEMORY_MODE::get_active_table(0);

        let mut inner = StoreInner {
            root_table: kconfig::MEMORY_MODE::phys_to_virt(root_table),
            virt_offset,
            asid: 0,
            alloc: LockedHeap::empty(),

            stack_limit,
        };

        let (mem_virt, flush) = inner.map_additional_pages(32);
        flush.flush().unwrap();

        unsafe {
            inner
                .alloc
                .lock()
                .init(mem_virt.start.as_raw() as *mut u8, mem_virt.size());
        }

        Store(Arc::new(inner))
    }

    pub fn asid(&self) -> usize {
        self.0.asid
    }

    pub fn root_table(&self) -> VirtualAddress {
        self.0.root_table
    }

    pub fn stack_limit(&self) -> usize {
        self.0.stack_limit
    }

    pub fn allocate_module<'wasm>(
        &self,
        engine: &Engine,
        module: CompiledModule<'wasm>,
    ) -> Instance<'wasm> {
        let mut code = CodeMemory::with_capacity_in(module.text.len(), self.clone());
        code.inner.extend(module.text);

        let vmctx_offsets = VMContextOffsets::for_module(engine.target_isa(), &module.module);

        let mut vmctx: Vec<u8, _> =
            Vec::with_capacity_in(vmctx_offsets.size() as usize, self.clone());
        vmctx.resize(vmctx_offsets.size() as usize, 0);

        log::trace!("{vmctx_offsets:?}");

        let stack = Stack::new_in(self.stack_limit(), self.clone());

        // TODO move to initialize
        write_u32_at(vmctx.as_mut(), VMCONTEXT_MAGIC, vmctx_offsets.magic());
        write_usize_at(
            vmctx.as_mut(),
            stack.inner.as_ptr() as usize,
            vmctx_offsets.stack_limit(),
        );

        Instance {
            code,
            vmctx,
            stack,
            module: module.module,
            functions: module.functions,
        }
    }
}

fn write_u32_at(buf: &mut [u8], n: u32, offset: u32) {
    buf[offset as usize..(offset + 4) as usize].copy_from_slice(&n.to_le_bytes());
}

fn write_usize_at(buf: &mut [u8], n: usize, offset: u32) {
    buf[offset as usize..(offset as usize + mem::size_of::<usize>())]
        .copy_from_slice(&n.to_le_bytes());
}

impl StoreInner {
    fn map_additional_pages(
        &mut self,
        num_pages: usize,
    ) -> (Range<VirtualAddress>, Flush<kconfig::MEMORY_MODE>) {
        with_frame_alloc(|alloc| {
            let mut mapper = Mapper::from_address(self.asid, self.root_table, alloc);
            let mut flush = Flush::empty(self.asid);

            let mem_phys = {
                let start = mapper.allocator_mut().allocate_frames(num_pages).unwrap();
                start..start.add(num_pages * kconfig::PAGE_SIZE)
            };

            let mem_virt = self.virt_offset..self.virt_offset.add(num_pages * kconfig::PAGE_SIZE);
            self.virt_offset = mem_virt.end;

            mapper
                .map_range(
                    mem_virt.clone(),
                    mem_phys,
                    EntryFlags::READ | EntryFlags::WRITE,
                    &mut flush,
                )
                .unwrap();

            (mem_virt, flush)
        })
    }
}

unsafe impl alloc::alloc::Allocator for Store {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let ptr = unsafe { self.0.alloc.alloc(layout) };

        if let Some(ptr) = NonNull::new(ptr) {
            Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
        } else {
            // TODO map new pages

            Err(AllocError)
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // TODO unmap pages
        self.0.alloc.dealloc(ptr.cast().as_ptr(), layout)
    }
}

#[derive(Debug)]
struct FuncRef<'a> {
    ptr: *const (),
    vmctx: *const (),
    stack: &'a Stack,
}

pub struct Instance<'wasm> {
    code: CodeMemory,
    vmctx: Vec<u8, Store>,
    stack: Stack,
    module: Module<'wasm>,
    functions: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl<'wasm> Instance<'wasm> {
    pub fn get_func_ref(&self, def_func_index: DefinedFuncIndex) -> Option<FuncRef> {
        let ptr = self.get_func_ptr(def_func_index)?;

        Some(FuncRef {
            ptr,
            vmctx: self.vmctx.as_ptr().cast(),
            stack: &self.stack,
        })
    }

    fn get_func_ptr(&self, def_func_index: DefinedFuncIndex) -> Option<*const ()> {
        let fib_trampoline_loc = self.functions[def_func_index].native_to_wasm_trampoline?;

        Some(unsafe { self.code.as_ptr().add(fib_trampoline_loc.start as usize) }.cast())
    }

    fn initialize(&mut self) {
        self.code.publish();

        //  - set magic value
        //  - init tables (by using VMTableDefinition from Instance)
        //  - init memories (by using )
        //  - init memories
        //      - insert VMMemoryDefinition for every not-shared, not-imported memory
        //      - insert *mut VMMemoryDefinition for every not-shared, not-imported memory
        //      - insert *mut VMMemoryDefinition for every not-imported, shared memory
        //  - init globals from const inits
        //  - TODO funcrefs??
        //  - init imports
        //      - copy from imports.functions
        //      - copy from imports.tables
        //      - copy from imports.memories
        //      - copy from imports.globals
        //  - set stack limit
        //  - dont init last_wasm_exit_fp, last_wasm_exit_pc, or last_wasm_entry_sp bc zero initialization
    }
}

pub struct CodeMemory {
    inner: Vec<u8, Store>,
    flushed: bool,
}

impl CodeMemory {
    pub fn with_capacity_in(capacity: usize, store: Store) -> Self {
        let capacity = (capacity + kconfig::PAGE_SIZE - 1) & !(kconfig::PAGE_SIZE - 1);

        Self {
            inner: Vec::with_capacity_in(capacity, store),
            flushed: false,
        }
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }

    pub fn append_code(&mut self, text_section_builder: &mut dyn TextSectionBuilder) {
        self.inner
            .extend(text_section_builder.finish(&mut Default::default()));
    }

    pub fn publish(&mut self) {
        self.make_executable().unwrap();
    }

    fn make_executable(&mut self) -> Result<(), vmm::Error> {
        debug_assert!(
            !self.flushed,
            "code memory has already been made executable"
        );

        if self.inner.is_empty() {
            return Ok(());
        }

        let store = self.inner.allocator();

        with_frame_alloc(|alloc| -> Result<(), vmm::Error> {
            let mut mapper: Mapper<kconfig::MEMORY_MODE> =
                Mapper::from_address(store.asid(), store.root_table(), alloc);
            let mut flush = Flush::empty(store.asid());

            let range = self.inner.as_ptr_range();
            let range = unsafe {
                VirtualAddress::new(range.start as usize)..VirtualAddress::new(range.end as usize)
            };
            let range = range.align(kconfig::PAGE_SIZE);

            log::trace!("Making {range:?} executable");

            mapper.set_flags_for_range(
                range,
                EntryFlags::READ | EntryFlags::EXECUTE,
                &mut flush,
            )?;

            flush.flush()?;
            self.flushed = true;

            Ok(())
        })
    }
}

pub struct Stack {
    inner: Vec<u8, Store>,
}

impl Stack {
    pub fn new_in(size: usize, store: Store) -> Self {
        let mut inner: Vec<u8, _> = Vec::with_capacity_in(size, store);
        inner.resize(size, 0); // TODO fill with stack pattern 0xACE0BACE

        Self { inner }
    }

    pub(crate) fn as_stack_ptr(&self) -> *const u8 {
        unsafe { self.inner.as_ptr().add(self.inner.len()) }
    }
}

impl Debug for Stack {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Stack")
            .field(&format_args!(
                "{:?}, size: {}, capacity: {}",
                self.inner.as_ptr_range(),
                self.inner.len(),
                self.inner.capacity()
            ))
            .finish()
    }
}
