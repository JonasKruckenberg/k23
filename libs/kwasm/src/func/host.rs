// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{iter, ptr};

use anyhow::bail;

use crate::func::typed::WasmTy;
use crate::func::{FuncData, FuncKind};
use crate::store::{StoreInner, StoreOpaque};
use crate::types::{FuncType, ValType};
use crate::vm::{
    InstanceAndStore, VMArrayCallHostFuncContext, VMContext, VMFuncRef, VMOpaqueContext, VMVal,
};
use crate::{Engine, Func};

#[derive(Debug)]
pub struct HostFunc {
    // Stored to unregister this function's signature with the engine when this
    // is dropped.
    engine: Engine,
    ctx: HostContext,
}

impl HostFunc {
    pub fn wrap<T, Params, Results>(
        engine: &Engine,
        func: impl IntoFunc<T, Params, Results>,
    ) -> (Self, FuncType) {
        let (ctx, ty) = func.into_func(engine);
        (
            HostFunc {
                ctx,
                engine: engine.clone(),
            },
            ty,
        )
    }

    pub fn to_func(self: Arc<Self>, store: Pin<&mut StoreOpaque>) -> Func {
        Func(store.add_function(FuncData {
            kind: FuncKind::SharedHost(self),
        }))
    }

    pub(super) fn func_ref(&self) -> NonNull<VMFuncRef> {
        NonNull::from(&self.ctx.0.func_ref)
    }
}

pub struct Caller<'a, T> {
    store: &'a mut StoreInner<T>,
    caller: &'a crate::vm::Instance,
}

impl<T> Caller<'_, T> {
    unsafe fn with<F, R>(caller: NonNull<VMContext>, f: F) -> R
    where
        // The closure must be valid for any `Caller` it is given; it doesn't
        // get to choose the `Caller`'s lifetime.
        F: for<'a> FnOnce(Caller<'a, T>) -> R,
        // And the return value must not borrow from the caller/store.
        R: 'static,
    {
        // Safety: ensured by caller
        unsafe {
            InstanceAndStore::from_vmctx(caller, |pair| {
                let (instance, store) = pair.unpack_with_state_mut::<T>();

                f(Caller {
                    store,
                    caller: instance,
                })
            })
        }
    }

    fn sub_caller(&mut self) -> Caller<'_, T> {
        Caller {
            store: self.store,
            caller: self.caller,
        }
    }
}

#[derive(Debug)]
pub struct HostContext(Box<VMArrayCallHostFuncContext>);

impl HostContext {
    fn from_closure<T, F, Params, Results>(engine: &Engine, f: F) -> (Self, FuncType)
    where
        F: Fn(Caller<'_, T>, Params) -> Results + Send + Sync + 'static,
        Params: HostParams,
        Results: HostResults,
    {
        let ty = FuncType::new(engine, Params::valtypes(), Results::valtypes());

        // Safety: the generics here ensure that the trampoline and `ty` match
        let this = Self(unsafe {
            VMArrayCallHostFuncContext::new(
                Self::array_call_trampoline::<T, F, Params, Results>,
                ty.clone(),
                Box::new(f),
            )
        });
        (this, ty)
    }

    unsafe extern "C" fn array_call_trampoline<T, F, Params, Results>(
        callee_vmctx: NonNull<VMOpaqueContext>,
        caller_vmctx: NonNull<VMOpaqueContext>,
        params_results: NonNull<VMVal>,
        params_len: usize,
    ) -> bool
    where
        F: Fn(Caller<'_, T>, Params) -> Results + 'static,
        Params: HostParams,
        Results: HostResults,
    {
        // Safety: TODO
        unsafe {
            // Note that this function is intentionally scoped into a
            // separate closure. Handling traps and panics will involve
            // longjmp-ing from this function which means we won't run
            // destructors. As a result anything requiring a destructor
            // should be part of this closure, and the long-jmp-ing
            // happens after the closure in handling the result.
            let run = move |mut caller: Caller<'_, T>| {
                let mut params_results = NonNull::slice_from_raw_parts(
                    params_results.cast::<MaybeUninit<VMVal>>(),
                    params_len,
                );
                let vmctx = VMArrayCallHostFuncContext::from_opaque(callee_vmctx);

                let func = vmctx.as_ref().func();

                debug_assert!(func.is::<F>());
                let func = &*ptr::from_ref(func).cast::<F>();

                let params = Params::load(
                    Pin::new_unchecked(&mut caller.store.opaque),
                    params_results.as_mut(),
                );
                let ret = func(caller.sub_caller(), params);

                if !ret.compatible_with_store(&caller.store.opaque) {
                    bail!("host function attempted to return cross-`Store` value to Wasm")
                } else {
                    ret.store(
                        Pin::new_unchecked(&mut caller.store.opaque),
                        params_results.as_mut(),
                    )?;

                    Ok(())
                }
            };

            // crate::wasm::trap_handler::catch_unwind_and_record_trap(|| {
            let vmctx = VMContext::from_opaque(caller_vmctx);
            Caller::with(vmctx, run).is_ok()
            // })
        }
    }
}

pub trait IntoFunc<T, Params, Results>: Send + Sync + 'static {
    fn into_func(self, engine: &Engine) -> (HostContext, FuncType);
}

/// # Safety
///
/// TODO
pub unsafe trait HostParams {
    /// Get the value type that each Type in the list represents.
    fn valtypes() -> impl Iterator<Item = ValType>;
    /// Load a version of `Self` from the `values` provided.
    ///
    /// # Safety
    ///
    /// This function is unsafe as it's up to the caller to ensure that `values` are
    /// valid for this given type.
    unsafe fn load(store: Pin<&mut StoreOpaque>, values: &mut [MaybeUninit<VMVal>]) -> Self;
}

/// # Safety
///
/// TODO
pub unsafe trait HostResults {
    /// Get the value type that each Type in the list represents.
    fn valtypes() -> impl Iterator<Item = ValType>;
    /// Stores this return value into the `ptr` specified using the rooted
    /// `store`.
    ///
    /// Traps are communicated through the `Result<_>` return value.
    ///
    /// # Unsafety
    ///
    /// This method is unsafe as `ptr` must have the correct length to store
    /// this result. This property is only checked in debug mode, not in release
    /// mode.
    unsafe fn store(
        self,
        store: Pin<&mut StoreOpaque>,
        ptr: &mut [MaybeUninit<VMVal>],
    ) -> crate::Result<()>;
    fn compatible_with_store(&self, store: &StoreOpaque) -> bool;
}

macro_rules! for_each_function_signature {
     ($mac:ident) => {
         $mac!(0);
         $mac!(1 A1);
         $mac!(2 A1 A2);
         $mac!(3 A1 A2 A3);
         $mac!(4 A1 A2 A3 A4);
         $mac!(5 A1 A2 A3 A4 A5);
         $mac!(6 A1 A2 A3 A4 A5 A6);
         $mac!(7 A1 A2 A3 A4 A5 A6 A7);
         $mac!(8 A1 A2 A3 A4 A5 A6 A7 A8);
         $mac!(9 A1 A2 A3 A4 A5 A6 A7 A8 A9);
         $mac!(10 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10);
         $mac!(11 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11);
         $mac!(12 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12);
         $mac!(13 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13);
         $mac!(14 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14);
         $mac!(15 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15);
         $mac!(16 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15 A16);
         $mac!(17 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15 A16 A17);
     };
 }

macro_rules! impl_into_func {
    ($num:tt $arg:ident) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        impl<T, F, $arg, Results> IntoFunc<T, $arg, Results> for F
        where
            F: Fn($arg) -> Results + Send + Sync + 'static,
            $arg: WasmTy,
            Results: HostResults,
        {
            fn into_func(self, engine: &Engine) -> (HostContext, FuncType) {
                HostContext::from_closure(engine, move |_: Caller<'_, T>, ($arg,)| {
                    self($arg)
                })
            }
        }

        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        impl<T, F, $arg, Results> IntoFunc<T, (Caller<'_, T>, $arg), Results> for F
        where
            F: Fn(Caller<'_, T>, $arg) -> Results + Send + Sync + 'static,
            $arg: WasmTy,
            Results: HostResults,
        {
            fn into_func(self, engine: &Engine) -> (HostContext, FuncType) {
                HostContext::from_closure(engine, move |caller: Caller<'_, T>, ($arg,)| {
                    self(caller, $arg)
                })
            }
        }
    };
    ($num:tt $($args:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        impl<T, F, $($args,)* Results> IntoFunc<T, ( $($args,)* ), Results> for F
        where
            F: Fn($($args,)*) -> Results + Send + Sync + 'static,
            $($args: WasmTy,)*
            Results: HostResults,
        {
            fn into_func(self, engine: &Engine) -> (HostContext, FuncType) {
                HostContext::from_closure(engine, move |_: Caller<'_, T>, ( $( $args ),* )| {
                    self($( $args ),*)
                })
            }
        }

        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        impl<T, F, $($args,)* Results> IntoFunc<T, (Caller<'_, T>, $($args,)* ), Results> for F
        where
            F: Fn(Caller<'_, T>, $($args,)*) -> Results + Send + Sync + 'static,
            $($args: WasmTy,)*
            Results: HostResults,
        {
            fn into_func(self, engine: &Engine) -> (HostContext, FuncType) {
                HostContext::from_closure(engine, move |caller: Caller<'_, T>, ( $( $args ),* )| {
                    self(caller, $( $args ),*)
                })
            }
        }
    }
}
for_each_function_signature!(impl_into_func);

macro_rules! impl_host_params {
      ($n:tt $($t:ident)*) => {
         #[allow(non_snake_case, reason = "argument names above are uppercase")]
         #[allow(clippy::unused_unit, reason = "macro quirk")]
         // Safety: see `WasmTy` for details
         unsafe impl<$($t: WasmTy,)*> HostParams for ($($t,)*) {
             unsafe fn load(mut _store: ::core::pin::Pin<&mut StoreOpaque>, _values: &mut [MaybeUninit<VMVal>]) -> Self {
                 let mut _i: usize = 0;

                 // Safety: ensured by caller
                 ($(unsafe {
                     debug_assert!(_i < _values.len());
                     let ptr = _values.get_unchecked(_i).assume_init_ref();
                     _i += 1;
                     $t::load(_store.as_mut(), ptr)
                 },)*)
             }

             fn valtypes() -> impl Iterator<Item=ValType> {
                 IntoIterator::into_iter([$($t::valtype(),)*])
             }
         }
      }
 }
for_each_function_signature!(impl_host_params);

// Safety: TODO
unsafe impl<T> HostResults for T
where
    T: WasmTy,
{
    fn valtypes() -> impl Iterator<Item = ValType> {
        iter::once(T::valtype())
    }

    unsafe fn store(
        self,
        store: Pin<&mut StoreOpaque>,
        ptr: &mut [MaybeUninit<VMVal>],
    ) -> crate::Result<()> {
        WasmTy::store(self, store, &mut ptr[0])
    }

    fn compatible_with_store(&self, store: &StoreOpaque) -> bool {
        self.compatible_with_store(store)
    }
}

// Safety: TODO
unsafe impl<T> HostResults for crate::Result<T>
where
    T: HostResults,
{
    fn valtypes() -> impl Iterator<Item = ValType> {
        T::valtypes()
    }

    unsafe fn store(
        self,
        store: Pin<&mut StoreOpaque>,
        ptr: &mut [MaybeUninit<VMVal>],
    ) -> crate::Result<()> {
        // Safety: ensured by caller
        unsafe { self.and_then(|val| val.store(store, ptr)) }
    }

    fn compatible_with_store(&self, store: &StoreOpaque) -> bool {
        match self {
            Ok(x) => <T as HostResults>::compatible_with_store(x, store),
            Err(_) => true,
        }
    }
}

macro_rules! impl_host_results {
      ($n:tt $($t:ident)*) => {
         #[allow(non_snake_case, reason = "argument names above are uppercase")]
         // Safety: see `WasmTy` for details
         unsafe impl<$($t: WasmTy,)*> HostResults for ($($t,)*) {
             fn valtypes() -> impl Iterator<Item=ValType> {
                 IntoIterator::into_iter([$($t::valtype(),)*])
             }

             unsafe fn store(self, mut _store: ::core::pin::Pin<&mut StoreOpaque>, _ptr: &mut [MaybeUninit<VMVal>]) -> crate::Result<()> {
                 let ($($t,)*) = self;
                 let mut _i: usize = 0;
                 $(
                     debug_assert!(_i < _ptr.len());
                     // Safety: TODO
                     let val = unsafe { _ptr.get_unchecked_mut(_i) };
                     _i += 1;
                     WasmTy::store($t, _store.as_mut(), val)?;
                 )*
                 Ok(())
             }

             fn compatible_with_store(&self, _store: &StoreOpaque) -> bool {
                 let ($($t,)*) = self;
                 let compatible = true;
                 $(
                     let compatible = compatible && WasmTy::compatible_with_store($t, _store);
                 )*
                 compatible
            }
         }
      }
 }
for_each_function_signature!(impl_host_results);
