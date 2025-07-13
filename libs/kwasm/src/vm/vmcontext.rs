// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// use crate::builtins::{foreach_builtin_function, BuiltinFunctionIndex};
use crate::builtins::{BuiltinFunctionIndex, foreach_builtin_function};
use crate::indices::VMSharedTypeIndex;
use crate::store::StoreOpaque;
use crate::vm::provenance::{VmPtr, VmSafe};
use crate::wasm::WasmValType;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::fmt;
use core::marker::PhantomPinned;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};
use static_assertions::const_assert_eq;

/// Magic value for core Wasm VM contexts.
///
/// This is stored at the start of all `VMContext` structures.
pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"core");

#[repr(C)]
#[derive(Copy, Clone)]
pub union VMVal {
    /// A WebAssembly `i32` value.
    ///
    /// Note that the payload here is a Rust `i32` but the WebAssembly `i32`
    /// type does not assign an interpretation of the upper bit as either signed
    /// or unsigned. The Rust type `i32` is simply chosen for convenience.
    ///
    /// This value is always stored in a little-endian format.
    i32: i32,

    /// A WebAssembly `i64` value.
    ///
    /// Note that the payload here is a Rust `i64` but the WebAssembly `i64`
    /// type does not assign an interpretation of the upper bit as either signed
    /// or unsigned. The Rust type `i64` is simply chosen for convenience.
    ///
    /// This value is always stored in a little-endian format.
    i64: i64,

    /// A WebAssembly `f32` value.
    ///
    /// Note that the payload here is a Rust `u32`. This is to allow passing any
    /// representation of NaN into WebAssembly without risk of changing NaN
    /// payload bits as its gets passed around the system. Otherwise though this
    /// `u32` value is the return value of `f32::to_bits` in Rust.
    ///
    /// This value is always stored in a little-endian format.
    f32: u32,

    /// A WebAssembly `f64` value.
    ///
    /// Note that the payload here is a Rust `u64`. This is to allow passing any
    /// representation of NaN into WebAssembly without risk of changing NaN
    /// payload bits as its gets passed around the system. Otherwise though this
    /// `u64` value is the return value of `f64::to_bits` in Rust.
    ///
    /// This value is always stored in a little-endian format.
    f64: u64,

    /// A WebAssembly `v128` value.
    ///
    /// The payload here is a Rust `[u8; 16]` which has the same number of bits
    /// but note that `v128` in WebAssembly is often considered a vector type
    /// such as `i32x4` or `f64x2`. This means that the actual interpretation
    /// of the underlying bits is left up to the instructions which consume
    /// this value.
    ///
    /// This value is always stored in a little-endian format.
    #[cfg(feature = "simd")]
    v128: [u8; 16],

    /// A WebAssembly `funcref` value (or one of its subtypes).
    ///
    /// The payload here is a pointer which is runtime-defined. This is one of
    /// the main points of unsafety about the `VMVal` type as the validity of
    /// the pointer here is not easily verified and must be preserved by
    /// carefully calling the correct functions throughout the runtime.
    ///
    /// This value is always stored in a little-endian format.
    funcref: *mut c_void,

    /// A WebAssembly `externref` value (or one of its subtypes).
    ///
    /// The payload here is a compressed pointer value which is
    /// runtime-defined. This is one of the main points of unsafety about the
    /// `VMVal` type as the validity of the pointer here is not easily verified
    /// and must be preserved by carefully calling the correct functions
    /// throughout the runtime.
    ///
    /// This value is always stored in a little-endian format.
    externref: u32,

    /// A WebAssembly `anyref` value (or one of its subtypes).
    ///
    /// The payload here is a compressed pointer value which is
    /// runtime-defined. This is one of the main points of unsafety about the
    /// `VMVal` type as the validity of the pointer here is not easily verified
    /// and must be preserved by carefully calling the correct functions
    /// throughout the runtime.
    ///
    /// This value is always stored in a little-endian format.
    anyref: u32,
}

// Safety: This type is just a bag-of-bits so it's up to the caller to figure out how
// to safely deal with threading concerns and safely access interior bits.
unsafe impl Send for VMVal {}
// Safety: See above
unsafe impl Sync for VMVal {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMFunctionImport {}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMFunctionImport {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTableImport {
    /// A pointer to the imported table description.
    pub from: VmPtr<VMTableDefinition>,

    /// A pointer to the `VMContext` that owns the table description.
    pub vmctx: VmPtr<VMContext>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMTableImport {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMMemoryImport {
    /// A pointer to the imported memory description.
    pub from: VmPtr<VMMemoryDefinition>,

    /// A pointer to the `VMContext` that owns the memory description.
    pub vmctx: VmPtr<VMContext>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMMemoryImport {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMGlobalImport {
    /// A pointer to the imported global variable description.
    pub from: VmPtr<VMGlobalDefinition>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMGlobalImport {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTagImport {
    /// A pointer to the imported tag description.
    pub from: VmPtr<VMTagDefinition>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMTagImport {}

#[derive(Debug)]
#[repr(C)]
pub struct VMTableDefinition {
    /// Pointer to the table data.
    pub base: VmPtr<u8>,

    /// The current number of elements in the table.
    pub current_elements: AtomicUsize,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMTableDefinition {}

#[derive(Debug)]
#[repr(C)]
pub struct VMMemoryDefinition {
    /// The start address.
    pub base: VmPtr<u8>,

    /// The current logical size of this linear memory in bytes.
    ///
    /// This is atomic because shared memories must be able to grow their length
    /// atomically. For relaxed access, see
    /// [`VMMemoryDefinition::current_length()`].
    pub current_length: AtomicUsize,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMMemoryDefinition {}

#[derive(Debug)]
#[repr(C)]
pub struct VMGlobalDefinition {
    storage: VMVal,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMGlobalDefinition {}

#[derive(Debug)]
#[repr(C)]
pub struct VMTagDefinition {
    /// Function signature's type id.
    pub type_index: VMSharedTypeIndex,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMTagDefinition {}

/// A placeholder byte-sized type which is just used to provide some amount of type
/// safety when dealing with pointers to JIT-compiled function bodies. Note that it's
/// deliberately not Copy, as we shouldn't be carelessly copying function body bytes
/// around.
#[derive(Debug)]
#[repr(C)]
pub struct VMFunctionBody(u8);

/// A function pointer that exposes the Wasm calling convention.
#[repr(transparent)]
pub struct VMWasmCallFunction(VMFunctionBody);

pub type VMArrayCallNative =
    unsafe extern "C" fn(*mut VMOpaqueContext, *mut VMOpaqueContext, *mut VMVal, usize) -> bool;

#[derive(Debug)]
#[repr(transparent)]
pub struct VMArrayCallFunction(VMFunctionBody);

#[derive(Debug)]
#[repr(C)]
pub struct VMFuncRef {
    /// Function pointer for this funcref if being called via the "array"
    /// calling convention that `Func::new` et al use.
    pub array_call: VmPtr<VMArrayCallFunction>,

    // /// Function pointer for this funcref if being called via the calling
    // /// convention we use when compiling Wasm.
    // pub wasm_call: Option<VmPtr<VMWasmCallFunction>>,
    /// Function signature's type id.
    pub type_index: VMSharedTypeIndex,

    /// The VM state associated with this function.
    ///
    /// The actual definition of what this pointer points to depends on the
    /// function being referenced: for core Wasm functions, this is a `*mut
    /// VMContext`, for host functions it is a `*mut VMHostFuncContext`, and for
    /// component functions it is a `*mut VMComponentContext`.
    pub vmctx: VmPtr<VMOpaqueContext>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMFuncRef {}

macro_rules! define_builtin_array {
    (
        $(
            $( #[$attr:meta] )*
            $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
        )*
    ) => {
        /// An array that stores addresses of builtin functions. We translate code
        /// to use indirect calls. This way, we don't have to patch the code.
        #[repr(C)]
        pub struct VMBuiltinFunctionsArray {
            $(
                $name: unsafe extern "C" fn(
                    $(define_builtin_array!(@ty $param)),*
                ) $( -> define_builtin_array!(@ty $result))?,
            )*
        }

        impl VMBuiltinFunctionsArray {
            // #[expect(unused_doc_comments, reason = "")]
            pub const INIT: VMBuiltinFunctionsArray = VMBuiltinFunctionsArray {
                $(
                    $name: crate::vm::builtins::raw::$name,
                )*
            };

            /// Helper to call `expose_provenance()` on all contained pointers.
            ///
            /// This is required to be called at least once before entering wasm
            /// to inform the compiler that these function pointers may all be
            /// loaded/stored and used on the "other end" to reacquire
            /// provenance in Pulley. Pulley models hostcalls with a host
            /// pointer as the first parameter that's a function pointer under
            /// the hood, and this call ensures that the use of the function
            /// pointer is considered valid.
            pub fn expose_provenance(&self) -> ::core::ptr::NonNull<Self>{
                $(
                    (self.$name as *mut u8).expose_provenance();
                )*
                ::core::ptr::NonNull::from(self)
            }
        }
    };

    (@ty u32) => (u32);
    (@ty u64) => (u64);
    (@ty u8) => (u8);
    (@ty bool) => (bool);
    (@ty pointer) => (*mut u8);
    (@ty vmctx) => (::core::ptr::NonNull<VMContext>);
}

foreach_builtin_function!(define_builtin_array);
const_assert_eq!(
    size_of::<VMBuiltinFunctionsArray>(),
    size_of::<usize>() * (BuiltinFunctionIndex::builtin_functions_total_number() as usize)
);
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMBuiltinFunctionsArray {}

#[derive(Debug, Default)]
#[repr(C)]
pub struct VMStoreContext {
    /// Indicator of how much fuel has been consumed and is remaining to
    /// WebAssembly.
    ///
    /// This field is typically negative and increments towards positive. Upon
    /// turning positive a wasm trap will be generated. This field is only
    /// modified if wasm is configured to consume fuel.
    pub fuel_consumed: UnsafeCell<i64>,

    /// Deadline epoch for interruption: if epoch-based interruption
    /// is enabled and the global (per engine) epoch counter is
    /// observed to reach or exceed this value, the guest code will
    /// yield if running asynchronously.
    pub epoch_deadline: UnsafeCell<u64>,
}
// SAFETY: the above structure is repr(C) and only contains `VmSafe` fields.
unsafe impl VmSafe for VMStoreContext {}

#[derive(Debug)]
#[repr(C, align(16))] // align 16 since globals are aligned to that and contained inside
pub struct VMContext {
    pub(super) _marker: PhantomPinned,
}

/// An "opaque" version of `VMContext` which must be explicitly casted to a target context.
pub struct VMOpaqueContext {
    magic: u32,
    _marker: PhantomPinned,
}

// === impl VMVal ===

impl VMVal {
    /// Create a null reference that is compatible with any of
    /// `{any,extern,func}ref`.
    pub const fn null() -> VMVal {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe {
            let raw = MaybeUninit::<Self>::zeroed().assume_init();
            debug_assert!(raw.get_anyref() == 0);
            debug_assert!(raw.get_externref() == 0);
            // debug_assert_eq!(raw.get_funcref(), ptr::null_mut());
            raw
        }
    }

    /// Creates a WebAssembly `i32` value
    #[inline]
    #[expect(clippy::cast_sign_loss, reason = "sign-loss is intentional")]
    pub const fn i32(i: i32) -> VMVal {
        VMVal::u64(i as u64)
    }

    /// Creates a WebAssembly `i64` value
    #[inline]
    pub const fn i64(i: i64) -> VMVal {
        VMVal { i64: i.to_le() }
    }
    /// Creates a WebAssembly `i32` value
    #[inline]
    pub const fn u32(i: u32) -> VMVal {
        // See comments in `VMVal::i32` for why this is setting the upper
        // 32-bits as well.
        VMVal::u64(i as u64)
    }

    /// Creates a WebAssembly `i64` value
    #[inline]
    #[expect(clippy::cast_possible_wrap, reason = "wrapping is intentional")]
    pub const fn u64(i: u64) -> VMVal {
        VMVal::i64(i as i64)
    }

    /// Creates a WebAssembly `f32` value
    #[inline]
    pub const fn f32(i: u32) -> VMVal {
        // See comments in `VMVal::i32` for why this is setting the upper
        // 32-bits as well.
        VMVal::u64(i as u64)
    }

    /// Creates a WebAssembly `f64` value
    #[inline]
    pub const fn f64(i: u64) -> VMVal {
        VMVal { f64: i.to_le() }
    }

    // /// Creates a WebAssembly `v128` value
    // #[inline]
    // pub fn v128(i: u128) -> VMVal {
    //     VMVal {
    //         v128: i.to_le_bytes(),
    //     }
    // }

    /// Creates a WebAssembly `funcref` value
    #[inline]
    pub fn funcref(i: *mut c_void) -> VMVal {
        VMVal {
            funcref: i.map_addr(|i| i.to_le()),
        }
    }

    /// Creates a WebAssembly `externref` value
    #[inline]
    pub const fn externref(e: u32) -> VMVal {
        VMVal {
            externref: e.to_le(),
        }
    }

    /// Creates a WebAssembly `anyref` value
    #[inline]
    pub const fn anyref(r: u32) -> VMVal {
        VMVal { anyref: r.to_le() }
    }

    /// Gets the WebAssembly `i32` value
    #[inline]
    pub const fn get_i32(self) -> i32 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe { i32::from_le(self.i32) }
    }

    /// Gets the WebAssembly `i64` value
    #[inline]
    pub const fn get_i64(self) -> i64 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe { i64::from_le(self.i64) }
    }

    /// Gets the WebAssembly `i32` value
    #[inline]
    #[expect(clippy::cast_sign_loss, reason = "sign-loss is intentional")]
    pub const fn get_u32(self) -> u32 {
        self.get_i32() as u32
    }

    /// Gets the WebAssembly `i64` value
    #[inline]
    #[expect(clippy::cast_sign_loss, reason = "sign-loss is intentional")]
    pub const fn get_u64(self) -> u64 {
        self.get_i64() as u64
    }

    /// Gets the WebAssembly `f32` value
    #[inline]
    pub const fn get_f32(self) -> u32 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe { u32::from_le(self.f32) }
    }

    /// Gets the WebAssembly `f64` value
    #[inline]
    pub const fn get_f64(self) -> u64 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe { u64::from_le(self.f64) }
    }

    // /// Gets the WebAssembly `v128` value
    // #[inline]
    // pub const fn get_v128(&self) -> u128 {
    //     // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
    //     unsafe { u128::from_le_bytes(self.v128) }
    // }

    /// Gets the WebAssembly `funcref` value
    #[inline]
    pub fn get_funcref(self) -> *mut c_void {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe { self.funcref.map_addr(usize::from_le) }
    }

    /// Gets the WebAssembly `externref` value
    #[inline]
    pub const fn get_externref(self) -> u32 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        u32::from_le(unsafe { self.externref })
    }

    /// Gets the WebAssembly `anyref` value
    #[inline]
    pub const fn get_anyref(self) -> u32 {
        // Safety: this is just a bag-of-bits, any bit pattern is valid
        u32::from_le(unsafe { self.anyref })
    }
}

impl fmt::Debug for VMVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Hex<T>(T);
        impl<T: fmt::LowerHex> fmt::Debug for Hex<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let bytes = size_of::<T>();
                let hex_digits_per_byte = 2;
                let hex_digits = bytes * hex_digits_per_byte;
                write!(f, "0x{:0width$x}", self.0, width = hex_digits)
            }
        }

        // Safety: this is just a bag-of-bits, any bit pattern is valid (even if nonsensical)
        unsafe {
            let mut f = f.debug_struct("VMVal");
            f.field("i32", &Hex(self.i32))
                .field("i64", &Hex(self.i64))
                .field("f32", &Hex(self.f32))
                .field("f64", &Hex(self.f64))
                .field("funcref", &self.funcref)
                .field("externref", &Hex(self.externref))
                .field("anyref", &Hex(self.anyref));

            #[cfg(feature = "simd")]
            f.field("v128", &Hex(u128::from_le_bytes(self.v128)));

            f.finish()
        }
    }
}

macro_rules! impl_from_vmval_for_int {
    ( $( $int:ty ),* $(,)? ) => {
        $(
            impl From<VMVal> for $int {
                #[allow(clippy::as_underscore, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap, reason = "wrapping is intentional")]
                fn from(untyped: VMVal) -> Self {
                    untyped.get_u64() as _
                }
            }
        )*
    };
}
impl_from_vmval_for_int!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! impl_from_vmval_for_float {
    ( $( $float:ty ),* $(,)? ) => {
        $(
            impl From<VMVal> for $float {
                #[allow(clippy::as_underscore, clippy::cast_possible_truncation, reason = "truncation is intentional")]
                fn from(untyped: VMVal) -> Self {
                    Self::from_bits(untyped.get_u64() as _)
                }
            }
        )*
    };
}
impl_from_vmval_for_float!(f32, f64);

macro_rules! impl_from_unsigned_for_vmval {
    ( $( $prim:ty ),* $(,)? ) => {
        $(
            impl From<$prim> for VMVal {
                #[allow(clippy::cast_lossless, clippy::as_underscore, reason = "TODO")]
                fn from(value: $prim) -> Self {
                    Self::u64(value as _)
                }
            }
        )*
    };
}
#[rustfmt::skip]
impl_from_unsigned_for_vmval!(
    bool, u8, u16, u32, u64,
);

macro_rules! impl_from_signed_for_vmval {
    ( $( $prim:ty as $base:ty ),* $(,)? ) => {
        $(
            impl From<$prim> for VMVal {
                #[allow(clippy::cast_lossless, clippy::cast_sign_loss, reason = "sign-loss is intentional")]
                fn from(value: $prim) -> Self {
                    Self::u64(u64::from(value as $base))
                }
            }
        )*
    };
}
#[rustfmt::skip]
impl_from_signed_for_vmval!(
    i8 as u8,
    i16 as u16,
    i32 as u32,
    i64 as u64,
);

macro_rules! impl_from_float_for_vmval {
    ( $( $float:ty ),* $(,)? ) => {
        $(
            impl From<$float> for VMVal {
                fn from(value: $float) -> Self {
                    Self::u64(u64::from(value.to_bits()))
                }
            }
        )*
    };
}
impl_from_float_for_vmval!(f32, f64);

// === impl VMTableDefinition ===

impl VMTableDefinition {}

// === impl VMMemoryDefinition ===

impl VMMemoryDefinition {
    pub fn current_length(&self, ordering: Ordering) -> usize {
        self.current_length.load(ordering)
    }
}

// === impl VMGlobalDefinition ===

impl VMGlobalDefinition {
    /// Construct a `VMGlobalDefinition`.
    pub const fn null() -> Self {
        Self {
            storage: VMVal::null(),
        }
    }

    /// Create a `VMGlobalDefinition` from a `VMVal`.
    ///
    /// # Unsafety
    ///
    /// This raw value's type must match the given `WasmValType`.
    pub unsafe fn from_vmval(
        _store: &mut StoreOpaque,
        wasm_ty: WasmValType,
        raw: VMVal,
    ) -> crate::Result<Self> {
        todo!()
    }

    /// Get this global's value as a `ValRaw`.
    ///
    /// # Unsafety
    ///
    /// This global's value's type must match the given `WasmValType`.
    pub unsafe fn to_vmval(
        &self,
        _store: &mut StoreOpaque,
        wasm_ty: WasmValType,
    ) -> crate::Result<VMVal> {
        todo!()
    }
}

// === impl VMTagDefinition ===

impl VMTagDefinition {
    pub fn new(type_index: VMSharedTypeIndex) -> Self {
        Self { type_index }
    }
}

// === impl VMFuncRef ===
//
// impl VMFuncRef {
//     pub unsafe fn array_call(
//         &self,
//         vm: InterpreterRef<'_>,
//         caller: *mut VMOpaqueContext,
//         args_and_results: *mut [VMVal],
//     ) -> bool {
//         todo!()
//
//         // match vm {
//         //     Some(vm) => self.array_call_interpreted(vm, caller, args_and_results),
//         //     None => self.array_call_native(caller, args_and_results),
//         // }
//     }
// }
//
