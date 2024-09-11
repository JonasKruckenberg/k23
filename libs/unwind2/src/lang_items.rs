use crate::exception::Exception;
use crate::utils::with_context;
use crate::{arch, raise_exception_phase2, Error};

#[lang = "eh_personality"]
extern "C" fn personality_stub() {}

pub fn ensure_personality_stub(ptr: u64) -> crate::Result<()> {
    if ptr == personality_stub as usize as u64 {
        Ok(())
    } else {
        Err(Error::DifferentPersonality)
    }
}

#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_Resume(exception: *mut Exception) -> ! {
    with_context(|ctx| {
        if let Err(err) = raise_exception_phase2(ctx.clone(), exception) {
            log::error!("Failed to resume exception: {err:?}");
            arch::abort()
        }

        unsafe { arch::restore_context(ctx) }
    })
}
