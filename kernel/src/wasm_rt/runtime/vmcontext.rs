#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "too many trivial unsafe blocks"
)]

use crate::wasm_rt::indices::VMSharedTypeIndex;
use core::ffi::c_void;
use core::fmt;
use core::marker::PhantomPinned;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;
use cranelift_entity::Unsigned;
use wasmparser::ValType;

pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"vmcx");

/// The VM "context", which holds guest-side instance state such as
/// globals, table pointers, memory pointers and other runtime information.
///
/// This struct is empty since the size of fields within `VMContext` is dynamic and
/// therefore can't be described by Rust's type system. The exact shape of an instances `VMContext`
/// is described by its `VMContextPlan` which lets you convert entity indices into `VMContext`-relative
/// offset for use in JIT code. For a higher-level access to these fields see the `Instance` methods.
#[derive(Debug)]
#[repr(C, align(16))] // align 16 since globals are aligned to that and contained inside
pub struct VMContext {
    _m: PhantomPinned,
}

impl VMContext {
    /// Helper function to cast between context types using a debug assertion to
    /// protect against some mistakes.
    #[inline]
    pub unsafe fn from_opaque(opaque: *mut VMOpaqueContext) -> *mut VMContext {
        // Note that in general the offset of the "magic" field is stored in
        // `VMOffsets::vmctx_magic`. Given though that this is a sanity check
        // about converting this pointer to another type we ideally don't want
        // to read the offset from potentially corrupt memory. Instead, it would
        // be better to catch errors here as soon as possible.
        //
        // To accomplish this the `VMContext` structure is laid out with the
        // magic field at a statically known offset (here it's 0 for now). This
        // static offset is asserted in `VMOffsets::from` and needs to be kept
        // in sync with this line for this debug assertion to work.
        //
        // Also note that this magic is only ever invalid in the presence of
        // bugs, meaning we don't actually read the magic and act differently
        // at runtime depending what it is, so this is a debug assertion as
        // opposed to a regular assertion.
        debug_assert_eq!(unsafe { (*opaque).magic }, VMCONTEXT_MAGIC);
        opaque.cast()
    }
}

/// An "opaque" version of `VMContext` which must be explicitly casted to a
/// target context.
///
/// This context is used to represent that contexts specified in
/// `VMFuncRef` can have any type and don't have an implicit
/// structure. Neither wasmtime nor cranelift-generated code can rely on the
/// structure of an opaque context in general and only the code which configured
/// the context is able to rely on a particular structure. This is because the
/// context pointer configured for `VMFuncRef` is guaranteed to be
/// the first parameter passed.
///
/// Note that Wasmtime currently has a layout where all contexts that are casted
/// to an opaque context start with a 32-bit "magic" which can be used in debug
/// mode to debug-assert that the casts here are correct and have at least a
/// little protection against incorrect casts.
#[derive(Debug)]
#[repr(C, align(16))]
pub struct VMOpaqueContext {
    pub(crate) magic: u32,
    _marker: PhantomPinned,
}

impl VMOpaqueContext {
    /// Helper function to clearly indicate that casts are desired.
    #[inline]
    pub fn from_vmcontext(ptr: *mut VMContext) -> *mut VMOpaqueContext {
        ptr.cast()
    }
}

#[derive(Clone, Copy)]
pub union VMVal {
    pub i32: i32,
    pub i64: i64,
    pub f32: u32,
    pub f64: u64,
    pub v128: [u8; 16],
    pub funcref: *mut c_void,
    pub externref: u32,
    pub anyref: u32,
}

impl PartialEq for VMVal {
    fn eq(&self, other: &Self) -> bool {
        // Safety: we're accessing a union
        unsafe { self.v128 == other.v128 }
    }
}

impl VMVal {
    #[inline]
    pub fn i32(i: i32) -> VMVal {
        VMVal::i64(i64::from(i))
    }
    #[inline]
    pub fn i64(i: i64) -> VMVal {
        VMVal { i64: i.to_le() }
    }
    #[inline]
    pub fn u32(i: u32) -> VMVal {
        VMVal::u64(u64::from(i))
    }
    #[inline]
    pub fn u64(i: u64) -> VMVal {
        VMVal::i64(i64::try_from(i).unwrap())
    }
    #[inline]
    pub fn f32(i: u32) -> VMVal {
        VMVal { f32: i.to_le() }
    }
    #[inline]
    pub fn f64(i: u64) -> VMVal {
        VMVal { f64: i.to_le() }
    }
    #[inline]
    pub fn v128(i: u128) -> VMVal {
        VMVal {
            v128: i.to_le_bytes(),
        }
    }
    #[inline]
    pub fn funcref(ptr: *mut c_void) -> VMVal {
        VMVal {
            funcref: ptr.map_addr(usize::to_le),
        }
    }
    #[inline]
    pub fn externref(e: u32) -> VMVal {
        assert_eq!(e, 0, "gc not supported");
        VMVal {
            externref: e.to_le(),
        }
    }
    #[inline]
    pub fn anyref(r: u32) -> VMVal {
        assert_eq!(r, 0, "gc not supported");
        VMVal { anyref: r.to_le() }
    }

    #[inline]
    pub fn get_i32(&self) -> i32 {
        // Safety: we're accessing a union
        unsafe { i32::from_le(self.i32) }
    }
    #[inline]
    pub fn get_i64(&self) -> i64 {
        // Safety: we're accessing a union
        unsafe { i64::from_le(self.i64) }
    }
    #[inline]
    pub fn get_u32(&self) -> u32 {
        self.get_i32().unsigned()
    }
    #[inline]
    pub fn get_u64(&self) -> u64 {
        self.get_i64().unsigned()
    }
    #[inline]
    pub fn get_f32(&self) -> u32 {
        // Safety: we're accessing a union
        unsafe { u32::from_le(self.f32) }
    }
    #[inline]
    pub fn get_f64(&self) -> u64 {
        // Safety: we're accessing a union
        unsafe { u64::from_le(self.f64) }
    }
    #[inline]
    pub fn get_v128(&self) -> u128 {
        // Safety: we're accessing a union
        unsafe { u128::from_le_bytes(self.v128) }
    }
    #[inline]
    pub fn get_funcref(&self) -> *mut c_void {
        // Safety: we're accessing a union
        unsafe { self.funcref.map_addr(usize::from_le) }
    }
    #[inline]
    pub fn get_externref(&self) -> u32 {
        // Safety: we're accessing a union
        let externref = u32::from_le(unsafe { self.externref });
        assert_eq!(externref, 0, "gc not supported");
        externref
    }
    #[inline]
    pub fn get_anyref(&self) -> u32 {
        // Safety: we're accessing a union
        let anyref = u32::from_le(unsafe { self.anyref });
        assert_eq!(anyref, 0, "gc not supported");
        anyref
    }
}

// Safety: This type is just a bag-of-bits so it's up to the caller to figure out how
// to safely deal with threading concerns and safely access interior bits.
unsafe impl Send for VMVal {}

// Safety: see above
unsafe impl Sync for VMVal {}

impl fmt::Debug for VMVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Hex<T>(T);
        impl<T: fmt::LowerHex> fmt::Debug for Hex<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let bytes = size_of::<T>();
                let hex_digits_per_byte = 2;
                let hex_digits = bytes.wrapping_mul(hex_digits_per_byte);
                write!(f, "0x{:0width$x}", self.0, width = hex_digits)
            }
        }

        // Safety: we're accessing a union here
        unsafe {
            f.debug_struct("ValRaw")
                .field("i32", &Hex(self.i32))
                .field("i64", &Hex(self.i64))
                .field("f32", &Hex(self.f32))
                .field("f64", &Hex(self.f64))
                .field("v128", &Hex(u128::from_le_bytes(self.v128)))
                .field("funcref", &self.funcref)
                .field("externref", &Hex(self.externref))
                .field("anyref", &Hex(self.anyref))
                .finish()
        }
    }
}

/// A function pointer that exposes the array calling convention.
///
/// Regardless of the underlying Wasm function type, all functions using the
/// array calling convention have the same Rust signature.
///
/// Arguments:
///
/// * Callee `vmctx` for the function itself.
///
/// * Caller's `vmctx` (so that host functions can access the linear memory of
///   their Wasm callers).
///
/// * A pointer to a buffer of `ValRaw`s where both arguments are passed into
///   this function, and where results are returned from this function.
///
/// * The capacity of the `ValRaw` buffer. Must always be at least
///   `max(len(wasm_params), len(wasm_results))`.
pub type VMArrayCallFunction =
    unsafe extern "C" fn(*mut VMContext, *mut VMContext, *mut VMVal, usize);

/// A function pointer that exposes the Wasm calling convention.
///
/// In practice, different Wasm function types end up mapping to different Rust
/// function types, so this isn't simply a type alias the way that
/// `VMArrayCallFunction` is. However, the exact details of the calling
/// convention are left to the Wasm compiler (e.g. Cranelift or Winch). Runtime
/// code never does anything with these function pointers except shuffle them
/// around and pass them back to Wasm.
#[repr(transparent)]
pub struct VMWasmCallFunction(VMFunctionBody);

/// A placeholder byte-sized type which is just used to provide some amount of type
/// safety when dealing with pointers to JIT-compiled function bodies. Note that it's
/// deliberately not Copy, as we shouldn't be carelessly copying function body bytes
/// around.
#[repr(C)]
pub struct VMFunctionBody(u8);

/// The VM caller-checked "funcref" record, for caller-side signature checking.
///
/// It consists of function pointer(s), a type id to be checked by the
/// caller, and the vmctx closure associated with this function.
#[derive(Debug)]
#[repr(C)]
pub struct VMFuncRef {
    /// Function pointer for this funcref if being called via the "array"
    /// calling convention that `Func::new` et al use.
    pub array_call: VMArrayCallFunction,
    /// Function pointer for this funcref if being called via the calling
    /// convention we use when compiling Wasm.
    pub wasm_call: NonNull<VMWasmCallFunction>,
    /// The VM state associated with this function.
    pub vmctx: *mut VMOpaqueContext,
    pub type_index: VMSharedTypeIndex,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMFunctionImport {
    /// Function pointer to use when calling this imported function from Wasm.
    pub wasm_call: NonNull<VMWasmCallFunction>,
    /// Function pointer to use when calling this imported function with the
    /// "array" calling convention that `Func::new` et al use.
    pub array_call: VMArrayCallFunction,
    /// The VM state associated with this function.
    ///
    /// For Wasm functions defined by core wasm instances this will be `*mut
    /// VMContext`, but for lifted/lowered component model functions this will
    /// be a `VMComponentContext`, and for a host function it will be a
    /// `VMHostFuncContext`, etc.
    pub vmctx: *mut VMOpaqueContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTableImport {
    pub from: *mut VMTableDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMMemoryImport {
    pub from: *mut VMMemoryDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMGlobalImport {
    pub from: *mut VMGlobalDefinition,
    pub vmctx: *mut VMContext,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct VMTableDefinition {
    pub base: *mut u8,
    pub current_length: u64,
}

#[derive(Debug)]
#[repr(C)]
pub struct VMMemoryDefinition {
    pub base: *mut u8,
    pub current_length: AtomicUsize,
}

// TODO Pack the globals more densely, rather than using the same size for all of types.
#[derive(Debug)]
#[repr(C, align(16))]
pub struct VMGlobalDefinition {
    data: [u8; 16],
}

#[expect(
    clippy::cast_ptr_alignment,
    reason = "Methods cast from a byte slice to types, this is fine though"
)]
impl VMGlobalDefinition {
    pub unsafe fn from_vmval(vmval: VMVal) -> Self {
        unsafe { Self { data: vmval.v128 } }
    }

    pub unsafe fn to_vmval(&self, wasm_ty: ValType) -> VMVal {
        match wasm_ty {
            ValType::I32 => VMVal {
                i32: unsafe { *self.as_i32() },
            },
            ValType::I64 => VMVal {
                i64: unsafe { *self.as_i64() },
            },
            ValType::F32 => VMVal {
                f32: unsafe { *self.as_f32_bits() },
            },
            ValType::F64 => VMVal {
                f64: unsafe { *self.as_f64_bits() },
            },
            ValType::V128 => VMVal { v128: self.data },
            ValType::Ref(_) => todo!(),
        }
    }

    /// Return a reference to the value as an i32.
    pub unsafe fn as_i32(&self) -> &i32 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<i32>()) }
    }

    /// Return a mutable reference to the value as an i32.
    pub unsafe fn as_i32_mut(&mut self) -> &mut i32 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<i32>()) }
    }

    /// Return a reference to the value as a u32.
    pub unsafe fn as_u32(&self) -> &u32 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<u32>()) }
    }

    /// Return a mutable reference to the value as an u32.
    pub unsafe fn as_u32_mut(&mut self) -> &mut u32 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>()) }
    }

    /// Return a reference to the value as an i64.
    pub unsafe fn as_i64(&self) -> &i64 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<i64>()) }
    }

    /// Return a mutable reference to the value as an i64.
    pub unsafe fn as_i64_mut(&mut self) -> &mut i64 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<i64>()) }
    }

    /// Return a reference to the value as an u64.
    pub unsafe fn as_u64(&self) -> &u64 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<u64>()) }
    }

    /// Return a mutable reference to the value as an u64.
    pub unsafe fn as_u64_mut(&mut self) -> &mut u64 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>()) }
    }

    /// Return a reference to the value as an f32.
    pub unsafe fn as_f32(&self) -> &f32 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<f32>()) }
    }

    /// Return a mutable reference to the value as an f32.
    pub unsafe fn as_f32_mut(&mut self) -> &mut f32 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<f32>()) }
    }

    /// Return a reference to the value as f32 bits.
    pub unsafe fn as_f32_bits(&self) -> &u32 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<u32>()) }
    }

    /// Return a mutable reference to the value as f32 bits.
    pub unsafe fn as_f32_bits_mut(&mut self) -> &mut u32 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>()) }
    }

    /// Return a reference to the value as an f64.
    pub unsafe fn as_f64(&self) -> &f64 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<f64>()) }
    }

    /// Return a mutable reference to the value as an f64.
    pub unsafe fn as_f64_mut(&mut self) -> &mut f64 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<f64>()) }
    }

    /// Return a reference to the value as f64 bits.
    pub unsafe fn as_f64_bits(&self) -> &u64 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<u64>()) }
    }

    /// Return a mutable reference to the value as f64 bits.
    pub unsafe fn as_f64_bits_mut(&mut self) -> &mut u64 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>()) }
    }

    /// Return a reference to the value as an u128.
    pub unsafe fn as_u128(&self) -> &u128 {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<u128>()) }
    }

    /// Return a mutable reference to the value as an u128.
    pub unsafe fn as_u128_mut(&mut self) -> &mut u128 {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<u128>()) }
    }

    /// Return a reference to the value as u128 bits.
    pub unsafe fn as_u128_bits(&self) -> &[u8; 16] {
        unsafe { &*(self.data.as_ref().as_ptr().cast::<[u8; 16]>()) }
    }

    /// Return a mutable reference to the value as u128 bits.
    pub unsafe fn as_u128_bits_mut(&mut self) -> &mut [u8; 16] {
        unsafe { &mut *(self.data.as_mut().as_mut_ptr().cast::<[u8; 16]>()) }
    }
}

#[cfg(test)]
mod test_vmglobal_definition {
    use super::VMGlobalDefinition;

    #[test]
    fn check_vmglobal_definition_alignment() {
        assert!(align_of::<VMGlobalDefinition>() >= align_of::<i32>());
        assert!(align_of::<VMGlobalDefinition>() >= align_of::<i64>());
        assert!(align_of::<VMGlobalDefinition>() >= align_of::<f32>());
        assert!(align_of::<VMGlobalDefinition>() >= align_of::<f64>());
        assert!(align_of::<VMGlobalDefinition>() >= align_of::<[u8; 16]>());
    }
}
