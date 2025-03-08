// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::util::send_sync_ptr::SendSyncPtr;
use crate::vm::UserBox;
use crate::wasm;
use crate::wasm::func::{for_each_function_signature, FuncData, FuncKind, FuncType, WasmTy};
use crate::wasm::runtime::{ExportedFunction, VMOpaqueContext, VM_HOST_CONTEXT_MAGIC};
use crate::wasm::store::Stored;
use crate::wasm::translate::WasmValType;
use crate::wasm::{runtime, Engine, Func, Store, VMContext, VMFuncRef, VMVal};
use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::any::Any;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{iter, ptr};

#[derive(Debug)]
pub struct HostFunc {
    engine: Engine,
    ctx: Pin<UserBox<HostContext>>,
}

#[derive(Debug)]
pub struct HostContext {
    magic: u32,
    pub(in crate::wasm) func_ref: VMFuncRef,
    store: SendSyncPtr<Store>,
    // the host function pointer, always of type `Fn(Caller, Params) -> Results + Send + Sync + 'static`
    // but type-erased so we don't need to propagate the `Params` and `Results` generics.
    func_func: Box<dyn Any + Send + Sync + 'static>,
}

pub struct Caller<'a> {
    pub(crate) store: &'a mut Store,
    caller: Stored<runtime::Instance>,
}

pub unsafe trait IntoFunc<Params, Results> {
    fn into_func(self, store: &mut Store) -> (Pin<UserBox<HostContext>>, FuncType);
}

pub unsafe trait HostParams {
    unsafe fn load(store: &mut Store, values: &mut [MaybeUninit<VMVal>]) -> Self;
    fn valtypes() -> impl ExactSizeIterator<Item = WasmValType>;
}

pub unsafe trait HostResults {
    unsafe fn store(self, store: &mut Store, ptr: &mut [MaybeUninit<VMVal>]) -> wasm::Result<()>;
    fn valtypes() -> impl ExactSizeIterator<Item = WasmValType>;
}

// === impl HostFunc ===

impl HostFunc {
    pub fn wrap<Params, Results>(
        store: &mut Store,
        func: impl IntoFunc<Params, Results>,
    ) -> (Self, FuncType) {
        let (ctx, ty) = func.into_func(store);
        (
            HostFunc {
                ctx,
                engine: store.engine.clone(),
            },
            ty,
        )
    }

    pub fn into_func(self, store: &mut Store) -> Func {
        Func(store.push_function(FuncData {
            kind: FuncKind::SharedHost(Arc::new(self)),
            ty: None,
        }))
    }

    pub(super) fn exported_func(&self) -> ExportedFunction {
        ExportedFunction {
            func_ref: NonNull::from(self.ctx.func_ref()),
        }
    }
}

// === impl HostContext ===

impl HostContext {
    fn from_closure<F, Params, Results>(
        store: &mut Store,
        func: F,
    ) -> (Pin<UserBox<HostContext>>, FuncType)
    where
        F: Fn(Caller, Params) -> Results + Send + Sync + 'static,
        Params: HostParams,
        Results: HostResults,
    {
        unsafe extern "C" fn array_call_trampoline<F, Params, Results>(
            callee_vmctx: *mut VMOpaqueContext,
            caller_vmctx: *mut VMOpaqueContext,
            args: *mut VMVal,
            args_len: usize,
        ) where
            F: Fn(Caller, Params) -> Results + 'static,
            Params: HostParams,
            Results: HostResults,
        {
            unsafe {
                let vmctx = HostContext::from_opaque(callee_vmctx);

                Caller::with((*vmctx).store.as_ptr(), caller_vmctx, |mut caller| {
                    let args_and_results = core::slice::from_raw_parts_mut(
                        args.cast::<MaybeUninit<VMVal>>(),
                        args_len,
                    );

                    let args = Params::load(caller.store, args_and_results);

                    let func = &(*vmctx).func_func;
                    let func = func.downcast_ref::<F>().unwrap();

                    // TODO catch unwind & trap
                    let results = func(caller.sub_caller(), args);

                    results.store(caller.store, args_and_results).unwrap();
                })
            }
        }

        let ty = FuncType::new(&store.engine, Params::valtypes(), Results::valtypes());
        let type_index = ty.type_index();

        let mut ctx = Self {
            magic: VM_HOST_CONTEXT_MAGIC,
            func_ref: VMFuncRef {
                array_call: array_call_trampoline::<F, Params, Results>,
                wasm_call: None,
                vmctx: ptr::null_mut(), // this will be replaced with our own address
                type_index,
            },
            store: SendSyncPtr::new(NonNull::from(store)),
            func_func: Box::new(func),
        };
        let mut aspace = unsafe { ctx.store.as_mut().alloc.0.lock() };
        let mut ctx = UserBox::new(&mut aspace, ctx, Some("HostContext".to_string())).unwrap();

        let vmctx = VMOpaqueContext::from_hostcontext(ptr::from_mut(ctx.as_mut()));
        ctx.func_ref.vmctx = vmctx;
        (UserBox::into_pin(ctx), ty)
    }

    /// Get this context's `VMFuncRef`.
    #[inline]
    pub fn func_ref(&self) -> &VMFuncRef {
        &self.func_ref
    }

    /// Helper function to cast between context types using a debug assertion to
    /// protect against some mistakes.
    #[inline]
    pub unsafe fn from_opaque(opaque: *mut VMOpaqueContext) -> *mut Self {
        // Safety: responsibility of caller
        unsafe {
            // See comments in `VMContext::from_opaque` for this debug assert
            debug_assert_eq!((*opaque).magic, VM_HOST_CONTEXT_MAGIC);
            opaque.cast()
        }
    }
}

// === impl Caller ===

impl Caller<'_> {
    fn with<F>(store: *mut Store, vmctx: *mut VMOpaqueContext, f: F)
    where
        F: FnOnce(Self),
    {
        unsafe {
            let store = &mut *store;
            let caller = store.get_instance_from_vmctx(VMContext::from_opaque(vmctx));

            f(Self { caller, store })
        }
    }

    fn sub_caller(&mut self) -> Caller<'_> {
        Caller {
            store: self.store,
            caller: self.caller,
        }
    }
}

// === impl IntoFunc ===

macro_rules! impl_into_func {
    ($num:tt $arg:ident) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        unsafe impl<F, $arg, Results> IntoFunc<$arg, Results> for F
        where
            F: Fn($arg) -> Results + Send + Sync + 'static,
            $arg: WasmTy,
            Results: HostResults,
        {
            fn into_func(self, store: &mut Store) -> (Pin<UserBox<HostContext>>, FuncType) {
                HostContext::from_closure(store, move |_: Caller<'_>, ($arg,)| {
                    self($arg)
                })
            }
        }

        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        unsafe impl<F, $arg, Results> IntoFunc<(Caller<'_>, $arg), Results> for F
        where
            F: Fn(Caller, $arg) -> Results + Send + Sync + 'static,
            $arg: WasmTy,
            Results: HostResults,
        {
            fn into_func(self, store: &mut Store) -> (Pin<UserBox<HostContext>>, FuncType) {
                HostContext::from_closure(store, move |caller: Caller<'_>, ($arg,)| {
                    self(caller, $arg)
                })
            }
        }
    };
    ($num:tt $($args:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        unsafe impl<F, $($args,)* Results> IntoFunc<( $($args,)* ), Results> for F
        where
            F: Fn($($args,)*) -> Results + Send + Sync + 'static,
            $($args: WasmTy,)*
            Results: HostResults,
        {
            fn into_func(self, store: &mut Store) -> (Pin<UserBox<HostContext>>, FuncType) {
                HostContext::from_closure(store, move |_: Caller<'_>, ( $( $args ),* )| {
                    self($( $args ),* )
                })
            }
        }

        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        unsafe impl<F, $($args,)* Results> IntoFunc<(Caller<'_>, $($args,)*), Results> for F
        where
            F: Fn(Caller, $($args,)*) -> Results + Send + Sync + 'static,
            $($args: WasmTy,)*
            Results: HostResults,
        {
            fn into_func(self, store: &mut Store) -> (Pin<UserBox<HostContext>>, FuncType) {
                HostContext::from_closure(store, move |caller: Caller<'_>, ( $( $args ),* )| {
                    self(caller, $( $args ),* )
                })
            }
        }
    }
}
for_each_function_signature!(impl_into_func);

// === impl HostParams ===

macro_rules! impl_host_params {
     ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> HostParams for ($($t,)*) {
            unsafe fn load(_store: &mut Store, _values: &mut [MaybeUninit<VMVal>]) -> Self {
                let mut _i = 0;
                ($(unsafe {
                    debug_assert!(_i < _values.len());
                    let ptr = _values.get_unchecked(_i).assume_init_ref();
                    _i += 1;
                    $t::load(_store, ptr)
                },)*)
            }

            fn valtypes() -> impl ExactSizeIterator<Item=WasmValType> {
                IntoIterator::into_iter([$($t::valtype(),)*])
            }
        }
     }
}
for_each_function_signature!(impl_host_params);

// === impl HostResults ===

unsafe impl<T> HostResults for T
where
    T: WasmTy,
{
    unsafe fn store(self, store: &mut Store, ptr: &mut [MaybeUninit<VMVal>]) -> wasm::Result<()> {
        WasmTy::store(self, store, &mut ptr[0])
    }

    fn valtypes() -> impl ExactSizeIterator<Item = WasmValType> {
        iter::once(T::valtype())
    }
}

// unsafe impl<T, E> HostResults for Result<T, E> where T: HostResults {
//     unsafe fn store(self, store: &mut Store, ptr: &mut [MaybeUninit<VMVal>]) -> wasm::Result<()> {
//         todo!()
//     }
// }

macro_rules! impl_host_results {
     ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> HostResults for ($($t,)*) {
            unsafe fn store(self, _store: &mut Store, _ptr: &mut [MaybeUninit<VMVal>]) -> wasm::Result<()> {
                let ($($t,)*) = self;
                let mut _i: usize = 0;
                $(
                    debug_assert!(_i < _ptr.len());
                    // Safety: TODO
                    let val = unsafe { _ptr.get_unchecked_mut(_i) };
                    _i += 1;
                    WasmTy::store($t, _store, val)?;
                )*
                Ok(())
            }

            fn valtypes() -> impl ExactSizeIterator<Item=WasmValType> {
                IntoIterator::into_iter([$($t::valtype(),)*])
            }
        }
     }
}
for_each_function_signature!(impl_host_results);
