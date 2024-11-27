// pub mod arch;

use core::fmt;
use core::num::NonZeroUsize;

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl VirtualAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "virtual address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
impl PhysicalAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "physical address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "physical address underflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "physical address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

#[derive(Debug, onlyerror::Error)]
enum AllocError {}

pub trait FrameAllocator {
    fn allocate(
        &mut self,
        frames: NonZeroUsize,
    ) -> Result<(PhysicalAddress, NonZeroUsize), AllocError>;
    fn deallocate(&mut self, addr: PhysicalAddress, frames: NonZeroUsize);
    fn allocate_zeroed(
        &mut self,
        frames: NonZeroUsize,
    ) -> Result<(PhysicalAddress, NonZeroUsize), AllocError>;
}

#[derive(Debug, onlyerror::Error)]
enum Error {}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub trait PhysicalAddressSpace {
    fn map_range(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: Flags,
    ) -> Result<(), Error>;
    fn protect_range(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        flags: Flags,
    ) -> Result<(), Error>;
    fn query(&mut self, virt: VirtualAddress) -> Result<Option<PhysicalAddress>, Error>;
}

mod arch {
    pub const PAGE_SIZE: usize = 0;
    pub const PAGE_TABLE_ENTRIES: usize = 0;
    pub const PAGE_TABLE_LEVELS: usize = 0;
    pub const VA_BITS: usize = 0;
}
