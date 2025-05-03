use crate::utils::get_unlimited_slice;
use alloc::boxed::Box;
use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::ptr;
use gimli::{
    BaseAddresses, EhFrame, EndianSlice, FrameDescriptionEntry, NativeEndian, UnwindSection,
};
use spin::Mutex;

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    entry: ptr::null_mut(),
});

struct Registry {
    entry: *mut Entry,
}
unsafe impl Send for Registry {}

struct Entry {
    next: *mut Entry,
    tbase: usize,
    dbase: usize,
    fde: *const c_void,
}

#[derive(Debug)]
pub(crate) struct FDESearchResult {
    pub fde: FrameDescriptionEntry<EndianSlice<'static, NativeEndian>>,
    pub bases: BaseAddresses,
    pub eh_frame: EhFrame<EndianSlice<'static, NativeEndian>>,
}

pub(crate) fn find_fde(pc: usize) -> Option<FDESearchResult> {
    unsafe {
        let guard = REGISTRY.lock();
        let mut cur = guard.entry;

        while !cur.is_null() {
            let bases = BaseAddresses::default()
                .set_text((*cur).tbase as _)
                .set_got((*cur).dbase as _);

            let eh_frame = EhFrame::new(get_unlimited_slice((*cur).fde.cast()), NativeEndian);
            let bases = bases.clone().set_eh_frame((*cur).fde.addr() as u64);
            if let Ok(fde) = eh_frame.fde_for_address(&bases, pc as _, EhFrame::cie_from_offset) {
                return Some(FDESearchResult {
                    fde,
                    bases,
                    eh_frame,
                });
            }

            cur = (*cur).next;
        }
    }

    None
}

#[unsafe(no_mangle)]
unsafe extern "C" fn __register_frame(fde: *const c_void) {
    if fde.is_null() {
        return;
    }

    let entry = Box::into_raw(Box::new(MaybeUninit::<Entry>::uninit())) as *mut Entry;
    unsafe {
        entry.write(Entry {
            next: ptr::null_mut(),
            tbase: 0,
            dbase: 0,
            fde,
        });

        let mut guard = REGISTRY.lock();
        (*entry).next = guard.entry;
        guard.entry = entry;
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn __deregister_frame(fde: *const c_void) {
    if fde.is_null() {
        return;
    }

    let mut guard = REGISTRY.lock();

    unsafe {
        let mut prev = &mut guard.entry;
        let mut cur = *prev;

        while !cur.is_null() {
            if (*cur).fde == fde {
                *prev = (*cur).next;

                drop(Box::from_raw(cur as *mut MaybeUninit<Entry>));

                return;
            }
            prev = &mut (*cur).next;
            cur = *prev;
        }
    }
}
