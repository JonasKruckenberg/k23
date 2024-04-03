use crate::kconfig;
use core::arch::asm;
use core::ops::RangeInclusive;
use vmm::PhysicalAddress;

#[repr(C, align(128))]
pub struct Stack([u8; (Self::GUARD_PAGES + Self::SIZE_PAGES) * kconfig::PAGE_SIZE]);

#[derive(Debug)]
pub struct StackUsage {
    pub used: usize,
    pub total: usize,
    pub high_watermark: usize,
}

impl Stack {
    pub const SIZE_PAGES: usize = kconfig::STACK_SIZE_PAGES;
    pub const GUARD_PAGES: usize = 8;
    pub const FILL_PATTERN: u64 = 0xACE0BACE;

    pub const ZERO: Self = Self([0; (Self::GUARD_PAGES + Self::SIZE_PAGES) * kconfig::PAGE_SIZE]);

    pub fn region(&self) -> RangeInclusive<PhysicalAddress> {
        let start = unsafe {
            PhysicalAddress::new(self.0.as_ptr() as usize)
                .add(Self::GUARD_PAGES * kconfig::PAGE_SIZE)
        };
        start..=start.add(Self::SIZE_PAGES * kconfig::PAGE_SIZE)
    }

    pub fn usage(&self) -> StackUsage {
        let sp: usize;
        unsafe {
            asm!("mv {}, sp", out(reg) sp);
        }
        let sp = unsafe { PhysicalAddress::new(sp) };

        let stack_region = self.region();

        let high_watermark = Self::stack_high_watermark(stack_region.clone());

        if sp < *stack_region.start() {
            panic!("stack overflow");
        }

        StackUsage {
            used: stack_region.end().sub_addr(sp),
            total: Self::SIZE_PAGES * kconfig::PAGE_SIZE,
            high_watermark: stack_region.end().sub_addr(high_watermark),
        }
    }

    fn stack_high_watermark(stack_region: RangeInclusive<PhysicalAddress>) -> PhysicalAddress {
        unsafe {
            let mut ptr = stack_region.start().as_raw() as *const u64;
            let stack_top = stack_region.end().as_raw() as *const u64;

            while ptr < stack_top && *ptr == Self::FILL_PATTERN {
                ptr = ptr.offset(1);
            }

            PhysicalAddress::new(ptr as usize)
        }
    }
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Loader stack is corrupted")
}
