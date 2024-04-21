use crate::boot_info::BootInfo;
use crate::elf::ElfSections;
use crate::kconfig;
use core::ops::{Add, Range};
use core::ptr::addr_of;
use core::{ptr, slice};
use vmm::{
    AddressRangeExt, BumpAllocator, EntryFlags, Flush, FrameAllocator, FrameUsage, Mode,
    PhysicalAddress, VirtualAddress, INIT,
};

type VMMode = INIT<kconfig::MEMORY_MODE>;

const INITIAL_STACK_PAGES: usize = 32;

pub struct Mapper<'dt> {
    inner: vmm::Mapper<VMMode, BumpAllocator<'dt, VMMode>>,
    flush: Flush<VMMode>,
    boot_info: &'dt BootInfo<'dt>,
    kernel: ElfSections,

    tls_size_pages: usize,
    hartmem_size_pages_phys: usize,
    hartmem_size_pages_virt: usize,
}

impl<'dt> Mapper<'dt> {
    pub fn new(
        alloc: BumpAllocator<'dt, VMMode>,
        boot_info: &'dt BootInfo<'dt>,
        kernel: ElfSections,
    ) -> Result<Self, vmm::Error> {
        let tls_size_pages =
            (kernel.tdata.virt.size() + kernel.tbss.virt.size()).div_ceil(kconfig::PAGE_SIZE);

        Ok(Self {
            inner: vmm::Mapper::new(0, alloc)?,
            flush: Flush::empty(0),
            boot_info,
            kernel,

            tls_size_pages,
            hartmem_size_pages_phys: tls_size_pages + INITIAL_STACK_PAGES,
            hartmem_size_pages_virt: tls_size_pages + kconfig::STACK_SIZE_PAGES_KERNEL,
        })
    }

    pub fn activate_page_table(&self) {
        kconfig::MEMORY_MODE::activate_table(0, self.inner.root_table().addr());
    }

    pub fn frame_usage(&self) -> FrameUsage {
        self.inner.allocator().frame_usage()
    }

    pub fn frame_alloc_offset(&self) -> usize {
        self.inner.allocator().offset()
    }

    pub fn kernel_sections(&self) -> &ElfSections {
        &self.kernel
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

    pub fn map_kernel_sections(&mut self) -> Result<(), vmm::Error> {
        log::trace!(
            "Mapping kernel text region {:?} => {:?}...",
            self.kernel.text.virt,
            self.kernel.text.phys
        );
        self.inner.map_range_with_flush(
            self.kernel.text.virt.clone(),
            self.kernel.text.phys.clone(),
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel rodata region {:?} => {:?}...",
            self.kernel.rodata.virt,
            self.kernel.rodata.phys
        );
        self.inner.map_range_with_flush(
            self.kernel.rodata.virt.clone(),
            self.kernel.rodata.phys.clone(),
            EntryFlags::READ,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel bss region {:?} => {:?}...",
            self.kernel.bss.virt,
            self.kernel.bss.phys
        );
        self.inner.map_range_with_flush(
            self.kernel.bss.virt.clone(),
            self.kernel.bss.phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        log::trace!(
            "Mapping kernel data region {:?} => {:?}...",
            self.kernel.data.virt,
            self.kernel.data.phys
        );
        self.inner.map_range_with_flush(
            self.kernel.data.virt.clone(),
            self.kernel.data.phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        Ok(())
    }

    pub fn hartmem_virt(&self, hartid: usize) -> Range<VirtualAddress> {
        let end = unsafe {
            VirtualAddress::new(
                kconfig::MEMORY_MODE::PHYS_OFFSET
                    - (self.hartmem_size_pages_virt * kconfig::PAGE_SIZE * hartid),
            )
        };

        end.sub(self.hartmem_size_pages_virt * kconfig::PAGE_SIZE)..end
    }

    pub fn map_hartmem(&mut self, hartid: usize) -> Result<Range<PhysicalAddress>, vmm::Error> {
        let hartmem_phys = {
            let start = self
                .inner
                .allocator_mut()
                .allocate_frames(self.hartmem_size_pages_phys)?;

            start..start.add(self.hartmem_size_pages_phys * kconfig::PAGE_SIZE)
        };

        // the tls region is at the top of hartmem
        let hartmem_virt = unsafe {
            let end = VirtualAddress::new(
                    kconfig::MEMORY_MODE::PHYS_OFFSET
                        - (self.hartmem_size_pages_virt * kconfig::PAGE_SIZE * hartid),
                );

            end.sub(self.hartmem_size_pages_phys * kconfig::PAGE_SIZE)..end
        };

        log::trace!(
            "Mapping hart {hartid} hart-local region {hartmem_virt:?} => {hartmem_phys:?}..."
        );
        self.inner.map_range_with_flush(
            hartmem_virt,
            hartmem_phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        Ok(hartmem_phys)
    }
}
