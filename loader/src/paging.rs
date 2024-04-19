use crate::boot_info::BootInfo;
use crate::elf::ElfSections;
use crate::kconfig;
use core::ops::Range;
use core::ptr::addr_of;
use vmm::{
    AddressRangeExt, BumpAllocator, EntryFlags, Flush, FrameAllocator, Mode, PhysicalAddress,
    VirtualAddress, INIT,
};

type VMMode = INIT<kconfig::MEMORY_MODE>;

pub struct Mapper<'a, 'dt> {
    inner: vmm::Mapper<VMMode, BumpAllocator<'a, VMMode>>,
    flush: Flush<VMMode>,
    boot_info: &'a BootInfo<'dt>,

    kernel_entry: Option<VirtualAddress>,
    kernel_virt: Option<Range<VirtualAddress>>,
    fdt_virt: Option<VirtualAddress>,
    stacks_virt: Option<Range<VirtualAddress>>,
}

impl<'a, 'dt> Mapper<'a, 'dt> {
    pub fn new(
        alloc: BumpAllocator<'a, VMMode>,
        boot_info: &'a BootInfo<'dt>,
    ) -> Result<Self, vmm::Error> {
        Ok(Self {
            inner: vmm::Mapper::new(0, alloc)?,
            flush: Flush::empty(0),
            boot_info,

            kernel_entry: None,
            kernel_virt: None,
            fdt_virt: None,
            stacks_virt: None,
        })
    }

    pub fn finish(
        &self,
    ) -> Result<
        (
            usize,
            VirtualAddress,
            VirtualAddress,
            Range<VirtualAddress>,
            Range<VirtualAddress>,
        ),
        vmm::Error,
    > {
        self.inner.activate();

        Ok((
            self.inner.allocator().offset(),
            self.fdt_virt.unwrap(),
            self.kernel_entry.unwrap(),
            self.kernel_virt.clone().unwrap(),
            self.stacks_virt.clone().unwrap(),
        ))
    }

    // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    // as opposed to m-mode where it would take effect after jump tp u-mode.
    // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    // We will then unmap the loader in the kernel.
    pub fn identity_map_loader(&mut self) -> Result<(), vmm::Error> {
        extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __bss_start: u8;
            static __stack_start: u8;
        }

        let own_executable_region: Range<PhysicalAddress> = unsafe {
            PhysicalAddress::new(addr_of!(__text_start) as usize)
                ..PhysicalAddress::new(addr_of!(__text_end) as usize)
        };

        let own_read_only_region: Range<PhysicalAddress> = unsafe {
            PhysicalAddress::new(addr_of!(__rodata_start) as usize)
                ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
        };

        let own_read_write_region: Range<PhysicalAddress> = unsafe {
            let start = PhysicalAddress::new(addr_of!(__bss_start) as usize);
            let stack_start = PhysicalAddress::new(addr_of!(__stack_start) as usize);

            start
                ..stack_start
                    .add(self.boot_info.cpus * kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE)
        };

        log::trace!("Identity mapping own executable region {own_executable_region:?}...");
        self.inner.identity_map_range_with_flush(
            own_executable_region,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut self.flush,
        )?;

        log::trace!("Identity mapping own read-only region {own_read_only_region:?}...");
        self.inner.identity_map_range_with_flush(
            own_read_only_region,
            EntryFlags::READ,
            &mut self.flush,
        )?;

        log::trace!("Identity mapping own read-write region {own_read_write_region:?}...");
        self.inner.identity_map_range_with_flush(
            own_read_write_region,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        Ok(())
    }

    pub fn map_physical_memory(&mut self) -> Result<(), vmm::Error> {
        for region_phys in &self.boot_info.memories {
            let region_virt = kconfig::MEMORY_MODE::phys_to_virt(region_phys.start)
                ..kconfig::MEMORY_MODE::phys_to_virt(region_phys.end);

            log::trace!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
            self.inner.map_range_with_flush(
                region_virt,
                region_phys.clone(),
                EntryFlags::READ | EntryFlags::WRITE,
                &mut self.flush,
            )?;
        }

        Ok(())
    }

    pub fn map_fdt(&mut self) -> Result<(), vmm::Error> {
        assert_eq!(
            self.boot_info.fdt.as_ptr().align_offset(kconfig::PAGE_SIZE),
            0
        );

        let fdt_phys = unsafe {
            let base = PhysicalAddress::new(self.boot_info.fdt.as_ptr() as usize);

            (base..base.add(self.boot_info.fdt.len())).align(kconfig::PAGE_SIZE)
        };
        let fdt_virt = kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.start)
            ..kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.end);

        log::trace!("Mapping fdt region {fdt_virt:?} => {fdt_phys:?}...");
        self.inner.map_range_with_flush(
            fdt_virt.clone(),
            fdt_phys,
            EntryFlags::READ,
            &mut self.flush,
        )?;

        self.fdt_virt = Some(fdt_virt.start);

        Ok(())
    }

    pub fn map_kernel_sections(&mut self, kernel: &ElfSections) -> Result<(), vmm::Error> {
        log::trace!(
            "Mapping kernel text region {:?} => {:?}...",
            kernel.text.virt,
            kernel.text.phys
        );
        self.inner.map_range_with_flush(
            kernel.text.virt.clone(),
            kernel.text.phys.clone(),
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel rodata region {:?} => {:?}...",
            kernel.rodata.virt,
            kernel.rodata.phys
        );
        self.inner.map_range_with_flush(
            kernel.rodata.virt.clone(),
            kernel.rodata.phys.clone(),
            EntryFlags::READ,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel bss region {:?} => {:?}...",
            kernel.bss.virt,
            kernel.bss.phys
        );
        self.inner.map_range_with_flush(
            kernel.bss.virt.clone(),
            kernel.bss.phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel data region {:?} => {:?}...",
            kernel.data.virt,
            kernel.data.phys
        );
        self.inner.map_range_with_flush(
            kernel.data.virt.clone(),
            kernel.data.phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        self.kernel_virt = Some(kernel.text.virt.start..kernel.data.virt.end);
        self.kernel_entry = Some(kernel.entry);

        Ok(())
    }

    // the kernel stacks regions start at the start of TLS working downwards
    // each region has a maximum size of STACK_SIZE_PAGES, but only INITIAL_STACK_PAGES in each region are mapped upfront
    // the rest will be allocated on-demand by the kernel trap handler.
    // This way we save physical memory, by not allocating unused stack space.
    pub fn map_kernel_stacks(&mut self) -> Result<(), vmm::Error> {
        const INITIAL_STACK_PAGES: usize = 64;

        let stacks_end = unsafe { VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET) };
        let mut stack_top = stacks_end;

        for hart in 0..self.boot_info.cpus {
            let stack_phys = {
                let base = self
                    .inner
                    .allocator_mut()
                    .allocate_frames(INITIAL_STACK_PAGES)?;
                base..base.add(INITIAL_STACK_PAGES * kconfig::PAGE_SIZE)
            };

            let stack_virt = stack_top.sub(INITIAL_STACK_PAGES * kconfig::PAGE_SIZE)..stack_top;

            log::trace!(
                "Mapping kernel stack region for hart {hart} {stack_virt:?} => {stack_phys:?}..."
            );

            self.inner.map_range_with_flush(
                stack_virt,
                stack_phys,
                EntryFlags::READ | EntryFlags::WRITE,
                &mut self.flush,
            )?;

            stack_top = stack_top.sub(kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE);
        }

        self.stacks_virt = Some(stack_top..stacks_end);

        Ok(())
    }
}
