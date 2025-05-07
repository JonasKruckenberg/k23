// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::PAGE_SIZE;
use crate::wasm::translate::{WasmFuncType, WasmHeapTopType, WasmHeapType, WasmValType};
use cranelift_codegen::ir;
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Signature};
use cranelift_codegen::isa::{CallConv, TargetIsa};

/// Helper macro to generate accessors for an enum.
macro_rules! enum_accessors {
    (@$bind:ident, $variant:ident, $ty:ty, $is:ident, $get:ident, $unwrap:ident, $cvt:expr) => {
        ///  Returns true when the enum is the correct variant.
        pub fn $is(&self) -> bool {
            matches!(self, Self::$variant(_))
        }

        ///  Returns the variant's value, returning None if it is not the correct type.
        #[inline]
        pub fn $get(&self) -> Option<$ty> {
            if let Self::$variant($bind) = self {
                Some($cvt)
            } else {
                None
            }
        }

        /// Returns the variant's value, panicking if it is not the correct type.
        ///
        /// # Panics
        ///
        /// Panics if `self` is not of the right type.
        #[inline]
        pub fn $unwrap(&self) -> $ty {
            self.$get().expect(concat!("expected ", stringify!($ty)))
        }
    };
    ($bind:ident $(($variant:ident($ty:ty) $is:ident $get:ident $unwrap:ident $cvt:expr))*) => ($(enum_accessors!{@$bind, $variant, $ty, $is, $get, $unwrap, $cvt})*)
}

/// Like `enum_accessors!`, but generated methods take ownership of `self`.
macro_rules! owned_enum_accessors {
    ($bind:ident $(($variant:ident($ty:ty) $get:ident $cvt:expr))*) => ($(
        /// Attempt to access the underlying value of this `Val`, returning
        /// `None` if it is not the correct type.
        #[inline]
        pub fn $get(self) -> Option<$ty> {
            if let Self::$variant($bind) = self {
                Some($cvt)
            } else {
                None
            }
        }
    )*)
}

/// Like `offset_of!`, but returns a `u32`.
///
/// # Panics
///
/// Panics if the offset is too large to fit in a `u32`.
macro_rules! u32_offset_of {
    ($ty:ident, $field:ident) => {
        u32::try_from(core::mem::offset_of!($ty, $field)).unwrap()
    };
}

pub(crate) use {enum_accessors, owned_enum_accessors, u32_offset_of};

pub fn value_type(ty: &WasmValType, pointer_type: ir::Type) -> ir::Type {
    match ty {
        WasmValType::I32 => ir::types::I32,
        WasmValType::I64 => ir::types::I64,
        WasmValType::F32 => ir::types::F32,
        WasmValType::F64 => ir::types::F64,
        WasmValType::V128 => ir::types::I8X16,
        WasmValType::Ref(rf) => reference_type(&rf.heap_type, pointer_type),
    }
}

/// Returns the reference type to use for the provided wasm type.
pub fn reference_type(wasm_ht: &WasmHeapType, pointer_type: ir::Type) -> ir::Type {
    match wasm_ht.top().0 {
        WasmHeapTopType::Func => pointer_type,
        WasmHeapTopType::Any | WasmHeapTopType::Extern => ir::types::I32,
        WasmHeapTopType::Exn => todo!(),
        WasmHeapTopType::Cont => todo!(),
    }
}

fn blank_sig(isa: &dyn TargetIsa, call_conv: CallConv) -> Signature {
    let pointer_type = isa.pointer_type();
    let mut sig = Signature::new(call_conv);

    // Add the caller/callee `vmctx` parameters.
    sig.params
        .push(AbiParam::special(pointer_type, ArgumentPurpose::VMContext));
    sig.params.push(AbiParam::new(pointer_type));

    sig
}

pub fn wasm_call_signature(isa: &dyn TargetIsa, func_ty: &WasmFuncType) -> Signature {
    let mut sig = blank_sig(isa, CallConv::Fast);

    let cvt = |ty: &WasmValType| AbiParam::new(value_type(ty, isa.pointer_type()));
    sig.params.extend(func_ty.params.iter().map(&cvt));
    sig.returns.extend(func_ty.results.iter().map(&cvt));

    sig
}

/// Get the Cranelift signature for all array-call functions, that is:
///
/// ```ignore
/// unsafe extern "C" fn(
///     callee_vmctx: *mut VMOpaqueContext,
///     caller_vmctx: *mut VMOpaqueContext,
///     values_ptr: *mut ValRaw,
///     values_len: usize,
/// )
/// ```
///
/// This signature uses the target's default calling convention.
///
/// Note that regardless of the Wasm function type, the array-call calling
/// convention always uses that same signature.
pub fn array_call_signature(isa: &dyn TargetIsa) -> ir::Signature {
    let mut sig = blank_sig(isa, CallConv::triple_default(isa.triple()));
    // The array-call signature has an added parameter for the `values_vec`
    // input/output buffer in addition to the size of the buffer, in units
    // of `ValRaw`.
    sig.params.push(AbiParam::new(isa.pointer_type()));
    sig.params.push(AbiParam::new(isa.pointer_type()));
    // boolean return value of whether this function trapped
    sig.returns.push(ir::AbiParam::new(ir::types::I8));
    sig
}

/// Is `bytes` a multiple of the host page size?
pub fn usize_is_multiple_of_host_page_size(bytes: usize) -> bool {
    bytes % PAGE_SIZE == 0
}

pub fn round_u64_up_to_host_pages(bytes: u64) -> u64 {
    let page_size = u64::try_from(PAGE_SIZE).unwrap();
    debug_assert!(page_size.is_power_of_two());
    let page_size_minus_one = page_size.checked_sub(1).unwrap();
    bytes
        .checked_add(page_size_minus_one)
        .map(|val| val & !page_size_minus_one)
        .unwrap_or_else(|| panic!("{bytes} is too large to be rounded up to a multiple of the host page size of {page_size}"))
}

/// Same as `round_u64_up_to_host_pages` but for `usize`s.
pub fn round_usize_up_to_host_pages(bytes: usize) -> usize {
    let bytes = u64::try_from(bytes).unwrap();
    let rounded = round_u64_up_to_host_pages(bytes);
    usize::try_from(rounded).unwrap()
}

/// Like `mem::size_of` but returns `u8` instead of `usize`
/// # Panics
///
/// Panics if the size of `T` is greater than `u8::MAX`.
pub fn u8_size_of<T: Sized>() -> u8 {
    u8::try_from(size_of::<T>()).expect("type size is too large to be represented as a u8")
}
