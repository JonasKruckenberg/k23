//! A type that "owns" a specific memory mapped region and exposes methods for managing its
//! permissions (read-only, read-write, executable). Also acts as a RAII guard for the region,
//! unmapping it on drop.

use crate::wasm::utils::usize_is_multiple_of_host_page_size;
use crate::wasm::Error;
use core::ops::Range;
use core::ptr::NonNull;
use core::{ptr, slice};
use rustix::mm::MprotectFlags;

#[derive(Debug)]
pub struct Mmap {
    memory: NonNull<[u8]>,
}

// Safety: TODO why is this safe??
unsafe impl Send for Mmap {}

// Safety: TODO why is this safe??
unsafe impl Sync for Mmap {}

impl Mmap {
    pub fn new_empty() -> Self {
        Self {
            memory: NonNull::from(&mut []),
        }
    }

    pub fn new(size: usize) -> crate::wasm::Result<Self> {
        assert!(usize_is_multiple_of_host_page_size(size));
        // Safety: we pass a nullptr so the kernel will allocate memory for us.
        let ptr = unsafe {
            rustix::mm::mmap_anonymous(
                ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
                rustix::mm::MapFlags::PRIVATE,
            )
            .map_err(|_| Error::MmapFailed)?
        };
        // Safety: the previous call ensures the ptr is valid and u8 doesn't have any alignment/validity requirements.
        let memory = unsafe { slice::from_raw_parts_mut(ptr.cast(), size) };
        let memory = NonNull::new(memory).unwrap();
        Ok(Mmap { memory })
    }

    pub fn with_reserve(size: usize) -> crate::wasm::Result<Self> {
        assert!(usize_is_multiple_of_host_page_size(size));
        assert!(size > 0);
        // Safety: we pass a nullptr so the kernel will allocate memory for us.
        let ptr = unsafe {
            rustix::mm::mmap_anonymous(
                ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::empty(),
                rustix::mm::MapFlags::PRIVATE,
            )
            .map_err(|_| Error::MmapFailed)?
        };

        // Safety: the previous call ensures the ptr is valid and u8 doesn't have any alignment/validity requirements.
        let memory = unsafe { slice::from_raw_parts_mut(ptr.cast(), size) };
        let memory = NonNull::new(memory).unwrap();
        Ok(Mmap { memory })
    }

    #[inline]
    pub unsafe fn slice(&self, range: Range<usize>) -> &[u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        slice::from_raw_parts(self.as_ptr().add(range.start), len)
    }
    pub unsafe fn slice_mut(&mut self, range: Range<usize>) -> &mut [u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        slice::from_raw_parts_mut(self.as_mut_ptr().add(range.start), len)
    }
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.memory.as_ptr() as *const u8
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.memory.as_ptr().cast()
    }

    #[inline]
    pub fn len(&self) -> usize {
        // Safety: the constructor ensures that the NonNull is valid.
        unsafe { (*self.memory.as_ptr()).len() }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn make_accessible(&mut self, start: usize, len: usize) -> crate::wasm::Result<()> {
        let ptr = self.memory.as_ptr();
        assert!(start + len <= self.memory.len());
        // Safety: overflow is checked by the assertions above
        unsafe {
            assert!(
                ptr.byte_add(start).cast::<u8>() <= ptr.byte_add(self.memory.len()).cast::<u8>()
            );

            assert_eq!(
                ptr.byte_add(start).cast::<u8>() as usize % host_page_size(),
                0,
                "changing of protections isn't page-aligned",
            );
        }

        // Safety: provenance invariant is checked by the assertions above
        unsafe {
            rustix::mm::mprotect(
                ptr.byte_add(start).cast(),
                len,
                MprotectFlags::READ | MprotectFlags::WRITE,
            )
            .map_err(|_| Error::MmapFailed)?;
        }

        Ok(())
    }

    pub fn make_executable(
        &self,
        range: Range<usize>,
        enable_branch_protection: bool,
    ) -> crate::wasm::Result<()> {
        assert!(range.start <= self.len());
        assert!(range.end <= self.len());
        assert_eq!(
            range.start % host_page_size(),
            0,
            "changing of protections isn't page-aligned",
        );

        // Safety: overflow is checked by the assertions above
        let base = unsafe { self.memory.as_ptr().byte_add(range.start).cast() };
        let len = range.end.checked_sub(range.start).unwrap();

        let flags = MprotectFlags::READ | MprotectFlags::EXEC;
        let flags = if enable_branch_protection {
            #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
            if std::arch::is_aarch64_feature_detected!("bti") {
                MprotectFlags::from_bits_retain(flags.bits() | /* PROT_BTI */ 0x10)
            } else {
                flags
            }

            #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
            flags
        } else {
            flags
        };

        // Safety: provenance invariant is checked by the assertions above
        unsafe {
            rustix::mm::mprotect(base, len, flags).map_err(|_| Error::MmapFailed)?;
        }

        Ok(())
    }

    pub fn make_readonly(&self, range: Range<usize>) -> crate::wasm::Result<()> {
        assert!(range.start <= self.len());
        assert!(range.end <= self.len());
        assert_eq!(
            range.start % host_page_size(),
            0,
            "changing of protections isn't page-aligned",
        );

        // Safety: overflow is checked by the assertions above
        let base = unsafe { self.memory.as_ptr().byte_add(range.start).cast() };
        let len = range.end.checked_sub(range.start).unwrap();

        // Safety: provenance invariant is checked by the assertions above
        unsafe {
            rustix::mm::mprotect(base, len, MprotectFlags::READ).map_err(|_| Error::MmapFailed)?;
        }

        Ok(())
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // Safety: The rest of the code has to ensure no references to code remain after this.
        unsafe {
            let ptr = self.memory.as_ptr().cast();
            let len = (*self.memory.as_ptr()).len();
            if len == 0 {
                return;
            }
            rustix::mm::munmap(ptr, len).expect("munmap failed");
        }
    }
}
