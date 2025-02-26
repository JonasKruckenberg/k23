use crate::arch;
use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::runtime::{StaticVMOffsets, VMContext, VMFunctionImport, VMVal};
use crate::wasm::store::Stored;
use crate::wasm::translate::WasmFuncType;
use crate::wasm::type_registry::RegisteredType;
use crate::wasm::values::Val;
use crate::wasm::{runtime, Store, MAX_WASM_STACK};
use core::arch::asm;
use core::ffi::c_void;
use core::mem;

/// A WebAssembly function.
#[derive(Debug, Clone, Copy)]
pub struct Func(Stored<runtime::ExportedFunction>);

impl Func {
    /// Returns the type of this function.
    ///
    /// # Panics
    ///
    /// TODO
    pub fn ty(self, store: &Store) -> FuncType {
        // Safety: at this point `VMContext` is initialized, so accessing its fields is safe
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        let ty = store
            .engine
            .type_registry()
            .get_type(&store.engine, func_ref.type_index)
            .unwrap();
        FuncType(ty)
    }

    /// Calls the given function with the provided arguments and places the results in the provided
    /// results slice.
    ///
    /// # Errors
    ///
    /// TODO
    ///
    /// # Safety
    ///
    /// It is up to the caller to ensure the provided arguments are of the correct types and that
    /// the `results` slice has enough space to hold the results of the function.
    pub unsafe fn call_unchecked(
        &self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::wasm::Result<()> {
        let ty = self.ty(store);
        let ty = ty.as_wasm_func_type();
        let values_vec_size = params.len().max(ty.results.len());

        // take out the argument storage from the store
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());

        // copy the arguments into the storage
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));
        for (arg, slot) in params.iter().copied().zip(&mut values_vec) {
            *slot = arg.as_vmval(store);
        }

        // do the actual call
        // Safety: caller has to ensure safety
        unsafe {
            self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size)?;
        }

        // copy the results out of the storage
        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = &ty.results[i];
            // Safety: caller has to ensure safety
            *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        }

        // clean up and return the argument storage
        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);

        Ok(())
    }

    #[allow(
        unreachable_code,
        clippy::unnecessary_wraps,
        reason = "TODO rework in progress. see #298"
    )]
    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store,
        args_results_ptr: *mut VMVal,
        args_results_len: usize,
    ) -> crate::wasm::Result<()> {
        // Safety: funcref is always initialized
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        // Safety: funcref is always initialized
        let vmctx = unsafe { VMContext::from_opaque(func_ref.vmctx) };
        let module = store[store.get_instance_from_vmctx(vmctx)].module();

        let _guard = enter_wasm(vmctx, &module.offsets().static_);

        // Safety: caller has to ensure safety
        unsafe {
            riscv::sstatus::set_spp(riscv::sstatus::SPP::User);
            riscv::sepc::set(func_ref.array_call as usize);
            asm! {
                "sret",
                in("a0") vmctx,
                in("a1") vmctx,
                in("a2") args_results_ptr,
                in("a3") args_results_len,
                options(noreturn)
            }
        }

        Ok(())
    }

    pub(crate) fn as_raw(&self, store: &mut Store) -> *mut c_void {
        store[self.0].func_ref.as_ptr().cast()
    }

    pub(crate) fn as_vmfunction_import(&self, store: &Store) -> VMFunctionImport {
        // Safety: at this point `VMContext` is initialized, so accessing its fields is safe
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        VMFunctionImport {
            wasm_call: func_ref.wasm_call,
            array_call: func_ref.array_call,
            vmctx: func_ref.vmctx,
        }
    }

    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedFunction) -> Self {
        Self(store.push_function(export))
    }
}

fn enter_wasm(vmctx: *mut VMContext, offsets: &StaticVMOffsets) -> WasmExecutionGuard {
    let stack_pointer = arch::get_stack_pointer();
    let wasm_stack_limit = stack_pointer.checked_sub(MAX_WASM_STACK).unwrap();

    // Safety: at this point the `VMContext` is initialized and accessing its fields is safe.
    unsafe {
        let stack_limit_ptr = vmctx
            .byte_add(offsets.vmctx_stack_limit() as usize)
            .cast::<usize>();
        let prev_stack = mem::replace(&mut *stack_limit_ptr, wasm_stack_limit);
        WasmExecutionGuard {
            stack_limit_ptr,
            prev_stack,
        }
    }
}

struct WasmExecutionGuard {
    stack_limit_ptr: *mut usize,
    prev_stack: usize,
}

impl Drop for WasmExecutionGuard {
    fn drop(&mut self) {
        // Safety: this relies on `enter_wasm` correctly calculating the stack limit pointer.
        unsafe {
            *self.stack_limit_ptr = self.prev_stack;
        }
    }
}

/// A WebAssembly function type.
///
/// This is essentially a reference counted index into the engine's type registry.
pub struct FuncType(RegisteredType);

impl FuncType {
    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.0.index()
    }

    pub fn as_wasm_func_type(&self) -> &WasmFuncType {
        self.0.unwrap_func()
    }

    pub(crate) fn into_registered_type(self) -> RegisteredType {
        self.0
    }
}
