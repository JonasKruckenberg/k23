// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::PAGE_SIZE;
use crate::device_tree::{Device, DeviceTree, IrqSource};
use crate::irq::{InterruptController, IrqClaim};
use crate::vm::{
    with_kernel_aspace, AddressRangeExt, AddressSpaceRegion, Permissions, PhysicalAddress,
};
use alloc::string::ToString;
use core::alloc::Layout;
use core::mem::{offset_of, MaybeUninit};
use core::num::NonZero;
use core::ops::{BitAnd, BitOr, Not};
use core::ptr;
use core::range::Range;
use fallible_iterator::FallibleIterator;
use static_assertions::const_assert_eq;

const MAX_CONTEXTS: usize = 64;

#[derive(Debug)]
pub struct Plic {
    /// The MMIO registers of the PLIC.
    regs: *mut PlicRegs,
    context: usize,
    /// The number of external interrupts supported by this controller.
    ///
    /// <https://github.com/torvalds/linux/blob/5bfc75d92efd494db37f5c4c173d3639d4772966/Documentation/devicetree/bindings/interrupt-controller/sifive%2Cplic-1.0.0.yaml#L69>
    ndev: usize,
}

#[repr(packed(4))]
#[repr(C)]
struct PlicRegs {
    source_priority: [MmioReg<u32>; 1024], // 0x0000000 -- 0x0000fff
    /// A 32x32 array of 32-bit registers, each representing a bitfield of 1024 pending interrupt bits.
    /// Bit 0 of word 0 is hardwired to zero.
    ///
    /// A pending bit in the PLIC core can be cleared by setting the associated enable bit then performing a claim.
    pending: [MmioReg<u32>; 32], // 0x0001000 -- 0x000107f
    _padding1: [u8; 3968],
    /// A 32x32 array of 32-bit registers, each representing a bitfield of 1024 interrupt enable bits.
    /// for a context. PLIC has 15872 enable contexts.
    enable: [[MmioReg<u32>; 32]; 15872], // 0x0002000 -- 0x01f1fff
    _padding2: [u8; 57344],
    /// Interrupt priority threshold and claim/complete register for each context.
    ///
    /// The memory layout is as follows:
    /// - context block 0
    ///     - 4-byte threshold register         0x200000
    ///     - 4-byte claim/complete register    0x200004
    /// - context block 1
    ///     - 4-byte threshold register         0x201000
    ///     - 4-byte claim/complete register    0x201004
    ///
    /// where each context block is aligned to a 4096-byte boundary.
    thresholds_claims: [ThresholdsClaimsRegs; 15872], // 0x0200000 -- 0x3fff000
}

const_assert_eq!(offset_of!(PlicRegs, source_priority), 0x000000);
const_assert_eq!(offset_of!(PlicRegs, pending), 0x001000);
const_assert_eq!(offset_of!(PlicRegs, enable), 0x002000);
const_assert_eq!(offset_of!(PlicRegs, thresholds_claims), 0x0200000);

#[repr(packed(4))]
#[repr(C)]
struct ThresholdsClaimsRegs {
    threshold: MmioReg<u32>,
    claim_complete: MmioReg<u32>,
    _padding: [u8; 4088],
}

impl Plic {
    #[cold]
    pub fn new(devtree: &DeviceTree, hlic_node: &Device) -> crate::Result<Plic> {
        let soc = devtree.find_by_path("/soc").expect("missing /soc node");

        let (context, dev) = soc
            .children()
            .filter(|dev| dev.is_compatible(["sifive,plic-1.0.0", "riscv,plic0"]))
            .find_map(|dev| {
                let interrupts = if let Some(interrupts) = dev.interrupts_extended(devtree) {
                    Either::Left(interrupts)
                } else if let Some(interrupts) = dev.interrupts(devtree) {
                    Either::Right(interrupts)
                } else {
                    return None;
                };

                for (context, (parent, _)) in interrupts
                    .enumerate()
                    .filter(|(_, (_, irq))| is_supervisor_source(irq))
                {
                    if parent.phandle == hlic_node.phandle {
                        return Some((context, dev));
                    }
                }

                None
            })
            .unwrap();

        let mmio_range = {
            let reg = dev.regs().unwrap().next()?.unwrap();

            let start = PhysicalAddress::new(reg.starting_address);
            Range::from(start..start.checked_add(reg.size.unwrap()).unwrap())
        };

        let mmio_region = with_kernel_aspace(|aspace| {
            let layout = Layout::from_size_align(mmio_range.size(), PAGE_SIZE).unwrap();
            aspace
                .map(
                    layout,
                    Permissions::READ | Permissions::WRITE,
                    |range, perms, batch| {
                        let region = AddressSpaceRegion::new_phys(
                            range,
                            perms,
                            mmio_range,
                            Some("PLIC".to_string()),
                        );
                        region.commit(batch, range, true)?;
                        Ok(region)
                    },
                )
                .unwrap()
                .range
        });

        // Specifies how many external interrupts are supported by this controller.
        let ndev = dev.property("riscv,ndev").unwrap().as_usize()?;

        let regs: *mut PlicRegs = mmio_region.start.as_ptr().cast_mut().cast();

        Ok(Plic {
            regs,
            context,
            ndev,
        })
    }
}

impl InterruptController for Plic {
    fn irq_claim(&mut self) -> Option<IrqClaim> {
        // Safety: constructor ensures this is valid, but at the end of the day, this is writing to
        // an MMIO region, so it's inherently unsafe.
        let regs = unsafe { self.regs.as_mut().unwrap() };
        regs.claim(self.context)
    }

    fn irq_complete(&mut self, claim: IrqClaim) {
        // Safety: constructor ensures this is valid, but at the end of the day, this is writing to
        // an MMIO region, so it's inherently unsafe.
        let regs = unsafe { self.regs.as_mut().unwrap() };
        regs.complete(self.context, claim);
    }

    fn irq_mask(&mut self, irq_num: u32) {
        assert!(irq_num > 0 && irq_num as usize <= self.ndev);
        // Safety: constructor ensures this is valid, but at the end of the day, this is writing to
        // an MMIO region, so it's inherently unsafe.
        let regs = unsafe { self.regs.as_mut().unwrap() };
        regs.set_priority(NonZero::new(irq_num as usize).unwrap(), 1);
        regs.enable(self.context, NonZero::new(irq_num as usize).unwrap(), true);
    }

    fn irq_unmask(&mut self, irq_num: u32) {
        assert!(irq_num as usize <= self.ndev);
        // Safety: constructor ensures this is valid, but at the end of the day, this is writing to
        // an MMIO region, so it's inherently unsafe.
        let regs = unsafe { self.regs.as_mut().unwrap() };
        regs.set_priority(NonZero::new(irq_num as usize).unwrap(), 1);
        regs.enable(self.context, NonZero::new(irq_num as usize).unwrap(), false);
    }
}

impl PlicRegs {
    /// Sets the priority of the given interrupt source.
    pub fn set_priority(&mut self, irq: NonZero<usize>, priority: u32) {
        assert!(priority < 8);
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        unsafe {
            self.source_priority[irq.get()].write(priority);
        }
    }

    /// Retrieves the pending interrupts for the given IRQ lane. The returned `u32` should be interpreted
    /// as a bitfield to determine which interrupts are pending.
    pub fn pending(&self, irq_lane: usize) -> u32 {
        debug_assert!(irq_lane < 32);
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        unsafe { self.pending[irq_lane].read() }
    }

    /// Enable or disable the given interrupt source for the given context.
    pub fn enable(&mut self, context: usize, irq: NonZero<usize>, enable: bool) {
        assert!(irq.get() <= 1023 && context < MAX_CONTEXTS);
        let irq_lane = irq.get() / 32;
        let irq = irq.get() % 32;
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        unsafe {
            self.enable[context][irq_lane].set_bits(1u32 << irq, enable);
        }
    }

    /// Sets the priority threshold for the given context. All interrupts to the given context with
    /// a priority less than or equal to the threshold will be masked.
    pub fn set_priority_threshold(&mut self, context: usize, priority: u32) {
        assert!(context < MAX_CONTEXTS && priority <= 7);
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        unsafe {
            self.thresholds_claims[context].threshold.write(priority);
        }
    }

    /// Send an interrupt claim message to the PLIC signalling that we will service an interrupt request
    /// for the given target context. Returns the highest priority interrupt that is pending or `None`
    /// if no interrupts where pending for the target context.
    pub fn claim(&mut self, context: usize) -> Option<IrqClaim> {
        assert!(context < MAX_CONTEXTS);
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        let claim = unsafe { self.thresholds_claims[context].claim_complete.read() };
        // Safety: we just obtained the value from the MMIO register, so it's valid.
        NonZero::new(claim).map(|raw| unsafe { IrqClaim::from_raw(raw) })
    }

    /// Send an interrupt complete message to the PLIC signalling that we have serviced the interrupt request.
    ///
    /// # Safety
    ///
    /// The `claim` must be *the same* value as the one returned by the `[claim`] method.
    pub fn complete(&mut self, context: usize, claim: IrqClaim) {
        assert!(context < MAX_CONTEXTS);
        // Safety: PLIC constructor & type layout ensure this ptr is valid, but at the end of the day,
        // this is writing to an MMIO region, so it's inherently unsafe.
        unsafe {
            self.thresholds_claims[context]
                .claim_complete
                .write(claim.as_u32());
        }
    }
}

#[repr(transparent)]
pub struct MmioReg<T> {
    value: MaybeUninit<T>,
}

impl<T> MmioReg<T> {
    pub unsafe fn zeroed() -> Self {
        Self {
            value: MaybeUninit::zeroed(),
        }
    }
    pub unsafe fn uninit() -> Self {
        Self {
            value: MaybeUninit::uninit(),
        }
    }
    pub const fn from(value: T) -> Self {
        Self {
            value: MaybeUninit::new(value),
        }
    }
}

// Generic implementation (WARNING: requires aligned pointers!)
impl<T> MmioReg<T> {
    unsafe fn read(&self) -> T {
        // Safety: ensured by caller
        unsafe { ptr::read_volatile(ptr::addr_of!(self.value).cast::<T>()) }
    }

    unsafe fn write(&mut self, value: T) {
        // Safety: ensured by caller
        unsafe { ptr::write_volatile(ptr::addr_of_mut!(self.value).cast::<T>(), value) };
    }

    #[inline(always)]
    unsafe fn get_bits(&self, flags: T) -> bool
    where
        T: Copy + PartialEq + BitAnd<Output = T>,
    {
        // Safety: ensured by caller
        unsafe { (self.read() & flags) == flags }
    }

    #[inline(always)]
    unsafe fn set_bits(&mut self, flags: T, value: bool)
    where
        T: BitOr<Output = T> + BitAnd<Output = T> + Not<Output = T>,
    {
        // Safety: ensured by caller
        unsafe {
            let tmp: T = if value {
                self.read() | flags
            } else {
                self.read() & !flags
            };
            self.write(tmp);
        }
    }
}

#[expect(clippy::match_like_matches_macro, reason = "its cleaner this way")]
fn is_supervisor_source(addr: &IrqSource) -> bool {
    match addr {
        IrqSource::C1(u32::MAX) | IrqSource::C3(u32::MAX, _, _) => false,
        IrqSource::C1(11) => false,
        _ => true,
    }
}

pub enum Either<L, R> {
    /// A value of type `L`.
    Left(L),
    /// A value of type `R`.
    Right(R),
}

impl<L, R, T> Iterator for Either<L, R>
where
    L: Iterator<Item = T>,
    R: Iterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Either::Left(left) => left.next(),
            Either::Right(right) => right.next(),
        }
    }
}
