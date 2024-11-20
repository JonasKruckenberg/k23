use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::{kconfig, LoaderRegions, PageAllocator};
use core::ops::Range;
use core::{ptr, slice};
use kmm::{
    BumpAllocator, EntryFlags, Flush, Mapper, Mode, PhysicalAddress, TlsTemplate, VirtualAddress,
};
use object::Object;

pub struct PageTableBuilder<'a> {
    /// The offset at which the physical memory should be mapped
    physical_memory_offset: VirtualAddress,

    mapper: Mapper<'a, kconfig::MEMORY_MODE>,
    flush: Flush<kconfig::MEMORY_MODE>,

    page_alloc: &'a mut PageAllocator,

    result: PageTableResult,
}

impl<'a> PageTableBuilder<'a> {
    pub fn from_alloc(
        frame_allocator: &'a mut BumpAllocator<'_, kconfig::MEMORY_MODE>,
        physical_memory_offset: VirtualAddress,
        page_alloc: &'a mut PageAllocator,
    ) -> crate::Result<Self> {
        let mapper = Mapper::new(0, frame_allocator)?;

        Ok(Self {
            physical_memory_offset,
            page_alloc,

            result: PageTableResult {
                page_table_addr: mapper.root_table().addr(),

                // set by the methods below
                entry: VirtualAddress::default(),
                per_hart_stack_size: 0,
                maybe_tls_allocation: None,
                stacks_virt: Range::default(),
                heap_virt: None,
                loader_region: Range::default(),
                kernel_image_offset: VirtualAddress::default(),
            },

            mapper,
            flush: Flush::empty(0),
        })
    }

    /// Map the kernel ELF, plus kernel stack and heap regions.
    pub fn map_kernel(
        mut self,
        kernel: &Kernel,
        machine_info: &MachineInfo,
    ) -> crate::Result<Self> {
        let mem_size = kernel.mem_size() as usize;
        let align = kernel.max_align() as usize;

        let kernel_image_offset = self.page_alloc.reserve_range(mem_size, align).start;
        let maybe_tls_template = self
            .mapper
            .elf(kernel_image_offset)
            .map_elf_file(&kernel.elf_file, &mut self.flush)?;

        // Allocate memory for TLS segments
        if let Some(template) = maybe_tls_template {
            self = self.allocate_tls(template, machine_info)?;
        }

        // Map stacks for kernel
        let stack_size_pages = usize::try_from(kernel.loader_config.kernel_stack_size_pages)?;
        self = self.map_kernel_stacks(machine_info, stack_size_pages)?;

        // Map heap for kernel
        if let Some(heap_size_pages) = kernel.loader_config.kernel_heap_size_pages {
            let heap_size_pages = usize::try_from(heap_size_pages)?;
            self = self.map_kernel_heap(heap_size_pages)?;
        }

        self.result.entry = kernel_image_offset.add(usize::try_from(kernel.elf_file.entry())?);
        self.result.per_hart_stack_size = stack_size_pages * kconfig::PAGE_SIZE;
        self.result.kernel_image_offset = kernel_image_offset;

        Ok(self)
    }

    /// Map the kernel thread-local storage (TLS) memory regions.
    fn allocate_tls(
        mut self,
        template: TlsTemplate,
        machine_info: &MachineInfo,
    ) -> crate::Result<Self> {
        let size_pages = template.mem_size.div_ceil(kconfig::PAGE_SIZE);
        let size = size_pages * kconfig::PAGE_SIZE * machine_info.cpus;

        let phys = {
            let start = self
                .mapper
                .allocator_mut()
                .allocate_frames_zeroed(size_pages * machine_info.cpus)?;

            start..start.add(size)
        };

        let virt = self.page_alloc.reserve_range(size, kconfig::PAGE_SIZE);

        log::trace!("Mapping TLS region {:?} => {:?}...", virt, phys);
        self.mapper.map_range(
            virt.clone(),
            phys.clone(),
            EntryFlags::READ | EntryFlags::WRITE,
            &mut self.flush,
        )?;

        self.result.maybe_tls_allocation = Some(TlsAllocation {
            virt,
            per_hart_size: size,
            tls_template: template,
        });

        Ok(self)
    }

    /// Map the kernel stacks for each hart.
    // TODO add guard pages below each stack allocation
    fn map_kernel_stacks(
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

        let stacks_virt = self.page_alloc.reserve_range(
            stack_size_page * kconfig::PAGE_SIZE * machine_info.cpus,
            kconfig::PAGE_SIZE,
        );

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

    /// Map the kernel heap region.
    fn map_kernel_heap(mut self, heap_size_pages: usize) -> crate::Result<Self> {
        self.result.heap_virt = Some(
            self.page_alloc
                .reserve_range(heap_size_pages * kconfig::PAGE_SIZE, kconfig::PAGE_SIZE),
        );
        log::trace!("Reserved heap region {:?}", self.result.heap_virt);

        Ok(self)
    }

    /// Map the physical memory into kernel address space.
    ///
    /// This can be used by the kernel for direct physical memory access or (on Risc-V) to access
    /// page tables (there is no recursive mapping).
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

    /// Identity map the loader itself (this binary).
    ///
    /// we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    /// as opposed to m-mode where it would take effect after jump tp u-mode.
    /// This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    /// We will then unmap the loader in the kernel.
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

        self.result.loader_region = VirtualAddress::new(loader_regions.executable.start.as_raw())
            ..VirtualAddress::new(loader_regions.read_write.end.as_raw());

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
}

#[derive(Default)]
pub struct PageTableResult {
    /// The address of the root page table
    page_table_addr: VirtualAddress,

    /// The entry point address of the kernel
    entry: VirtualAddress,

    /// The offset at which the kernel image was mapped
    pub kernel_image_offset: VirtualAddress,
    /// Memory region allocated for kernel TLS regions, as well as the template TLS to use for
    /// initializing them.
    pub maybe_tls_allocation: Option<TlsAllocation>,
    /// Memory region allocated for kernel stacks
    pub stacks_virt: Range<VirtualAddress>,
    /// The size of each stack in bytes
    per_hart_stack_size: usize,
    /// Memory region allocated for kernel heap
    pub heap_virt: Option<Range<VirtualAddress>>,

    /// Memory region allocated for loader itself
    pub loader_region: Range<VirtualAddress>,
}

impl PageTableResult {
    /// The kernel entry address as specified in the ELF file.
    pub fn kernel_entry(&self) -> VirtualAddress {
        self.entry
    }

    /// The kernel stack region for a given hartid.
    pub fn stack_region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
        let end = self.stacks_virt.end.sub(self.per_hart_stack_size * hartid);

        end.sub(self.per_hart_stack_size)..end
    }

    /// The thread-local storage region for a given hartid.
    pub fn tls_region_for_hart(&self, hartid: usize) -> Option<Range<VirtualAddress>> {
        Some(self.maybe_tls_allocation.as_ref()?.region_for_hart(hartid))
    }

    /// Initialize the TLS region for a given hartid.
    /// This will copy the `.tdata` section from the TLS template to the TLS region.
    pub fn init_tls_region_for_hart(&self, hartid: usize) {
        if let Some(allocation) = &self.maybe_tls_allocation {
            allocation.initialize_for_hart(hartid);
        }
    }

    /// Active the page table.
    ///
    /// This will switch to the new page table, and flush the TLB.
    ///
    /// # Safety
    ///
    /// This function is probably **the** most unsafe function in the entire loader,
    /// it will invalidate all pointers and references that are not covered by the
    /// loaders identity mapping (everything that doesn't live in the loader data/rodata/bss sections
    /// or on the loader stack).
    ///
    /// Extreme care must be taken to ensure that pointers passed to the kernel have been "translated"
    /// to virtual addresses before leaving the kernel.
    pub unsafe fn activate_table(&self) {
        kconfig::MEMORY_MODE::activate_table(0, self.page_table_addr);
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
        if self.tls_template.file_size == 0 {
            return;
        }

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
