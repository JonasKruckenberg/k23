use crate::kconfig;
use crate::runtime::instantiate::stack::Stack;
use crate::runtime::instantiate::{GuestAllocator, InstanceData, InstanceHandle};
use crate::runtime::module::Module;
use cranelift_codegen::entity::PrimaryMap;
use vmm::VirtualAddress;

#[derive(Debug)]
pub struct Store<'wasm> {
    allocator: GuestAllocator,
    instances: PrimaryMap<InstanceHandle, InstanceData<'wasm>>,
}

impl<'wasm> Store<'wasm> {
    pub fn new() -> Self {
        Self {
            allocator: unsafe { GuestAllocator::new_in_kernel_space(VirtualAddress::new(0x1000)) },
            instances: PrimaryMap::new(),
        }
    }

    pub fn allocator(&self) -> GuestAllocator {
        self.allocator.clone()
    }

    pub fn instance_data_mut(&mut self, instance: InstanceHandle) -> &mut InstanceData<'wasm> {
        &mut self.instances[instance]
    }

    pub fn push_instance(&mut self, module: Module<'wasm>) -> InstanceHandle {
        let vmctx = self.allocator.allocate_vmctx(&module.offsets);
        let stack = Stack::new(16 * kconfig::PAGE_SIZE, self.allocator.clone()).unwrap();

        // TODO allocate memories
        // TODO allocate tables

        self.instances.push(InstanceData {
            module_info: module.info,
            code: module.code,
            stack,
            vmctx,
            vmctx_offsets: module.offsets,
        })
    }
}

impl<'wasm> Drop for Store<'wasm> {
    fn drop(&mut self) {
        // go through all instances and deallocate their VMContexts
        for (_, instance) in &self.instances {
            self.allocator
                .deallocate_vmctx(instance.vmctx, &instance.vmctx_offsets)
        }
    }
}
