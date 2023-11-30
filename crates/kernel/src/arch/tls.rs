use crate::arch;
use crate::arch::paging::{entry::PageFlags, MAPPER};
use crate::board_info::BoardInfo;
use crate::paging::PhysicalAddress;
use core::arch::asm;
use core::ptr::addr_of;

// Example of TLS usage:
// #[thread_local]
// static mut HART_ID: usize = 1;

extern "C" {
    static __tdata_start: u8;
    static __tbss_end: u8;
}

static mut TLS_BASE: Option<PhysicalAddress> = None;

pub fn setup(board_info: &BoardInfo) -> crate::Result<()> {
    let tls_start = unsafe { addr_of!(__tdata_start) as usize };
    let tls_end = unsafe { addr_of!(__tbss_end) as usize };

    let tls_region = unsafe { PhysicalAddress::new(tls_start)..PhysicalAddress::new(tls_end) };

    if !tls_region.is_empty() {
        let mut mapper = unsafe { MAPPER.lock() };
        let mapper = mapper.as_mut().unwrap();

        let num_pages = (tls_region.end.as_raw() - tls_region.start.as_raw())
            .div_ceil(arch::PAGE_SIZE)
            * board_info.cpus;
        let start = mapper.allocator_mut().allocate_frames(num_pages)?;

        log::debug!(
            "mapping TLS region: {:?}",
            start..start.add(num_pages * arch::PAGE_SIZE)
        );

        for i in 0..num_pages {
            let phys = start.add(i * arch::PAGE_SIZE);
            let flush = mapper.map_identity(phys, PageFlags::READ | PageFlags::WRITE)?;
            unsafe {
                flush.ignore();
            }
        }

        unsafe {
            TLS_BASE = Some(start);
        }
    }

    Ok(())
}

pub fn activate() {
    unsafe {
        let base = TLS_BASE.unwrap();
        asm!("mv tp, {0}", in(reg) base.as_raw());
    }
}
