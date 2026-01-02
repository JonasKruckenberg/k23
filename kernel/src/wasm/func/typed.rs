// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr;
use core::ptr::NonNull;

use static_assertions::assert_impl_all;

use crate::wasm::func::do_call;
use crate::wasm::store::StoreOpaque;
use crate::wasm::types::{FuncType, HeapType, RefType, ValType};
use crate::wasm::vm::VMVal;
use crate::wasm::{Engine, Func};

pub struct TypedFunc<Params, Results> {
    ty: FuncType,
    func: Func,
    _m: PhantomData<fn(Params) -> Results>,
}
assert_impl_all!(TypedFunc<(), ()>: Send, Sync);

impl<Params, Results> TypedFunc<Params, Results> {
    #[inline]
    pub(super) unsafe fn new_unchecked(func: Func, ty: FuncType) -> Self {
        Self {
            ty,
            func,
            _m: PhantomData,
        }
    }
}

#[derive(Copy, Clone)]
pub enum TypeCheckPosition {
    Param,
    Result,
}

/// A type that can be used as an argument or return value for WASM functions.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmTy: Send {
    /// Return this values type.
    fn valtype() -> ValType;
    /// Store the values lowered representation into the given `slot`.
    fn store(self, store: &mut StoreOpaque, slot: &mut MaybeUninit<VMVal>) -> crate::Result<()>;
    /// Load the value from its lowered representation.
    unsafe fn load(store: &mut StoreOpaque, ptr: &VMVal) -> Self;
    /// Is the value compatible with the given store?
    ///
    /// Numbers (`I32`, `I64`, `F32`, `F64`, and `V128`) are store agnostic and can be used
    /// across stores, while all references are tied to their owning store (they are just pointers
    /// into store-owned memory after all) and *cannot* be used outside their owning store.
    fn compatible_with_store(&self, store: &StoreOpaque) -> bool;

    fn dynamic_concrete_type_check(
        &self,
        store: &StoreOpaque,
        nullable: bool,
        actual: &HeapType,
    ) -> crate::Result<()>;

    /// Assert this value is of the `expected` type.
    #[inline]
    fn typecheck(
        engine: &Engine,
        actual: ValType,
        position: TypeCheckPosition,
    ) -> crate::Result<()> {
        let expected = Self::valtype();

        match position {
            TypeCheckPosition::Result => expected.ensure_matches(engine, &actual),
            TypeCheckPosition::Param => match (expected.as_ref(), actual.as_ref()) {
                (Some(expected_ref), Some(actual_ref)) if actual_ref.heap_type().is_concrete() => {
                    expected_ref
                        .heap_type()
                        .top()
                        .ensure_matches(engine, &actual_ref.heap_type().top())
                }
                _ => expected.ensure_matches(engine, &actual),
            },
        }
    }
}

/// A type that can be used as an argument for WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmParams: Send {
    /// The storage for holding the [`VMVal`] parameters and results
    ///
    /// The storage for holding the array-call parameters and results.
    /// This should most likely be a `[VMVal; N]` array where `N` is the number of parameters.
    type VMValStorage: Copy;

    /// Assert that the provided types are compatible with this type.
    ///
    /// # Errors
    ///
    /// This should return an error IF
    /// - The number of provided types does not *exactly* match the number of expected types.
    /// - The type does not match its expected type.
    fn typecheck(
        engine: &Engine,
        params: impl ExactSizeIterator<Item = ValType>,
    ) -> crate::Result<()>;

    /// Stores this types lowered representation into the provided buffer.
    fn store(
        self,
        store: &mut StoreOpaque,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> crate::Result<()>;
}

/// A type that may be returned from WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// Note: This type piggybacks off of the [`WasmParams`] trait as all types implement both
/// [`WasmParams`] AND [`WasmResults`] we can just reuse [`WasmParams`] typecheck and storage
/// implementations.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmResults: WasmParams {
    /// Loads type from its lowered representation.
    unsafe fn load(store: &mut StoreOpaque, abi: &Self::VMValStorage) -> Self;
}

impl<Params, Results> TypedFunc<Params, Results>
where
    Params: WasmParams,
    Results: WasmResults,
{
    // /// Invokes this functions with the `params`, returning the results asynchronously.
    // ///
    // /// Execution of WebAssembly will happen synchronously in the `poll` method of the
    // /// future returned from this function. Note that, the faster `poll` methods return the more
    // /// responsive the overall system is, so WebAssembly execution interruption should be configured
    // /// such that this futures `poll` method resolves *reasonably* quickly.
    // /// (Reasonably because at the end of the day a task will need to block if it wants to perform
    // /// any meaningful work, there is no way around it).
    // ///
    // /// This function will return `Ok(())` if execution completed without a trap
    // /// or error of any kind. In this situation the results will be written to
    // /// the provided `results` array.
    // ///
    // /// # Errors
    // ///
    // /// Any error which occurs throughout the execution of the function will be
    // /// returned as `Err(e)`.
    // /// Errors typically indicate that execution of WebAssembly was halted
    // /// mid-way and did not complete after the error condition happened.
    // pub fn call(self, store: &mut StoreOpaque, params: Params) -> crate::Result<Results> {
    //     todo!()
    // }

    pub fn into_func(self) -> Func {
        self.func
    }

    pub fn call(&self, store: &mut StoreOpaque, params: Params) -> crate::Result<Results> {
        // Safety: The code below does the lowering and lifting of VM values. The correctness of which is enforced
        // both through this type's generics AND the correct implementation of the array-call trampoline.
        unsafe {
            // Okay, so what is going on here?
            //
            // Because the `TypedFunc` generics we know exactly how much space we need for the
            // array-call parameters and results *at compile time* (declared by the `WasmParams::VMValStorage` or
            // `WasmResults::VMValStorage` associated types). Which neatly allows us to allocate that array on the stack.
            //
            // Remember that the array-call "calling convention" will store the parameters in a consecutive
            // array and use *that same array* to store the results too. This means before jumping to
            // WASM we essentially have a `Params::VMValStorage` on stack, while after returning we
            // have a `Results::VMValStorage` on stack. Note also that (just like when calling through
            // the untyped func) we need to make sure to reserve enough space to fit the larger of the two types.
            //
            // If that sounded suspiciously like a union, congrats! You are correct, a union is exactly
            // what we use.
            //
            // Notice how `Func::call_inner` uses a very different approach: Instead of allocating on
            // stack, it has to use a heap-allocated vec that it needs to resize appropriately.
            union Storage<T: Copy, U: Copy> {
                params: MaybeUninit<T>,
                results: U,
            }

            // Allocate the space on stack
            let mut storage = Storage::<Params::VMValStorage, Results::VMValStorage> {
                params: MaybeUninit::uninit(),
            };

            // Aaand initialize it by lowering the params into it
            params.store(store, &self.ty, &mut storage.params)?;

            // Now, heres a tricky part: We allocated a `Storage::<Params::VMValStorage, Results::VMValStorage>`
            // type on stack above. But what we need for the array call is a `*mut [VMVal]`. We therefore
            // need to essentially "transmute" the union into a slice of `VMVal` which is exactly what
            // we do below.
            {
                let storage_len = size_of_val::<Storage<_, _>>(&storage) / size_of::<VMVal>();
                let storage: *mut Storage<_, _> = &mut storage;
                let storage = storage.cast::<VMVal>();
                let storage = core::slice::from_raw_parts_mut(storage, storage_len);

                let func_ref = self.func.vm_func_ref(store).as_ref();

                // now do the actual WASM calling and trap catching business
                do_call(store, func_ref, storage)?;
            }

            // At this point we have successfully returned from WASM which means that now we have a
            // `Results::VMValStorage` on stack instead of the `Params::VMValStorage` that we wrote into it.
            // Go ahead and lift our Rust values from it and return.
            Ok(Results::load(store, &storage.results))
        }
    }

    #[cfg(debug_assertions)]
    fn debug_typecheck(engine: &Engine, ty: &FuncType) {
        Params::typecheck(engine, ty.params()).expect("params should match");
        Results::typecheck(engine, ty.results()).expect("results should match");
    }
}

// === impl WasmTy ===

macro_rules! impl_wasm_ty_for_ints {
    ($($integer:ident/$get_integer:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the integer methods
        unsafe impl WasmTy for $integer {
            #[inline]
            fn valtype() -> ValType {
                ValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut StoreOpaque, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
                ptr.write(VMVal::$integer(self));
                Ok(())
            }

            #[inline]
            unsafe fn load(_store: &mut StoreOpaque, ptr: &VMVal) -> Self {
                ptr.$get_integer()
            }

            #[inline]
            fn dynamic_concrete_type_check(
                &self,
                _store: &StoreOpaque,
                _nullable: bool,
                _actual: &HeapType,
            ) -> crate::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for integers");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &StoreOpaque) -> bool {
                true
            }
        }
    )*)
}

impl_wasm_ty_for_ints! {
    i32/get_i32 => I32
    i64/get_i64 => I64
    u32/get_u32 => I32
    u64/get_u64 => I64
}

macro_rules! impl_wasm_ty_for_floats {
    ($($float:ident/$get_float:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the float methods
        unsafe impl WasmTy for $float {
            #[inline]
            fn valtype() -> ValType {
                ValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut StoreOpaque, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
                ptr.write(VMVal::$float(self.to_bits()));
                Ok(())
            }

            #[inline]
            unsafe fn load(_store: &mut StoreOpaque, ptr: &VMVal) -> Self {
                $float::from_bits(ptr.$get_float())
            }

            #[inline]
            fn dynamic_concrete_type_check(
                &self,
                _store: &StoreOpaque,
                _nullable: bool,
                _actual: &HeapType,
            ) -> crate::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for floats");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &StoreOpaque) -> bool {
                true
            }
        }
    )*)
}

impl_wasm_ty_for_floats! {
    f32/get_f32 => F32
    f64/get_f64 => F64
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Func {
    fn valtype() -> ValType {
        ValType::Ref(RefType::FUNCREF)
    }

    fn store(self, store: &mut StoreOpaque, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
        let raw = self.vm_func_ref(store).as_ptr();
        ptr.write(VMVal::funcref(raw.cast::<c_void>()));
        Ok(())
    }

    unsafe fn load(store: &mut StoreOpaque, ptr: &VMVal) -> Self {
        // Safety: ensured by caller
        unsafe {
            let ptr = NonNull::new(ptr.get_funcref()).unwrap().cast();
            Func::from_vm_func_ref(store, ptr)
        }
    }

    fn compatible_with_store(&self, store: &StoreOpaque) -> bool {
        store.has_function(self.0)
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &StoreOpaque,
        _nullable: bool,
        _actual: &HeapType,
    ) -> crate::Result<()> {
        todo!()
    }
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Option<Func> {
    fn valtype() -> ValType {
        ValType::Ref(RefType::FUNCREF)
    }

    fn store(self, store: &mut StoreOpaque, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
        let raw = if let Some(f) = self {
            f.vm_func_ref(store).as_ptr()
        } else {
            ptr::null_mut()
        };
        ptr.write(VMVal::funcref(raw.cast::<c_void>()));
        Ok(())
    }

    unsafe fn load(store: &mut StoreOpaque, ptr: &VMVal) -> Self {
        // Safety: ensured by caller
        unsafe {
            let ptr = NonNull::new(ptr.get_funcref())?.cast();
            Some(Func::from_vm_func_ref(store, ptr))
        }
    }

    fn compatible_with_store(&self, store: &StoreOpaque) -> bool {
        if let Some(f) = self {
            store.has_function(f.0)
        } else {
            true
        }
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &StoreOpaque,
        _nullable: bool,
        _actual: &HeapType,
    ) -> crate::Result<()> {
        todo!()
    }
}

// === impl WasmParams | WasmResults ===

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

macro_rules! impl_wasm_params {
    ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> WasmParams for ($($t,)*) {
            type VMValStorage = [VMVal; $n];

            fn typecheck(_engine: &Engine, mut params: impl ExactSizeIterator<Item = ValType>) -> crate::Result<()> {
                let mut _n = 0;

                $(
                    match params.next() {
                        Some(t) => {
                            _n += 1;
                            $t::typecheck(_engine, t, TypeCheckPosition::Param)?
                        },
                        None => {
                            ::anyhow::bail!("expected {} types, found {}", $n as usize, params.len() + _n);
                        },
                    }
                )*

                match params.next() {
                    None => Ok(()),
                    Some(_) => {
                        _n += 1;
                        ::anyhow::bail!("expected {} types, found {}", $n, params.len() + _n);
                    },
                }
            }

            fn store(self, _store: &mut StoreOpaque, _func_ty: &FuncType, _dst: &mut MaybeUninit<Self::VMValStorage>) -> crate::Result<()> {
                #[allow(unused_imports, reason = "macro quirk")]
                use ::k32_util::MaybeUninitExt;

                let ($($t,)*) = self;
                let mut _i: usize = 0;

                $(
                    if !$t.compatible_with_store(_store) {
                        ::anyhow::bail!("attempt to pass cross-`Store` value to Wasm as function argument");
                    }

                    if $t::valtype().is_ref() {
                        let param_ty = _func_ty.param(_i).unwrap();
                        let ref_ty = param_ty.unwrap_ref();
                        if ref_ty.heap_type().is_concrete() {
                            $t.dynamic_concrete_type_check(_store, ref_ty.is_nullable(), ref_ty.heap_type())?;
                        }
                    }

                    // Safety: the macro guarantees that `Self::VMValStorage` has enough space
                    let dst = unsafe { _dst.map(|p| &raw mut (*p)[_i]) };
                    $t.store(_store, dst)?;

                    _i += 1;
                )*
                Ok(())
            }
        }
    }
}

macro_rules! impl_wasm_results {
    ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        #[allow(clippy::unused_unit, reason = "the empty tuple case generates an empty tuple as return value, which makes clippy mad but thats fine")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> WasmResults for ($($t,)*) {
            unsafe fn load(_store: &mut StoreOpaque, _abi: &Self::VMValStorage) -> Self {
                let [$($t,)*] = _abi;
                // Safety: ensured by caller
                ($(unsafe { $t::load(_store, $t) },)*)
            }
        }
    }
}

for_each_function_signature!(impl_wasm_params);
for_each_function_signature!(impl_wasm_results);

// Forwards from a bare type `T` to the 1-tuple type `(T,)`
// Safety: see `impl_wasm_params!`
unsafe impl<T: WasmTy> WasmParams for T {
    type VMValStorage = <(T,) as WasmParams>::VMValStorage;

    fn typecheck(
        engine: &Engine,
        params: impl ExactSizeIterator<Item = ValType>,
    ) -> crate::Result<()> {
        <(T,) as WasmParams>::typecheck(engine, params)
    }

    fn store(
        self,
        store: &mut StoreOpaque,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> crate::Result<()> {
        <(T,) as WasmParams>::store((self,), store, func_ty, dst)
    }
}

// Forwards from a bare type `T` to the 1-tuple type `(T,)`
// Safety: see `impl_wasm_results!`
unsafe impl<T: WasmTy> WasmResults for T {
    unsafe fn load(store: &mut StoreOpaque, abi: &Self::VMValStorage) -> Self {
        // Safety: ensured by caller
        unsafe { <(T,) as WasmResults>::load(store, abi).0 }
    }
}
