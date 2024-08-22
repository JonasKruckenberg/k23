use crate::machine_info::MachineInfo;
use crate::payload::Payload;
use crate::{kconfig, LoaderRegions};
use core::ops::Range;
use core::{ptr, slice};
use kmm::{
    BumpAllocator, EntryFlags, Flush, Mapper, Mode, PhysicalAddress, TlsTemplate, VirtualAddress,
    INIT,
};
use object::Object;

pub struct PageTableBuilder<'a> {
    /// The offset at which the physical memory should be mapped
    physical_memory_offset: VirtualAddress,
    /// The highest available virtual address
    free_range_end: VirtualAddress,

    mapper: Mapper<'a, INIT<kconfig::MEMORY_MODE>>,
    flush: Flush<INIT<kconfig::MEMORY_MODE>>,

    result: PageTableResult,
}

impl<'a> PageTableBuilder<'a> {
    pub fn from_alloc(
        frame_allocator: &'a mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    ) -> crate::Result<Self> {
        let physical_memory_offset = VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET);

        let mapper = Mapper::new(0, frame_allocator)?;

        Ok(Self {
            physical_memory_offset,
            free_range_end: physical_memory_offset,

            result: PageTableResult {
                page_table_addr: mapper.root_table().addr(),
                free_range_virt: VirtualAddress::default()..physical_memory_offset,

                // set by the methods below
                entry: VirtualAddress::default(),
                stack_size: 0,
                maybe_tls_allocation: None,
                stacks_virt: Range::default(),
                loader_virt: Range::default(),
            },

            mapper,
            flush: Flush::empty(0),
        })
    }

    pub fn map_payload(
        mut self,
        payload: &Payload,
        machine_info: &MachineInfo,
    ) -> crate::Result<Self> {
        let maybe_tls_template = self
            .mapper
            .map_elf_file(&payload.elf_file, &mut self.flush)?;

        // Allocate memory for TLS segments
        if let Some(template) = maybe_tls_template {
            self = self.allocate_tls(template, machine_info)?;
        }

        // Map stacks for payload
        let stack_size_pages = usize::try_from(payload.loader_config.kernel_stack_size_pages)?;

        self = self.map_payload_stacks(machine_info, stack_size_pages)?;

        self.result.entry = VirtualAddress::new(usize::try_from(payload.elf_file.entry())?);
        self.result.stack_size = stack_size_pages * kconfig::PAGE_SIZE;

        Ok(self)
    }

    fn allocate_tls(
        mut self,
        template: TlsTemplate,
        machine_info: &MachineInfo,
    ) -> crate::Result<Self> {
        let size_pages = template.mem_size.div_ceil(kconfig::PAGE_SIZE);

        let phys = {
            let start = self
                .mapper
                .allocator_mut()
                .allocate_frames_zeroed(size_pages * machine_info.cpus)?;

            start..start.add(size_pages * kconfig::PAGE_SIZE * machine_info.cpus)
        };

        let virt = self.allocate_virt_dynamic(size_pages * machine_info.cpus);

        log::trace!("Mapping TLS region {:?} => {:?}...", virt, phys);

        self.mapper.map_range(
            virt.clone(),
            phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        self.result.maybe_tls_allocation = Some(TlsAllocation {
            virt,
            per_hart_size: size_pages,
            tls_template: template,
        });

        Ok(self)
    }

    // TODO add guard pages below each stack allocation
    pub fn map_payload_stacks(
        mut self,
        machine_info: &MachineInfo,
        stack_size_page: usize,
    ) -> crate::Result<Self> {
        let stacks_phys = {
            let start = self
                .mapper
                .allocator_mut()
                .allocate_frames(stack_size_page)?;

            start..start.add(stack_size_page * kconfig::PAGE_SIZE * machine_info.cpus)
        };

        let stacks_virt = self.allocate_virt_dynamic(stack_size_page * machine_info.cpus);

        log::trace!("Mapping stack region {stacks_virt:?} => {stacks_phys:?}...");
        self.mapper.map_range(
            stacks_virt.clone(),
            stacks_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        self.result.stacks_virt = stacks_virt;

        Ok(self)
    }

    // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    // as opposed to m-mode where it would take effect after jump tp u-mode.
    // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    // We will then unmap the loader in the kernel.
    pub fn identity_map_loader(mut self, loader_regions: &LoaderRegions) -> crate::Result<Self> {
        log::trace!(
            "Identity mapping own executable region {:?}...",
            loader_regions.executable
        );
        self.mapper.map_range_identity(
            loader_regions.executable.clone(),
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut self.flush,
        )?;

        log::trace!(
            "Identity mapping own read-only region {:?}...",
            loader_regions.read_only
        );
        self.mapper.map_range_identity(
            loader_regions.read_only.clone(),
            EntryFlags::READ,
            &mut self.flush,
        )?;

        log::trace!(
            "Identity mapping own read-write region {:?}...",
            loader_regions.read_write
        );
        self.mapper.map_range_identity(
            loader_regions.read_write.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        self.result.loader_virt = VirtualAddress::new(loader_regions.executable.start.as_raw())
            ..VirtualAddress::new(loader_regions.read_write.end.as_raw());

        Ok(self)
    }

    pub fn map_physical_memory(mut self, machine_info: &MachineInfo) -> Result<Self, kmm::Error> {
        for region_phys in &machine_info.memories {
            let region_virt =
                self.phys_to_virt(region_phys.start)..self.phys_to_virt(region_phys.end);

            log::trace!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
            self.mapper.map_range(
                region_virt,
                region_phys.clone(),
                EntryFlags::READ | EntryFlags::WRITE,
                &mut self.flush,
            )?;
        }

        Ok(self)
    }

    pub fn print_statistics(self) -> Self {
        const KIB: usize = 1024;
        const MIB: usize = 1024 * KIB;

        let frame_usage = self.mapper.allocator().frame_usage();
        log::info!(
            "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
            (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
            (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
            (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
        );

        self
    }

    pub fn result(self) -> PageTableResult {
        self.result
    }

    fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.physical_memory_offset.add(phys.as_raw())
    }

    fn allocate_virt_dynamic(&mut self, pages: usize) -> Range<VirtualAddress> {
        let range = self.free_range_end.sub(pages * kconfig::PAGE_SIZE)..self.free_range_end;
        self.free_range_end = range.start;
        range
    }
}

#[derive(Default)]
pub struct PageTableResult {
    /// The address of the root page table
    page_table_addr: VirtualAddress,
    /// The range of addresses that may be used for dynamic allocations
    pub free_range_virt: Range<VirtualAddress>,

    /// Memory region allocated for payload TLS regions, as well as the template TLS to use for
    /// initializing them.
    pub maybe_tls_allocation: Option<TlsAllocation>,
    /// Memory region allocated for payload stacks
    pub stacks_virt: Range<VirtualAddress>,
    /// The size of each stack in bytes
    stack_size: usize,

    /// Memory region allocated for loader itself
    pub loader_virt: Range<VirtualAddress>,
    /// The entry point address of the payload
    entry: VirtualAddress,
}

impl PageTableResult {
    pub fn payload_entry(&self) -> VirtualAddress {
        self.entry
    }

    pub fn stack_region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
        let end = self.stacks_virt.end.sub(self.stack_size * hartid);

        end.sub(self.stack_size)..end
    }

    pub fn tls_region_for_hart(&self, hartid: usize) -> Option<Range<VirtualAddress>> {
        Some(self.maybe_tls_allocation.as_ref()?.region_for_hart(hartid))
    }

    pub fn activate_table(&self) {
        kconfig::MEMORY_MODE::activate_table(0, self.page_table_addr);
    }

    pub fn init_tls_region_for_hart(&self, hartid: usize) {
        if let Some(allocation) = &self.maybe_tls_allocation {
            allocation.initialize_for_hart(hartid);
        }
    }
}

pub struct TlsAllocation {
    /// The TLS region in virtual memory
    virt: Range<VirtualAddress>,
    /// The per-hart size of the TLS region.
    /// Both `virt` and `phys` size is an integer multiple of this.
    per_hart_size: usize,
    /// The template we allocated for
    pub tls_template: TlsTemplate,
}

impl TlsAllocation {
    pub fn region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
        let start = self.virt.start.add(self.per_hart_size * hartid);

        start..start.add(self.per_hart_size)
    }

    pub fn initialize_for_hart(&self, hartid: usize) {
        let src = unsafe {
            slice::from_raw_parts(
                self.tls_template.start_addr.as_raw() as *const u8,
                self.tls_template.file_size,
            )
        };

        let dst = unsafe {
            slice::from_raw_parts_mut(
                self.virt.start.add(self.per_hart_size * hartid).as_raw() as *mut u8,
                self.tls_template.file_size,
            )
        };

        log::trace!(
            "Copying tdata from {:?} to {:?}",
            src.as_ptr_range(),
            dst.as_ptr_range()
        );

        debug_assert_eq!(src.len(), dst.len());
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }
    }
}
