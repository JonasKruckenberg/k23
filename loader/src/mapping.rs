// without -zseparate-loadable-segments
// ro
// virt 0xffffffff80000000..0xffffffff80003fe0
// phys 0x0000000000000000..0x0000000000003fe0
// rx
// virt 0xffffffff80004fe0..0xffffffff8001b312
// phys 0x0000000000003fe0..0x000000000001a312
// rw
// virt 0xffffffff8001c320..0xffffffff80020000
// phys 0x000000000001a320..0x000000000001d4b8
// rw
// virt 0xffffffff800204b8..0xffffffff800206d0
// phys 0x000000000001d4b8..0x000000000001d590

// with -zseparate-loadable-segments
// ro
// virt 0xffffffff80000000..0xffffffff80004168
// phys 0x0000000000000000..0x0000000000004168
// rx
// virt 0xffffffff80005000..0xffffffff8001b5f2
// phys 0x00000000000165f2..0x00000000000325f2
// rw
// virt 0xffffffff8001c000..0xffffffff80020000
// phys 0x000000000001c000..0x000000000001f230
// rw
// virt 0xffffffff80020000..0xffffffff80020218
// phys 0x0000000000020000..0x00000000000200d8

// without -zseparate-loadable-segments
// ro 0xffffffff80000000..0xffffffff80004000 => 0x00000..0x04000
// rx 0xffffffff80004000..0xffffffff8001c000 => 0x03000..0x1b000
// rw 0xffffffff8001c000..0xffffffff80020000 => 0x1a000..0x1e000
//    0xffffffff8001f4b8..0xffffffff80020000 => BSS
// rw 0xffffffff80020000..0xffffffff80021000 => 0x1d000..0x1e000
//    0xffffffff80020590..0xffffffff800206d0 => BSS

// with -zseparate-loadable-segments
// ro 0xffffffff80000000..0xffffffff80005000 => 0x9fb22000..0x9fb27000
// rx 0xffffffff80005000..0xffffffff8001c000 => 0x9fb27000..0x9fb3e000
// rw 0xffffffff8001c000..0xffffffff80020000 => 0x9fb3e000..0x9fb42000
//    0xffffffff8001f230..0xffffffff80020000 => BSS
// rw 0xffffffff80020000..0xffffffff80021000 => 0x9fb42000..0x9fb43000
//    0xffffffff800200d8..0xffffffff80020218 => BSS

use crate::machine_info::MachineInfo;
use crate::payload::Payload;
use crate::{kconfig, LoaderRegions};
use core::mem::MaybeUninit;
use core::ops::{Div, Range};
use core::{mem, ptr, slice};
use loader_api::{BootInfo, MemoryRegion};
use object::Object;
use vmm::{
    BumpAllocator, EntryFlags, Flush, Mapper, Mode, PhysicalAddress, TlsTemplate, VirtualAddress,
    INIT,
};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

pub fn set_up_mappings(
    payload: Payload,
    machine_info: &MachineInfo,
    loader_regions: &LoaderRegions,
    fdt_virt: VirtualAddress,
    frame_allocator: &mut BumpAllocator<INIT<kconfig::MEMORY_MODE>>,
) -> Result<Mappings, vmm::Error> {
    let mut mapper = Mapper::new(0, frame_allocator)?;
    let mut flush = Flush::empty(0);

    let maybe_tls_template = mapper.map_elf_file(&payload.elf_file, &mut flush)?;
    log::trace!("maybe_tls_template {maybe_tls_template:?}");

    let physical_memory_offset = VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET);

    let mut state = State {
        physical_memory_offset,
        dynamic_range_end: physical_memory_offset,
        mapper,
        flush,
    };

    // Allocate memory for TLS segments
    let maybe_tls_allocation = maybe_tls_template
        .map(|template| state.allocate_tls(template, machine_info))
        .transpose()?;

    // Map stacks for payload
    let stack_size_pages = usize::try_from(payload.loader_config.kernel_stack_size_pages).unwrap();

    let stacks_virt = state.map_payload_stacks(machine_info, stack_size_pages)?;

    // Map misc structs that will be accessed through the physical memory mapping
    let bootinfo_phys = state.allocate_bootinfo()?;

    // Map all physical memory
    state.map_physical_memory(machine_info)?;

    // Identity map self
    let loader_virt = state.identity_map_loader(&loader_regions)?;

    let frame_usage = state.mapper.allocator().frame_usage();
    log::info!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
        (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    Ok(Mappings {
        entry: VirtualAddress::new(usize::try_from(payload.elf_file.entry()).unwrap()),
        table_addr: state.mapper.root_table().addr(),
        physical_memory_offset: state.physical_memory_offset,
        maybe_tls_allocation,
        stacks_virt,
        stack_size: stack_size_pages * kconfig::PAGE_SIZE,
        loader_virt,
        fdt_virt,
        bootinfo_phys,
        dynamic_range_start: VirtualAddress::default(),
        dynamic_range_end: state.dynamic_range_end,
    })
}

struct State<'a> {
    /// The offset at which the physical memory should be mapped
    physical_memory_offset: VirtualAddress,
    /// The highest available virtual address
    dynamic_range_end: VirtualAddress,

    mapper: Mapper<'a, INIT<kconfig::MEMORY_MODE>>,
    flush: Flush<INIT<kconfig::MEMORY_MODE>>,
}

impl<'a> State<'a> {
    // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    // as opposed to m-mode where it would take effect after jump tp u-mode.
    // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    // We will then unmap the loader in the kernel.
    pub fn identity_map_loader(
        &mut self,
        loader_regions: &LoaderRegions,
    ) -> Result<Range<VirtualAddress>, vmm::Error> {
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

        Ok(self.phys_to_virt(loader_regions.executable.start)
            ..self.phys_to_virt(loader_regions.read_write.end))
    }

    // Allocate physical memory for the BootInfo struct and MemoryRegion slice
    // this will be populated later
    pub fn allocate_bootinfo(&mut self) -> Result<Range<PhysicalAddress>, vmm::Error> {
        let base = self.mapper.allocator_mut().allocate_frame()?;

        Ok(base..base.add(kconfig::PAGE_SIZE))
    }

    pub fn map_physical_memory(&mut self, machine_info: &MachineInfo) -> Result<(), vmm::Error> {
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

        Ok(())
    }

    pub fn map_payload_stacks(
        &mut self,
        machine_info: &MachineInfo,
        stack_size_page: usize,
    ) -> Result<Range<VirtualAddress>, vmm::Error> {
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

        Ok(stacks_virt)
    }

    fn allocate_tls(
        &mut self,
        template: TlsTemplate,
        machine_info: &MachineInfo,
    ) -> Result<TlsAllocation, vmm::Error> {
        let size_pages = template.mem_size.div_ceil(kconfig::PAGE_SIZE);

        let phys = {
            let start = self
                .mapper
                .allocator_mut()
                .allocate_frames(size_pages * machine_info.cpus)?;

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

        Ok(TlsAllocation {
            virt,
            per_hart_size: size_pages,
            tls_template: template,
        })
    }

    fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.physical_memory_offset.add(phys.as_raw())
    }

    fn allocate_virt_dynamic(&mut self, pages: usize) -> Range<VirtualAddress> {
        let range = self.dynamic_range_end.sub(pages * kconfig::PAGE_SIZE)..self.dynamic_range_end;
        self.dynamic_range_end = range.start;
        range
    }
}

pub struct TlsAllocation {
    /// The TLS region in virtual memory
    virt: Range<VirtualAddress>,
    /// The per-hart size of the TLS region.
    /// Both `virt` and `phys` size is an integer multiple of this.
    per_hart_size: usize,
    /// The template we allocated for
    tls_template: TlsTemplate,
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

pub struct Mappings {
    entry: VirtualAddress,
    physical_memory_offset: VirtualAddress,
    maybe_tls_allocation: Option<TlsAllocation>,
    loader_virt: Range<VirtualAddress>,
    table_addr: VirtualAddress,
    stacks_virt: Range<VirtualAddress>,
    stack_size: usize,
    fdt_virt: VirtualAddress,
    bootinfo_phys: Range<PhysicalAddress>,
    dynamic_range_start: VirtualAddress,
    dynamic_range_end: VirtualAddress,
}

impl Mappings {
    pub fn activate_table(&self) {
        kconfig::MEMORY_MODE::activate_table(0, self.table_addr);
    }

    pub fn entry_point(&self) -> VirtualAddress {
        self.entry
    }

    pub fn stack_region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
        let end = self.stacks_virt.end.sub(self.stack_size * hartid);

        end.sub(self.stack_size)..end
    }

    pub fn tls_region_for_hart(&self, hartid: usize) -> Option<Range<VirtualAddress>> {
        Some(self.maybe_tls_allocation.as_ref()?.region_for_hart(hartid))
    }

    pub fn initialize_tls_region_for_hart(&self, hartid: usize) {
        if let Some(alloc) = &self.maybe_tls_allocation {
            alloc.initialize_for_hart(hartid)
        }
    }

    pub fn finalize_memory_regions(
        &mut self,
        f: impl FnOnce(&Self, &'static mut [MaybeUninit<MemoryRegion>]) -> &'static mut [MemoryRegion],
    ) -> &'static mut [MemoryRegion] {
        let offset = mem::size_of::<BootInfo>();
        let base_ptr =
            self.bootinfo_phys.start.add(offset).as_raw() as *mut MaybeUninit<MemoryRegion>;
        let num_regions = (kconfig::PAGE_SIZE - offset).div(mem::size_of::<MemoryRegion>());

        let memory_regions = unsafe { slice::from_raw_parts_mut(base_ptr, num_regions) };

        f(self, memory_regions)
    }

    pub fn finalize_boot_info(
        &mut self,
        machine_info: &'static MachineInfo,
        memory_regions: &'static mut [MemoryRegion],
    ) {
        let boot_info = unsafe {
            let boot_info_ptr = self.bootinfo_phys.start.as_raw() as *mut MaybeUninit<BootInfo>;
            &mut *boot_info_ptr
        };

        // memory_regions: &'static mut [MemoryRegion] is a reference to physical memory, but going forward
        // we need it to be a reference to virtual memory.
        let memory_regions = unsafe {
            let ptr = memory_regions
                .as_mut_ptr()
                .byte_add(self.physical_memory_offset.as_raw());
            slice::from_raw_parts_mut(ptr, memory_regions.len())
        };

        let boot_info = boot_info.write(BootInfo::new(memory_regions));
        boot_info.boot_hart = machine_info.boot_hart;
        boot_info.physical_memory_offset = self.physical_memory_offset;
        boot_info.fdt_virt = Some(self.fdt_virt);
        boot_info.loader_virt = Some(self.loader_virt.clone());
        boot_info.free_virt = self.dynamic_range_start..self.dynamic_range_end;
    }

    pub fn boot_info(&self) -> &'static mut BootInfo {
        let boot_info_ptr = self
            .physical_memory_offset
            .add(self.bootinfo_phys.start.as_raw())
            .as_raw() as *mut BootInfo;

        unsafe { &mut *boot_info_ptr }
    }
}
