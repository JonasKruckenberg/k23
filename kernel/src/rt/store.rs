use crate::rt::codegen::{MemoryPlan, TablePlan};
use crate::rt::guest_memory::{GuestAllocator, GuestVec};
use crate::rt::instance::{Instance, InstanceData};
use crate::rt::memory::Memory;
use crate::rt::module::Module;
use crate::rt::table::{FuncTable, Table, TableElementType};
use crate::rt::vmcontext::{VMContext, VMContextPlan};
use core::alloc::{Allocator, Layout};
use core::cell::{Ref, RefCell, RefMut};
use core::ptr::NonNull;
use cranelift_entity::PrimaryMap;
use cranelift_wasm::wasmparser::map::HashMap;
use cranelift_wasm::{DefinedMemoryIndex, DefinedTableIndex};
use vmm::VirtualAddress;

pub struct Store<'wasm> {
    allocator: GuestAllocator,
    instances: PrimaryMap<Instance, RefCell<InstanceData<'wasm>>>,
    vmctx2instance: HashMap<NonNull<VMContext>, Instance>,
}

impl<'wasm> Store<'wasm> {
    pub fn new(asid: usize) -> Self {
        Self {
            allocator: unsafe { GuestAllocator::new_in_kernel_space(VirtualAddress::new(0x1000)) },
            instances: PrimaryMap::new(),
            vmctx2instance: HashMap::default(),
        }
    }

    pub fn guest_allocator(&self) -> GuestAllocator {
        self.allocator.clone()
    }

    pub fn instance_data(&self, instance: Instance) -> Ref<'_, InstanceData<'wasm>> {
        self.instances[instance].borrow()
    }

    pub fn instance_data_mut(&self, instance: Instance) -> RefMut<'_, InstanceData<'wasm>> {
        log::debug!("instance_data_mut");
        self.instances[instance].borrow_mut()
    }

    pub fn instance_for_vmctx(&self, vmctx: NonNull<VMContext>) -> Instance {
        self.vmctx2instance[&vmctx]
    }

    pub fn allocate_module(&mut self, module: &Module<'wasm>) -> Instance {
        let vmctx = self.allocate_vmctx(&module.vmctx_plan);
        let tables = self.allocate_tables(module.info.module.defined_tables());
        let memories = self.allocate_memories(module.info.module.defined_memories());

        let handle = self.instances.push(RefCell::new(InstanceData {
            module_info: module.info.clone(),
            code: module.code.clone(),
            vmctx,
            vmctx_plan: module.vmctx_plan.clone(),
            tables,
            memories,
        }));
        self.vmctx2instance.insert(vmctx, handle);

        handle
    }

    fn allocate_vmctx(&mut self, plan: &VMContextPlan) -> NonNull<VMContext> {
        let layout = Layout::from_size_align(plan.size() as usize, 16).unwrap();

        self.allocator.allocate(layout).unwrap().cast()
    }

    fn allocate_tables<'a>(
        &mut self,
        plans: impl ExactSizeIterator<Item = (DefinedTableIndex, &'a TablePlan)> + 'a,
    ) -> PrimaryMap<DefinedTableIndex, Table> {
        let mut tables = PrimaryMap::new();

        for (_, table) in plans {
            tables.push(self.allocate_table(table));
        }

        tables
    }

    fn allocate_table(&mut self, plan: &TablePlan) -> Table {
        let n = usize::try_from(plan.table.minimum).unwrap();

        let mut elements = GuestVec::with_capacity_in(n, self.guest_allocator());
        elements.resize(n, None);

        match TableElementType::from(plan.table.wasm_ty) {
            TableElementType::Func => Table::Func(FuncTable {
                elements,
                maximum: plan.table.maximum,
            }),
            TableElementType::GcRef => todo!(),
        }
    }

    fn allocate_memories<'a>(
        &mut self,
        plans: impl ExactSizeIterator<Item = (DefinedMemoryIndex, &'a MemoryPlan)> + 'a,
    ) -> PrimaryMap<DefinedMemoryIndex, Memory> {
        let mut memories = PrimaryMap::new();

        for (_, memory) in plans {
            memories.push(self.allocate_memory(memory));
        }

        memories
    }

    fn allocate_memory(&mut self, plan: &MemoryPlan) -> Memory {
        let inner =
            GuestVec::with_capacity_in(plan.memory.minimum as usize, self.guest_allocator());

        Memory {
            inner,
            current_length: usize::try_from(plan.memory.minimum).unwrap(),
            maximum: plan.memory.maximum.map(|v| usize::try_from(v).unwrap()),
            asid: 0,
        }
    }
}
