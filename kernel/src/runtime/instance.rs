use crate::rt::instantiate::{InstanceHandle, Store};
use crate::rt::module::Module;
use crate::rt::{VMContext, VMFuncRef};
use core::arch::asm;
use core::mem;
use core::ptr::NonNull;
use cranelift_wasm::EntityIndex;

#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub struct Instance(InstanceHandle);

impl Instance {
    pub fn get_func(&self, store: &mut Store, name: &str) -> Option<Func> {
        let data = store.instance_data_mut(self.0);

        let export = data.module_info.module.exports.get(name)?;
        let EntityIndex::Function(func_index) = export else {
            panic!("not a function")
        };
        let export = data.get_exported_func(*func_index);

        Some(Func {
            instance: self.0,
            vm_func_ref: export.func_ref,
        })
    }

    pub(super) unsafe fn new_raw<'wasm>(store: &mut Store<'wasm>, module: Module<'wasm>) -> Self {
        // TODO assert engine sameness

        let handle = store.push_instance(module);
        let data = store.instance_data_mut(handle);

        data.initialize();

        if let Some(func_index) = data.module_info.module.start_func {
            let def_func_index = data
                .module_info
                .module
                .defined_func_index(func_index)
                .unwrap();
            let func_info = &data.module_info.funcs[def_func_index];

            log::debug!("TODO call start function {func_info:?}");
        }

        Self(handle)
    }
}

#[derive(Debug)]
pub struct Func {
    instance: InstanceHandle,
    vm_func_ref: NonNull<VMFuncRef>,
}

impl Func {
    pub fn call(&self, store: &mut Store) {
        let data = store.instance_data_mut(self.instance);

        unsafe {
            let func_ptr = mem::transmute::<_, unsafe extern "C" fn(*mut VMContext, usize)>(
                self.vm_func_ref.as_ptr().read().native_call,
            );

            log::trace!(
                "setting {:p} + {} {:p} to {:p}",
                data.vmctx.as_ptr(),
                data.vmctx_offsets.stack_limit(),
                data.vmctx_plus_offset_mut::<u8>(data.vmctx_offsets.stack_limit()),
                data.stack.stack_limit()
            );
            *data.vmctx_plus_offset_mut(data.vmctx_offsets.stack_limit()) =
                data.stack.stack_limit() as usize;

            log::trace!(
                "before call vmctx {:p} trampoline_ptr {func_ptr:?} stack_limit {:p}",
                data.vmctx.as_ptr(),
                data.stack.stack_limit()
            );
            data.stack.on_stack(data.vmctx.as_ptr(), func_ptr);
            
            log::trace!("finished call");
        };
    }
}
