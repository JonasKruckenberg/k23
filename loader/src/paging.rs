use crate::boot_info::BootInfo;
use crate::elf::ElfSections;
use crate::kconfig;
use core::ops::Range;
use core::ptr::addr_of;
use core::{ptr, slice};
use vmm::{
    AddressRangeExt, BumpAllocator, EntryFlags, Flush, FrameAllocator, Mapper, Mode,
    PhysicalAddress, VirtualAddress, INIT,
};

type VMMode = INIT<kconfig::MEMORY_MODE>;

const INITIAL_STACK_PAGES: usize = 32;

pub fn init(
    alloc: BumpAllocator<VMMode>,
    boot_info: &BootInfo,
    kernel: ElfSections,
) -> Result<PageTableResult, vmm::Error> {
    let mut state = State::new(alloc, boot_info, kernel)?;

    state.map_physical_memory()?;
    state.identity_map_loader()?;
    state.map_kernel_sections()?;

    for hartid in 0..boot_info.cpus {
        state.map_hartmem(hartid)?;
    }

    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;

    let frame_usage = state.mapper.allocator().frame_usage();
    log::info!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
        (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    Ok(PageTableResult {
        table_addr: state.mapper.root_table().addr(),
        kernel_entry_virt: state.kernel.entry,
        hartmem_size_pages_virt: state.hartmem_size_pages_virt,
        frame_alloc_offset: state.mapper.allocator().offset(),
    })
}

pub struct PageTableResult {
    pub kernel_entry_virt: VirtualAddress,
    pub frame_alloc_offset: usize,
    table_addr: VirtualAddress,
    hartmem_size_pages_virt: usize,
}

impl PageTableResult {
    pub fn activate_table(&self) {
        kconfig::MEMORY_MODE::activate_table(0, self.table_addr);
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
}

struct State<'dt> {
    mapper: Mapper<VMMode, BumpAllocator<'dt, VMMode>>,
    flush: Flush<VMMode>,

    boot_info: &'dt BootInfo<'dt>,
    kernel: ElfSections,

    tls_size_pages: usize,
    hartmem_size_pages_phys: usize,
    hartmem_size_pages_virt: usize,
}

impl<'dt> State<'dt> {
    pub fn new(
        alloc: BumpAllocator<'dt, VMMode>,
        boot_info: &'dt BootInfo<'dt>,
        kernel: ElfSections,
    ) -> Result<Self, vmm::Error> {
        let tls_size_pages =
            (kernel.tdata.virt.size() + kernel.tbss.virt.size()).div_ceil(kconfig::PAGE_SIZE);

        Ok(Self {
            mapper: Mapper::new(0, alloc)?,
            flush: Flush::empty(0),
            boot_info,
            kernel,

            tls_size_pages,
            hartmem_size_pages_phys: tls_size_pages + INITIAL_STACK_PAGES,
            hartmem_size_pages_virt: tls_size_pages + kconfig::STACK_SIZE_PAGES_KERNEL,
        })
    }

    pub fn map_physical_memory(&mut self) -> Result<(), vmm::Error> {
        for region_phys in &self.boot_info.memories {
            let region_virt = kconfig::MEMORY_MODE::phys_to_virt(region_phys.start)
                ..kconfig::MEMORY_MODE::phys_to_virt(region_phys.end);

            log::trace!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
            self.mapper.map_range_with_flush(
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
        self.mapper.identity_map_range_with_flush(
            own_executable_region,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut self.flush,
        )?;

        log::trace!("Identity mapping own read-only region {own_read_only_region:?}...");
        self.mapper.identity_map_range_with_flush(
            own_read_only_region,
            EntryFlags::READ,
            &mut self.flush,
        )?;

        log::trace!("Identity mapping own read-write region {own_read_write_region:?}...");
        self.mapper.identity_map_range_with_flush(
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
        self.mapper.map_range_with_flush(
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
        self.mapper.map_range_with_flush(
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
        self.mapper.map_range_with_flush(
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
        self.mapper.map_range_with_flush(
            self.kernel.data.virt.clone(),
            self.kernel.data.phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        Ok(())
    }

    pub fn map_hartmem(&mut self, hartid: usize) -> Result<Range<PhysicalAddress>, vmm::Error> {
        let hartmem_phys = {
            let start = self
                .mapper
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
        self.mapper.map_range_with_flush(
            hartmem_virt,
            hartmem_phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        // copy tdata
        unsafe {
            let src = slice::from_raw_parts(
                self.kernel.tdata.phys.start.as_raw() as *const u8,
                self.kernel.tdata.phys.size(),
            );

            let tdata_addr = hartmem_phys.end.sub(src.len());
            let dst = slice::from_raw_parts_mut(tdata_addr.as_raw() as *mut u8, src.len());

            log::trace!(
                "Copying tdata from {:?} to {:?}",
                src.as_ptr_range(),
                dst.as_ptr_range()
            );

            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }

        Ok(hartmem_phys)
    }
}
