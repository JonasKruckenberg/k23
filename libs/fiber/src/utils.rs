use crate::arch;
use core::mem::ManuallyDrop;
use core::ptr;

/// Internal type for a value that has been encoded in a `usize`.
pub type EncodedValue = usize;

/// Encodes the given value in a `usize` either directly or as a pointer to the
/// argument. This function logically takes ownership of the value, so it should
/// not be dropped afterwards.
pub unsafe fn encode_val<T>(val: &mut ManuallyDrop<T>) -> EncodedValue {
    // Safety: ensured by caller
    unsafe {
        if size_of::<T>() <= size_of::<EncodedValue>() {
            let mut out = 0;
            ptr::write_unaligned(ptr::from_mut(&mut out).cast::<T>(), ManuallyDrop::take(val));
            out
        } else {
            ptr::from_ref(val) as EncodedValue
        }
    }
}

// Decodes a value produced by `encode_usize` either by converting it directly
// or by treating the `usize` as a pointer and dereferencing it.
pub unsafe fn decode_val<T>(val: EncodedValue) -> T {
    // Safety: ensured by caller
    unsafe {
        if size_of::<T>() <= size_of::<EncodedValue>() {
            ptr::read_unaligned(ptr::from_ref(&val).cast::<T>())
        } else {
            ptr::read(val as *const T)
        }
    }
}

/// Helper function to push a value onto a stack.
#[inline]
pub unsafe fn push(sp: &mut usize, val: Option<usize>) {
    // Safety: ensured by caller
    unsafe {
        *sp -= size_of::<usize>();
        if let Some(val) = val {
            *(*sp as *mut usize) = val;
        }
    }
}

/// Helper function to allocate an object on the stack with proper alignment.
///
/// This function is written such that the stack pointer alignment can be
/// constant-folded away when the object doesn't need an alignment greater than
/// `STACK_ALIGNMENT`.
#[inline]
pub unsafe fn allocate_obj_on_stack<T>(sp: &mut usize, sp_offset: usize, obj: T) {
    // Safety: ensured by caller
    unsafe {
        // Sanity check to avoid stack overflows.
        assert!(size_of::<T>() <= 1024, "type is too big to transfer");

        if align_of::<T>() > arch::STACK_ALIGNMENT {
            *sp -= size_of::<T>();
            *sp &= !(align_of::<T>() - 1);
        } else {
            // We know that sp + sp_offset is aligned to STACK_ALIGNMENT. Calculate
            // how much padding we need to add so that sp_offset + padding +
            // sizeof(T) is aligned to STACK_ALIGNMENT.
            let total_size = sp_offset + size_of::<T>();
            let align_offset = total_size % arch::STACK_ALIGNMENT;
            if align_offset != 0 {
                *sp -= arch::STACK_ALIGNMENT - align_offset;
            }
            *sp -= size_of::<T>();
        }
        (*sp as *mut T).write(obj);

        // The stack is aligned to STACK_ALIGNMENT at this point.
        debug_assert_eq!(*sp % arch::STACK_ALIGNMENT, 0);
    }
}
