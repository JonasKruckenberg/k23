// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod host;
mod typed;

use crate::arch;
use crate::mem::VirtualAddress;
use crate::util::zip_eq::IteratorExt;
use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::trap_handler::{Trap, TrapReason};
use crate::wasm::types::FuncType;
use crate::wasm::values::Val;
use crate::wasm::vm::{
    ExportedFunction, VMArrayCallHostFuncContext, VMFuncRef, VMFunctionImport, VMOpaqueContext,
    VMVal, VmPtr,
};
use crate::wasm::{Module, Store, MAX_WASM_STACK};
use alloc::boxed::Box;
use alloc::sync::Arc;
use anyhow::ensure;
use core::ffi::c_void;
use core::mem;
use core::ptr::NonNull;
pub use host::{HostFunc, IntoFunc};
pub use typed::{TypedFunc, WasmParams, WasmResults, WasmTy};

#[derive(Clone, Copy, Debug)]
pub struct Func(pub(super) Stored<FuncData>);
#[derive(Debug)]
pub struct FuncData {
    kind: FuncKind,
}
unsafe impl Send for FuncData {}
unsafe impl Sync for FuncData {}

#[derive(Debug)]

enum FuncKind {
    StoreOwned { export: ExportedFunction },
    SharedHost(Arc<HostFunc>),
    Host(Box<HostFunc>),
}

impl Func {
    pub fn wrap<T, Params, Results>(
        store: &mut Store<T>,
        func: impl IntoFunc<T, Params, Results>,
    ) -> TypedFunc<Params, Results>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let (func, ty) = HostFunc::wrap(store.engine(), func);

        let stored = store.add_function(FuncData {
            kind: FuncKind::Host(Box::new(func)),
        });

        unsafe { TypedFunc::new_unchecked(Self(stored), ty) }
    }

    pub fn typed<Params, Results>(
        self,
        store: &StoreOpaque,
    ) -> crate::Result<TypedFunc<Params, Results>>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let ty = self.ty(store);
        Params::typecheck(store.engine(), ty.params())?;
        Results::typecheck(store.engine(), ty.results())?;

        Ok(unsafe { TypedFunc::new_unchecked(self, ty) })
    }

    pub fn call(
        self,
        store: &mut StoreOpaque,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        // Do the typechecking. Notice how `TypedFunc::call` is essentially the same function
        // minus this typechecking? Yeah. That's the benefit of the typed function.
        let ty = self.ty(store);

        let mut params_ = ty.params().zip_eq(params);
        while let Some((expected, param)) = params_.next() {
            let found = param.ty(store)?;
            ensure!(
                expected.matches(&found),
                "Type mismatch. Expected `{expected:?}`, but found `{found:?}`"
            );
        }

        ensure!(
            results.len() >= ty.results().len(),
            "Results slice too small. Need space for at least {}, but got only {}",
            ty.results().len(),
            results.len()
        );

        // Safety: we have checked the types above, we're safe to proceed
        unsafe { self.call_unchecked(store, params, results) }
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
        self,
        store: &mut StoreOpaque,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        // This function mainly performs the lowering and lifting of VMVal values from and to Rust.
        // Because - unlike TypedFunc - we don't have compile-time knowledge about the function type,
        // we use a heap allocated vec (obtained through `store.take_wasm_vmval_storage()`) to store
        // our parameters into and read results from.
        //
        // This is obviously a little less efficient, but it's not that big of a deal.

        // take out the argument storage from the store
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());

        // resize the vec so we can be sure that params and results will fit.
        let values_vec_size = params.len().max(results.len());
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));

        // copy the arguments into the storage vec
        for (arg, slot) in params.iter().zip(&mut values_vec) {
            unsafe {
                *slot = arg.to_vmval(store)?;
            }
        }

        // Safety: func refs obtained from our store are always usable by us.
        let func_ref = unsafe { self.vm_func_ref(store).as_ref() };

        // do the actual call
        // Safety: at this point we have typechecked, we have allocated enough memory for the params
        // and results, and obtained a valid func ref to call.
        unsafe {
            do_call(store, func_ref, &mut values_vec)?;
        }

        // copy the results out of the storage
        let func_ty = self.ty(store);
        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = func_ty.result(i).unwrap();
            // Safety: caller has to ensure safety
            *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        }

        // clean up and return the argument storage
        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);

        Ok(())
    }

    pub fn ty(&self, store: &StoreOpaque) -> FuncType {
        FuncType::from_shared_type_index(store.engine(), self.type_index(store))
    }

    pub fn matches_ty(&self, store: &StoreOpaque, ty: FuncType) -> bool {
        let actual_ty = self.ty(store);
        actual_ty.matches(&ty)
    }

    pub(super) fn type_index(&self, store: &StoreOpaque) -> VMSharedTypeIndex {
        unsafe { self.vm_func_ref(store).as_ref().type_index }
    }

    pub(super) unsafe fn from_exported_function(
        store: &mut StoreOpaque,
        export: ExportedFunction,
    ) -> Self {
        let stored = store.add_function(FuncData {
            kind: FuncKind::StoreOwned { export },
        });
        Self(stored)
    }

    pub(super) fn as_vmfunction_import(
        &self,
        store: &mut StoreOpaque,
        module: &Module,
    ) -> VMFunctionImport {
        let f = self.vm_func_ref(store);

        unsafe {
            VMFunctionImport {
                wasm_call: f.as_ref().wasm_call.unwrap_or_else(|| {
                    // Assert that this is a array-call function, since those
                    // are the only ones that could be missing a `wasm_call`
                    // trampoline.
                    let _ = VMArrayCallHostFuncContext::from_opaque(f.as_ref().vmctx.as_non_null());

                    let sig = self.type_index(store);

                    let ptr = module.wasm_to_array_trampoline(sig).expect(
                        "if the wasm is importing a function of a given type, it must have the \
                         type's trampoline",
                    );

                    VmPtr::from(ptr)
                }),
                array_call: f.as_ref().array_call,
                vmctx: f.as_ref().vmctx,
            }
        }
    }

    pub(super) fn comes_from_same_store(&self, store: &StoreOpaque) -> bool {
        store.has_function(self.0)
    }

    pub(super) unsafe fn from_vm_func_ref(
        store: &mut StoreOpaque,
        func_ref: NonNull<VMFuncRef>,
    ) -> Self {
        unsafe {
            debug_assert!(func_ref.as_ref().type_index != VMSharedTypeIndex::default());
            Func::from_exported_function(store, ExportedFunction { func_ref })
        }
    }

    pub(super) fn vm_func_ref(&self, store: &StoreOpaque) -> NonNull<VMFuncRef> {
        match &store[self.0].kind {
            FuncKind::StoreOwned { export } => export.func_ref,
            FuncKind::SharedHost(func) => NonNull::from(func.func_ref()),
            FuncKind::Host(func) => NonNull::from(func.func_ref()),
        }
    }

    pub(super) unsafe fn from_vmval(store: &mut StoreOpaque, raw: *mut c_void) -> Option<Self> {
        unsafe { Some(Func::from_vm_func_ref(store, NonNull::new(raw.cast())?)) }
    }

    /// Extracts the raw value of this `Func`, which is owned by `store`.
    ///
    /// This function returns a value that's suitable for writing into the
    /// `funcref` field of the [`ValRaw`] structure.
    ///
    /// # Safety
    ///
    /// The returned value is only valid for as long as the store is alive and
    /// this function is properly rooted within it. Additionally this function
    /// should not be liberally used since it's a very low-level knob.
    pub(super) unsafe fn to_vmval(&self, store: &mut StoreOpaque) -> *mut c_void {
        self.vm_func_ref(store).as_ptr().cast()
    }
}

pub(super) unsafe fn do_call(
    store: &mut StoreOpaque,
    func_ref: &VMFuncRef,
    params_and_results: &mut [VMVal],
) -> crate::Result<()> {
    // Safety: TODO
    unsafe {
        let span = tracing::trace_span!("WASM");

        span.in_scope(|| {
            let exit = enter_wasm(store);
            let res = crate::wasm::trap_handler::catch_traps(store, |caller| {
                tracing::trace!("calling VMFuncRef array call");
                let success = func_ref.array_call(VMOpaqueContext::from_vmcontext(caller), NonNull::from(params_and_results));
                tracing::trace!(success, "returned from VMFuncRef array call");
            });
            exit_wasm(store, exit);

            match res {
                Ok(()) => Ok(()),
                Err(Trap { reason, backtrace }) => {
                    let error = match reason {
                        TrapReason::User(err) => err,
                        TrapReason::Jit {
                            pc,
                            faulting_addr,
                            trap,
                        } => {
                            let mut err: anyhow::Error = trap.into();
                            if let Some(fault) =
                                faulting_addr.and_then(|addr| store.wasm_fault(pc, addr))
                            {
                                err = err.context(fault);
                            }
                            err
                        }
                        TrapReason::Wasm(trap_code) => trap_code.into(),
                    };

                    if let Some(bt) = backtrace {
                        tracing::debug!("TODO properly format wasm backtrace {bt:?}");

                        // let bt = WasmBacktrace::from_captured(store, bt, pc);
                        // if !bt.wasm_trace.is_empty() {
                        //     error = error.context(bt);
                        // }
                    }

                    Err(error)
                }
            }
        })
    }
}

/// This function is called to register state within `Store` whenever
/// WebAssembly is entered within the `Store`.
///
/// This function sets up various limits such as:
///
/// * The stack limit. This is what ensures that we limit the stack space
///   allocated by WebAssembly code and it's relative to the initial stack
///   pointer that called into wasm.
///
/// This function may fail if the stack limit can't be set because an
/// interrupt already happened.
fn enter_wasm(store: &mut StoreOpaque) -> Option<VirtualAddress> {
    // If this is a recursive call, e.g. our stack limit is already set, then
    // we may be able to skip this function.
    //
    // // For synchronous stores there's nothing else to do because all wasm calls
    // // happen synchronously and on the same stack. This means that the previous
    // // stack limit will suffice for the next recursive call.
    // //
    // // For asynchronous stores then each call happens on a separate native
    // // stack. This means that the previous stack limit is no longer relevant
    // // because we're on a separate stack.
    // if unsafe { *store.vm_store_context().stack_limit.get() } != VirtualAddress::MAX
    //     && !store.async_support()
    // {
    //     return None;
    // }

    // Ignore this stack pointer business on miri since we can't execute wasm
    // anyway and the concept of a stack pointer on miri is a bit nebulous
    // regardless.
    if cfg!(miri) {
        return None;
    }

    // When Cranelift has support for the host then we might be running native
    // compiled code meaning we need to read the actual stack pointer. If
    // Cranelift can't be used though then we're guaranteed to be running pulley
    // in which case this stack pointer isn't actually used as Pulley has custom
    // mechanisms for stack overflow.
    let stack_pointer = arch::get_stack_pointer();

    // Determine the stack pointer where, after which, any wasm code will
    // immediately trap. This is checked on the entry to all wasm functions.
    //
    // Note that this isn't 100% precise. We are requested to give wasm
    // `max_wasm_stack` bytes, but what we're actually doing is giving wasm
    // probably a little less than `max_wasm_stack` because we're
    // calculating the limit relative to this function's approximate stack
    // pointer. Wasm will be executed on a frame beneath this one (or next
    // to it). In any case it's expected to be at most a few hundred bytes
    // of slop one way or another. When wasm is typically given a MB or so
    // (a million bytes) the slop shouldn't matter too much.
    //
    // After we've got the stack limit then we store it into the `stack_limit`
    // variable.
    let wasm_stack_limit = VirtualAddress::new(stack_pointer - MAX_WASM_STACK).unwrap();
    let prev_stack = unsafe {
        mem::replace(
            &mut *store.vm_store_context().stack_limit.get(),
            wasm_stack_limit,
        )
    };

    Some(prev_stack)
}

fn exit_wasm(store: &mut StoreOpaque, prev_stack: Option<VirtualAddress>) {
    // If we don't have a previous stack pointer to restore, then there's no
    // cleanup we need to perform here.
    let prev_stack = match prev_stack {
        Some(stack) => stack,
        None => return,
    };

    unsafe {
        *store.vm_store_context().stack_limit.get() = prev_stack;
    }
}
