use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use crate::runtime::compile::CompiledFunction;
use crate::runtime::FunctionLoc;
use alloc::vec::Vec;
use core::alloc::{AllocError, Layout};
use core::ptr;
use core::ptr::NonNull;
use vmm::{Mode, VirtualAddress};

#[derive(Debug)]
pub struct Store {
    asid: usize,
    root_table: VirtualAddress,
}

impl Store {
    pub fn clone_from_kernel(asid: usize, kernel_root_table: VirtualAddress) -> Self {
        let root_table = with_frame_alloc(|alloc| alloc.allocate_frame()).unwrap();
        let root_table = kconfig::MEMORY_MODE::phys_to_virt(root_table);

        unsafe {
            ptr::copy_nonoverlapping(
                kernel_root_table.as_raw() as *const u8,
                root_table.as_raw() as *mut u8,
                kconfig::PAGE_SIZE,
            );
        }

        Self { asid, root_table }
    }

    pub fn activate<M: Mode>(&self) {
        unsafe { vmm::Table::<M>::new(self.root_table, M::PAGE_TABLE_LEVELS - 1) }
            .debug_print_table()
            .unwrap();

        M::activate_table(self.asid, self.root_table)
    }
}

unsafe impl alloc::alloc::Allocator for Store {
    fn allocate(&self, _layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        todo!()
    }

    unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {
        todo!()
    }
}
#[derive(Debug)]

pub struct CodeMemory<'store> {
    inner: Vec<u8, &'store Store>,
}

impl<'store> CodeMemory<'store> {
    pub fn with_capacity_in(capacity: usize, store: &'store Store) -> Self {
        Self {
            inner: Vec::with_capacity_in(capacity, store),
        }
    }

    pub fn append_func(&mut self, func: CompiledFunction) -> FunctionLoc {
        let loc = FunctionLoc {
            start: self.inner.len() as u32,
            length: func.buffer.total_size(),
        };

        self.inner.extend_from_slice(func.buffer.data());

        loc
    }
}

pub struct Memory<'store> {
    inner: Vec<u8, &'store Store>,
}

impl<'store> Memory<'store> {
    pub fn with_capacity_in(capacity: usize, store: &'store Store) -> Self {
        Self {
            inner: Vec::with_capacity_in(capacity, store),
        }
    }
}
