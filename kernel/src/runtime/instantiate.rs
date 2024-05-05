use crate::runtime::translate::{MemoryPlan, TablePlan};
use crate::runtime::vmcontext::VMContextOffsets;

pub struct GuestAllocator {}

impl GuestAllocator {
    pub fn allocate_module(&mut self, module: CompiledModule) -> Instance {
        // - for CodeMemory
        // - for VMContext
        // - for Stack
        // - for each table
        //      - allocate space
        // - for each memory
        //      - allocate space
        // result -> Instance

        todo!()
    }

    pub fn allocate_vmctx(&mut self, plan: VMContextOffsets) {
        todo!()
    }

    pub fn allocate_table(&mut self, plan: TablePlan) {
        todo!()
    }

    pub fn deallocate_table(&mut self, plan: TablePlan) {
        todo!()
    }

    pub fn allocate_memory(&mut self, plan: MemoryPlan) {
        todo!()
    }

    pub fn deallocate_memory(&mut self, plan: MemoryPlan) {
        todo!()
    }

    pub fn allocate_stack(&mut self) {
        todo!()
    }

    pub fn deallocate_stack(&mut self) {
        todo!()
    }
}

pub struct Instance {}

impl Instance {
    fn new() -> Self {
        // 3. Initialize tables from const init exprs
        // 4. Initialize memories from const init exprs
        // 5. IF present => run start function

        todo!()
    }

    fn init_vmctx(&mut self) {
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

        // - dont init stack_limit, it's set on function entry
        // - dont init last_wasm_exit_fp, last_wasm_exit_pc, or last_wasm_entry_sp bc zero initialization
    }

    fn init_memory(&mut self) {}

    fn init_table(&mut self) {}
}
