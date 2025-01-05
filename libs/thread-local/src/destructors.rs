use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;
use crate::abort;

#[thread_local]
static DTORS: RefCell<Vec<(*mut u8, unsafe extern "C" fn(*mut u8))>> = RefCell::new(Vec::new());

pub(crate) unsafe fn register(t: *mut u8, dtor: unsafe extern "C" fn(*mut u8)) {
    let Ok(mut dtors) = DTORS.try_borrow_mut() else {
        // This point can only be reached if the global allocator calls this
        // function again.
        // FIXME: maybe use the system allocator instead?
        log::error!("the global allocator may not use TLS with destructors");
        abort()
    };
    
    riscv::hio::HostStream::new_stdout()
        .write_fmt(format_args!("registering destructor")).unwrap();
    
    dtors.push((t, dtor));
}

/// The [`guard`] module contains platform-specific functions which will run this
/// function on thread exit if [`guard::enable`] has been called.
///
/// # Safety
///
/// May only be run on thread exit to guarantee that there are no live references
/// to TLS variables while they are destroyed.
pub unsafe fn run() {
    loop {
        let mut dtors = DTORS.borrow_mut();
        match dtors.pop() {
            Some((t, dtor)) => {
                drop(dtors);
                unsafe {
                    dtor(t);
                }
            }
            None => {
                // Free the list memory.
                *dtors = Vec::new();
                break;
            }
        }
    }
}