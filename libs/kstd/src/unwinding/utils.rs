use core::ptr;

use gimli::Pointer;

use crate::arch;

pub unsafe fn get_unlimited_slice<'a>(start: *const u8) -> &'a [u8] {
    // Create the largest possible slice for this address.
    let start = start as usize;
    let end = start.saturating_add(isize::MAX as _);
    let len = end - start;
    unsafe { core::slice::from_raw_parts(start as *const _, len) }
}

pub unsafe fn deref_pointer(ptr: Pointer) -> usize {
    match ptr {
        Pointer::Direct(x) => x as _,
        Pointer::Indirect(x) => unsafe { *(x as *const _) },
    }
}

// Helper function to turn `save_context` which takes function pointer to a closure-taking function.
pub fn with_context<T, F: FnOnce(&mut arch::unwinding::Context) -> T>(f: F) -> T {
    use core::mem::ManuallyDrop;

    union Data<T, F> {
        f: ManuallyDrop<F>,
        t: ManuallyDrop<T>,
    }

    extern "C" fn delegate<T, F: FnOnce(&mut arch::unwinding::Context) -> T>(
        ctx: &mut arch::unwinding::Context,
        ptr: *mut (),
    ) {
        // SAFETY: This function is called exactly once; it extracts the function, call it and
        // store the return value. This function is `extern "C"` so we don't need to worry about
        // unwinding past it.
        unsafe {
            let data = &mut *ptr.cast::<Data<T, F>>();
            let t = ManuallyDrop::take(&mut data.f)(ctx);
            data.t = ManuallyDrop::new(t);
        }
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    arch::unwinding::save_context(delegate::<T, F>, ptr::addr_of_mut!(data).cast());
    unsafe { ManuallyDrop::into_inner(data.t) }
}
