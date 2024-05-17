use crate::runtime::guest_memory::GuestVec;
use crate::runtime::trap::Trap;
use crate::runtime::vmcontext::{VMFuncRef, VMTableDefinition};
use core::ptr::NonNull;
use core::slice;
use cranelift_wasm::{WasmHeapType, WasmRefType};

pub type FuncTableElem = Option<NonNull<VMFuncRef>>;

pub enum TableElementType {
    Func,
    GcRef,
}

impl From<WasmRefType> for TableElementType {
    fn from(ty: WasmRefType) -> Self {
        match ty.heap_type {
            WasmHeapType::Func | WasmHeapType::ConcreteFunc(_) | WasmHeapType::NoFunc => {
                TableElementType::Func
            }
            WasmHeapType::Extern | WasmHeapType::Any | WasmHeapType::I31 | WasmHeapType::None => {
                TableElementType::GcRef
            }
        }
    }
}

#[derive(Debug)]
pub enum Table {
    Func(FuncTable),
}

impl Table {
    pub fn len(&self) -> usize {
        match self {
            Table::Func(table) => table.elements.len(),
        }
    }
}

#[derive(Debug)]
pub struct FuncTable {
    pub elements: GuestVec<FuncTableElem>,
    pub maximum: Option<u32>,
}

impl Table {
    pub unsafe fn as_vmtable(&self) -> VMTableDefinition {
        match self {
            Table::Func(FuncTable { elements, .. }) => VMTableDefinition {
                base: elements.as_ptr() as *mut _,
                current_length: elements.len().try_into().unwrap(),
            },
        }
    }

    pub fn init_func(
        &mut self,
        dst: u32,
        items: impl ExactSizeIterator<Item = *mut VMFuncRef>,
    ) -> Result<(), Trap> {
        let dst = usize::try_from(dst).map_err(|_| Trap::TableOutOfBounds)?;

        let elements = self
            .funcrefs_mut()
            .get_mut(dst..)
            .and_then(|s| s.get_mut(..items.len()))
            .ok_or(Trap::TableOutOfBounds)?;

        for (item, slot) in items.zip(elements) {
            *slot = item;
        }

        Ok(())
    }

    fn funcrefs_mut(&mut self) -> &mut [*mut VMFuncRef] {
        match self {
            Table::Func(table) => unsafe {
                slice::from_raw_parts_mut(table.elements.as_mut_ptr().cast(), table.elements.len())
            },
        }
    }
}
