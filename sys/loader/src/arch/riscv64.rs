// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::naked_asm;

use fdt::Fdt;
use mem_core::arch::riscv64::Riscv64Sv39;
use mem_core::{HardwareAddressSpace, VirtualAddress};
use riscv::satp;
use uefi::proto::unsafe_protocol;
use uefi::{Status, StatusExt, boot};

use crate::error::Error;
use crate::kernel::RelocatedKernel;
use crate::{KernelAspaceLayout, Result};

/// The kernel address space while the loader is still constructing it — i.e.
/// before the MMU is switched on and control is handed to the kernel.
pub type KernelAspace = HardwareAddressSpace<Riscv64Sv39>;

pub fn get_ticks() -> u64 {
    riscv::register::time::read64()
}

/// Read the boot hart's id. Primary source is `RISCV_EFI_BOOT_PROTOCOL`
/// ([spec]); falls back to `/chosen/boot-hartid` on older U-Boot.
///
/// [spec]: https://github.com/riscv-non-isa/riscv-uefi/blob/main/boot_protocol.adoc
pub fn boot_hart_id(fdt: Option<&Fdt<'_>>) -> Result<usize> {
    efi_boot_hartid()
        .or_else(|| fdt.and_then(fdt_boot_hartid))
        .ok_or(Error::NoBootCpuId)
}

fn efi_boot_hartid() -> Option<usize> {
    /// Not in uefi-rs/uefi-raw — defined inline here.
    /// GUID `ccd15fec-6f73-4eec-8395-3e69e4b940bf`.
    #[repr(C)]
    #[unsafe_protocol("ccd15fec-6f73-4eec-8395-3e69e4b940bf")]
    struct RiscvEfiBootProtocol {
        revision: u64,
        get_boot_hartid: unsafe extern "efiapi" fn(this: *mut Self, hartid: *mut usize) -> Status,
    }

    let handle = boot::get_handle_for_protocol::<RiscvEfiBootProtocol>().ok()?;
    let mut proto = boot::open_protocol_exclusive::<RiscvEfiBootProtocol>(handle).ok()?;

    let mut hartid: usize = 0;
    unsafe { (proto.get_boot_hartid)(&mut *proto, &mut hartid) }
        .to_result()
        .ok()?;

    Some(hartid)
}

fn fdt_boot_hartid(fdt: &Fdt<'_>) -> Option<usize> {
    fdt.find_node("/chosen")
        .ok()??
        .find_property("boot-hartid")
        .ok()??
        .as_usize()
        .ok()
}

pub unsafe fn handoff(
    aspace_layout: KernelAspaceLayout,
    kernel: &RelocatedKernel,
    aspace: KernelAspace,
) {
    let (arch, root_page_table) = aspace.into_raw_parts();

    let ppn = root_page_table.get() >> 12_i32;
    let satp = (satp::Mode::Sv39 as usize) << 60 | ((arch.asid() as usize) << 44) | ppn;

    unsafe {
        handoff_trampoline(
            aspace_layout.boot_info.start,
            aspace_layout.boot_hart_stack.end,
            aspace_layout.boot_hart_tls.start,
            kernel.entry(),
            satp,
        )
    }
}

#[unsafe(link_section = ".text.handoff_trampoline")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn handoff_trampoline(
    boot_info: VirtualAddress,
    stack_top: VirtualAddress,
    tls: VirtualAddress,
    entry: VirtualAddress,
    satp_value: usize,
) -> ! {
    naked_asm! {
        // // set SUM
        // "li   t0, (1 << 18)", // Set bit 18 (SUM)
        // "csrrs zero, sstatus, t0",

        // set SATP
        "csrw satp, a4",
        "sfence.vma zero, zero",

        "mv  sp, a1", // Set the kernel stack ptr
        "mv  tp, a2", // Set the kernel thread ptr
        "mv ra, zero", // Reset return address

        "jalr zero, a3",

        "1:",
        "   wfi",
        "   j 1b"
    }
}
