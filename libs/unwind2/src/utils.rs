use crate::arch;
use core::ptr;
use gimli::{Pointer, Register, RegisterRule, UnwindTableRow};

pub struct StoreOnStack;

// gimli's MSRV doesn't allow const generics, so we need to pick a supported array size.
const fn next_value(x: usize) -> usize {
    let supported = [0, 1, 2, 3, 4, 8, 16, 32, 64, 128];
    let mut i = 0;
    while i < supported.len() {
        if supported[i] >= x {
            return supported[i];
        }
        i += 1;
    }
    192
}

impl<R: gimli::ReaderOffset> gimli::UnwindContextStorage<R> for StoreOnStack {
    type Rules = [(Register, RegisterRule<R>); next_value(arch::MAX_REG_RULES)];
    type Stack = [UnwindTableRow<R, Self>; 2];
}

pub unsafe fn get_unlimited_slice<'a>(start: *const u8) -> &'a [u8] {
    // Create the largest possible slice for this address.
    let start = start as usize;
    let end = start.saturating_add(isize::MAX as _);
    let len = end - start;
    unsafe { core::slice::from_raw_parts(start as *const _, len) }
}

pub unsafe fn deref_pointer(ptr: Pointer) -> u64 {
    match ptr {
        Pointer::Direct(x) => x,
        Pointer::Indirect(x) => unsafe { *(x as *const u64) },
    }
}

// Helper function to turn `save_context` which takes function pointer to a closure-taking function.
pub fn with_context<T, F: FnOnce(&mut arch::Context) -> T>(f: F) -> T {
    use core::mem::ManuallyDrop;

    union Data<T, F> {
        f: ManuallyDrop<F>,
        t: ManuallyDrop<T>,
    }

    extern "C" fn delegate<T, F: FnOnce(&mut arch::Context) -> T>(
        ctx: &mut arch::Context,
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
    arch::save_context(delegate::<T, F>, ptr::addr_of_mut!(data).cast());
    unsafe { ManuallyDrop::into_inner(data.t) }
}
