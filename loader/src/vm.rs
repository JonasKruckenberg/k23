use crate::error::Error;
use crate::kernel::{parse_inlined_kernel, Kernel};
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::{LoaderRegions, ENABLE_KASLR};
use core::ops::{Div, Range};
use core::{ptr, slice};
use loader_api::TlsTemplate;
use pmm::{
    AddressRangeExt, Arch as _, ArchFlags, BumpAllocator, FrameAllocator, PhysicalAddress,
    VirtualAddress,
};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};
use xmas_elf::P64;

fn init_frame_alloc<'a>(
    minfo: &'a MachineInfo,
    loader_regions: &LoaderRegions,
) -> BumpAllocator<'a, pmm::Riscv64Sv39> {
    unsafe { BumpAllocator::new_with_lower_bound(&minfo.memories, loader_regions.read_write.end) }
}

fn init_page_alloc(minfo: &MachineInfo) -> PageAllocator<pmm::Riscv64Sv39> {
    let mut page_alloc = if ENABLE_KASLR {
        PageAllocator::new(ChaCha20Rng::from_seed(
            minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
        ))
    } else {
        PageAllocator::new_no_kaslr()
    };
    page_alloc
}

fn setup_kernel_address_space(
    minfo: &MachineInfo,
) -> crate::Result<KernelAddressSpace<pmm::Riscv64Sv39>> {
    {
        let loader_regions = LoaderRegions::new::<pmm::Riscv64Sv39>(minfo);

        let mut frame_alloc = init_frame_alloc(minfo, &loader_regions);
        let mut page_alloc = init_page_alloc(minfo);

        let mut pmm = pmm::Riscv64Sv39::new(&mut frame_alloc, 0, VirtualAddress::default())?;

        // Identity map the loader itself (this binary).
        //
        // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
        // as opposed to m-mode where it would take effect after jump tp u-mode.
        // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
        // We will then unmap the loader in the kernel.
        let loader_phys = identity_map_loader(&mut pmm, &mut frame_alloc, loader_regions)?;

        // Map the physical memory into kernel address space.
        //
        // This will be used by the kernel to access the page tables, BootInfo struct and maybe
        // more in the future.
        let physmap = map_physical_memory(&mut pmm, &mut frame_alloc, &mut page_alloc, minfo)?;

        // activate MMU
        pmm.activate()?;

        let mut pmm = pmm::Riscv64Sv39::from_active(0, physmap.start)?;
    }

    let kernel = parse_inlined_kernel()?;
    
    // Map the kernel ELF file
    let (kernel_virt, maybe_tls_allocation) =
        map_kernel(&mut pmm, &mut frame_alloc, &mut page_alloc, &kernel, minfo)?;

    // Map stacks for kernel
    //
    // This will set up a stack for each hart that is reported by the device tree, if the kernel
    // has ways to activate other harts unknown to us currently it will have to set up stacks itself.
    let per_hart_stack_size = usize::try_from(kernel.loader_config.kernel_stack_size_pages)?;
    let stacks_virt = map_kernel_stacks(
        &mut pmm,
        &mut frame_alloc,
        &mut page_alloc,
        minfo,
        per_hart_stack_size,
    )?;

    Ok(KernelAddressSpace {
        entry: kernel_virt
            .start
            .add(usize::try_from(kernel.elf_file.header.pt2.entry_point())?),
        maybe_tls_allocation,
        per_hart_stack_size,
        kernel_virt,
        stacks_virt,
        physmap,
        loader: Default::default(),
        pmm_arch: pmm,
    })
}

pub fn identity_map_loader<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    loader_regions: LoaderRegions,
) -> crate::Result<Range<PhysicalAddress>>
where
    A: pmm::Arch,
{
    log::trace!(
        "Identity mapping own executable region {:?}...",
        loader_regions.executable
    );
    arch.identity_map_contiguous(
        frame_alloc,
        loader_regions.executable.clone(),
        ArchFlags::READ | ArchFlags::EXECUTE,
    )?;

    log::trace!(
        "Identity mapping own read-only region {:?}...",
        loader_regions.read_only
    );
    arch.identity_map_contiguous(
        frame_alloc,
        loader_regions.read_only.clone(),
        ArchFlags::READ,
    )?;

    log::trace!(
        "Identity mapping own read-write region {:?}...",
        loader_regions.read_write
    );
    arch.identity_map_contiguous(
        frame_alloc,
        loader_regions.read_write.clone(),
        ArchFlags::READ | ArchFlags::WRITE,
    )?;

    Ok(loader_regions.executable.start..loader_regions.read_write.end)
}

fn get_alignment_for_size<A>(size: usize) -> usize
where
    A: pmm::Arch,
{
    for lvl in 0..A::PAGE_TABLE_LEVELS {
        let page_size = 1 << (A::PAGE_SHIFT + lvl * A::PAGE_ENTRY_SHIFT);

        if size <= page_size {
            return page_size;
        }
    }

    unreachable!()
}

pub fn map_physical_memory<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    page_alloc: &mut PageAllocator<A>,
    minfo: &MachineInfo,
) -> crate::Result<Range<VirtualAddress>>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    let physmem_hull = minfo.memory_hull();
    let alignment = get_alignment_for_size::<A>(physmem_hull.size());

    let physmap_virt = page_alloc.reserve_range(minfo.memory_hull().size(), alignment);

    log::trace!("physmap {physmap_virt:?}");
    for region_phys in &minfo.memories {
        let region_virt = physmap_virt.start.add(region_phys.start.as_raw())
            ..physmap_virt.start.add(region_phys.end.as_raw());

        log::trace!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
        arch.map_contiguous(
            frame_alloc,
            region_virt,
            region_phys.clone(),
            ArchFlags::READ | ArchFlags::WRITE,
        )?;
    }

    Ok(physmap_virt)
}

pub struct KernelAddressSpace<A> {
    entry: VirtualAddress,
    maybe_tls_allocation: Option<TlsAllocation>,
    per_hart_stack_size: usize,
    kernel_virt: Range<VirtualAddress>,
    stacks_virt: Range<VirtualAddress>,
    physmap: Range<VirtualAddress>,
    loader: Range<PhysicalAddress>,
    pmm_arch: A,
}

// impl<A> KernelAddressSpace<A> {
//     pub fn new(
//         mut pmm_arch: A,
//         frame_alloc: &mut BumpAllocator<A>,
//         kernel: &Kernel,
//         loader_regions: LoaderRegions,
//         minfo: &MachineInfo,
//     ) -> crate::Result<Self>
//     where
//         A: pmm::Arch,
//         [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
//     {
//         // Set up the virtual memory "allocator" that we pull memory region assignments from for
//         // the various kernel regions
//         let mut page_alloc = if ENABLE_KASLR {
//             PageAllocator::new(ChaCha20Rng::from_seed(
//                 minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
//             ))
//         } else {
//             PageAllocator::new_no_kaslr()
//         };
//

//         let physmap = map_physical_memory(&mut pmm_arch, frame_alloc, &mut page_alloc, minfo)?;
//

//         let loader = identity_map_loader(&mut pmm_arch, frame_alloc, loader_regions)?;
//

//
//         
//     }
//
//     /// The kernel entry address as specified in the ELF file.
//     pub fn entry_virt(&self) -> VirtualAddress {
//         self.entry
//     }
//
//     pub fn loader_phys(&self) -> Range<PhysicalAddress> {
//         self.loader.clone()
//     }
//
//     pub fn kernel_virt(&self) -> Range<VirtualAddress> {
//         self.kernel_virt.clone()
//     }
//     pub fn physmap(&self) -> Range<VirtualAddress> {
//         self.physmap.clone()
//     }
//
//     pub fn tls_template(&self) -> Option<TlsTemplate> {
//         self.maybe_tls_allocation
//             .as_ref()
//             .map(|a| a.tls_template.clone())
//     }
//
//     /// The kernel stack region for a given hartid.
//     pub fn stack_region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
//         let end = self.stacks_virt.end.sub(self.per_hart_stack_size * hartid);
//
//         end.sub(self.per_hart_stack_size)..end
//     }
//
//     /// The thread-local storage region for a given hartid.
//     pub fn tls_region_for_hart(&self, hartid: usize) -> Option<Range<VirtualAddress>> {
//         Some(self.maybe_tls_allocation.as_ref()?.region_for_hart(hartid))
//     }
//
//     /// Initialize the TLS region for a given hartid.
//     /// This will copy the `.tdata` section from the TLS template to the TLS region.
//     pub fn init_tls_region_for_hart(&self, hartid: usize) {
//         if let Some(allocation) = &self.maybe_tls_allocation {
//             allocation.initialize_for_hart(hartid);
//         }
//     }
//
//     /// Active the page table.
//     ///
//     /// This will switch to the new page table, and flush the TLB.
//     ///
//     /// # Safety
//     ///
//     /// This function is probably **the** most unsafe function in the entire loader,
//     /// it will invalidate all pointers and references that are not covered by the
//     /// loaders identity mapping (everything that doesn't live in the loader data/rodata/bss sections
//     /// or on the loader stack).
//     ///
//     /// Extreme care must be taken to ensure that pointers passed to the kernel have been "translated"
//     /// to virtual addresses before leaving the kernel.
//     pub unsafe fn activate(&self) -> crate::Result<()>
//     where
//         A: pmm::Arch,
//     {
//         self.pmm_arch.activate().map_err(Into::into)
//     }
// }

pub struct TlsAllocation {
    /// The TLS region in virtual memory
    virt: Range<VirtualAddress>,
    /// The per-hart size of the TLS region.
    /// Both `virt` and `phys` size is an integer multiple of this.
    per_hart_size: usize,
    /// The template we allocated for
    pub tls_template: TlsTemplate,
}

// impl TlsAllocation {
//     pub fn region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
//         let start = self.virt.start.add(self.per_hart_size * hartid);
//
//         start..start.add(self.per_hart_size)
//     }
//
//     pub fn initialize_for_hart(&self, hartid: usize) {
//         if self.tls_template.file_size == 0 {
//             return;
//         }
//
//         let src = unsafe {
//             slice::from_raw_parts(
//                 self.tls_template.start_addr.as_raw() as *const u8,
//                 self.tls_template.file_size,
//             )
//         };
//
//         let dst = unsafe {
//             slice::from_raw_parts_mut(
//                 self.virt.start.add(self.per_hart_size * hartid).as_raw() as *mut u8,
//                 self.tls_template.file_size,
//             )
//         };
//
//         log::trace!(
//             "Copying tdata from {:?} to {:?}",
//             src.as_ptr_range(),
//             dst.as_ptr_range()
//         );
//
//         debug_assert_eq!(src.len(), dst.len());
//         unsafe {
//             ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
//         }
//     }
// }
//
fn map_kernel<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    page_alloc: &mut PageAllocator<A>,
    kernel: &Kernel,
    minfo: &MachineInfo,
) -> crate::Result<(Range<VirtualAddress>, Option<TlsAllocation>)>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    let mem_size = kernel.mem_size() as usize;
    let align = kernel.max_align() as usize;

    let virt_range = page_alloc.reserve_range(mem_size, align);
    log::trace!("kernel_virt {virt_range:?}");
    log::trace!("kernel_phys {:?}", kernel.elf_file.input.as_ptr_range());

    let virt_base = virt_range.start;

    let phys_base = PhysicalAddress::new(kernel.elf_file.input.as_ptr() as usize);
    assert!(
        phys_base.is_aligned(A::PAGE_SIZE),
        "Loaded ELF file is not sufficiently aligned"
    );

    // print the elf sections for debugging purposes
    kernel.debug_print_elf()?;

    let mut maybe_tls_allocation = None;

    // Load the segments into virtual memory.
    // Apply relocations in virtual memory.
    for ph in kernel.elf_file.program_iter() {
        match ph.get_type().map_err(Error::Elf)? {
            Type::Load => handle_load_segment(
                arch,
                frame_alloc,
                &ProgramHeader::try_from(ph)?,
                phys_base,
                virt_base,
            )?,
            Type::Tls => {
                let old = maybe_tls_allocation.replace(handle_tls_segment(
                    arch,
                    frame_alloc,
                    page_alloc,
                    &ProgramHeader::try_from(ph)?,
                    virt_base,
                    minfo,
                )?);
                assert!(old.is_none(), "multiple TLS segments not supported");
            }
            _ => {}
        }
    }

    // Apply relocations in virtual memory.
    for ph in kernel.elf_file.program_iter() {
        if ph.get_type().map_err(Error::Elf)? == Type::Dynamic {
            handle_dynamic_segment(
                &ProgramHeader::try_from(ph)?,
                &kernel.elf_file,
                phys_base,
                virt_base,
            )?;
        }
    }

    // Mark some memory regions as read-only after relocations have been
    // applied.
    for ph in kernel.elf_file.program_iter() {
        if ph.get_type().map_err(Error::Elf)? == Type::GnuRelro {
            handle_relro_segment(arch, &ProgramHeader::try_from(ph)?, virt_base)?;
        }
    }

    Ok((virt_range, maybe_tls_allocation))
}


fn handle_load_segment<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    ph: &ProgramHeader,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
) -> crate::Result<()>
where
    A: pmm::Arch,
{
    let flags = flags_for_segment(ph);

    log::debug!(
        "Handling Segment: LOAD off {offset:#016x} vaddr {vaddr:#016x} align {align} filesz {filesz:#016x} memsz {memsz:#016x} flags {flags:?}",
        offset = ph.offset,
        vaddr = ph.virtual_address,
        align = ph.align,
        filesz = ph.file_size,
        memsz = ph.mem_size
    );

    let phys = {
        let start = phys_base.add(ph.offset);
        let end = start.add(ph.file_size);

        start.align_down(ph.align)..end.align_up(ph.align)
    };

    let virt = {
        let start = virt_base.add(ph.virtual_address);
        let end = start.add(ph.file_size);

        start.align_down(ph.align)..end.align_up(ph.align)
    };

    log::trace!("mapping {virt:?} => {phys:?}");
    arch.map_contiguous(frame_alloc, virt, phys, flags)?;

    if ph.file_size < ph.mem_size {
        handle_bss_section(arch, frame_alloc, ph, flags, phys_base, virt_base)?;
    }

    Ok(())
}

fn handle_bss_section<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    ph: &ProgramHeader,
    flags: ArchFlags,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
) -> crate::Result<()>
where
    A: pmm::Arch,
{
    let virt_start = virt_base.add(ph.virtual_address);
    let zero_start = virt_start.add(ph.file_size);
    let zero_end = virt_start.add(ph.mem_size);

    let data_bytes_before_zero = zero_start.as_raw() & 0xfff;

    log::debug!(
        "handling BSS {:?}, data bytes before {data_bytes_before_zero}",
        zero_start..zero_end
    );

    if data_bytes_before_zero != 0 {
        let last_page = virt_start.add(ph.file_size - 1).align_down(ph.align);
        let last_frame = phys_base
            .add(ph.offset + ph.file_size - 1)
            .align_down(ph.align);

        let new_frame = allocate_and_copy(frame_alloc, last_frame, data_bytes_before_zero)?;

        log::debug!(
            "remapping {:?} to {:?}",
            last_page..last_page.add(ph.align),
            new_frame..new_frame.add(ph.align)
        );

        arch.remap_contiguous(last_page..last_page, new_frame..new_frame, flags)?;
    }

    let additional_virt = {
        let start = zero_start.align_up(ph.align).align_down(ph.align);
        let end = zero_end.align_up(ph.align);
        start..end
    };

    if !additional_virt.is_empty() {
        // additional_virt should be page-aligned, but just to make sure
        debug_assert!(additional_virt.is_aligned(ph.align));

        let additional_phys = frame_alloc.allocate_frames(additional_virt.size().div(ph.align));

        log::trace!("mapping additional zeros {additional_virt:?}...");
        arch.map(additional_virt, additional_phys, flags)?;
    }

    Ok(())
}

fn handle_tls_segment<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    page_alloc: &mut PageAllocator<A>,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
    minfo: &MachineInfo,
) -> crate::Result<TlsAllocation>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    let size_pages = ph.mem_size.div_ceil(A::PAGE_SIZE);
    let size = size_pages * A::PAGE_SIZE * minfo.cpus;

    let phys = frame_alloc.allocate_frames(size_pages * minfo.cpus);
    let virt = page_alloc.reserve_range(size, A::PAGE_SIZE);

    log::trace!("Mapping TLS region {virt:?}...");
    arch.map(virt.clone(), phys, ArchFlags::READ | ArchFlags::WRITE)?;

    let allocation = TlsAllocation {
        virt,
        per_hart_size: size,
        tls_template: TlsTemplate {
            start_addr: virt_base.add(ph.virtual_address),
            mem_size: ph.mem_size,
            file_size: ph.file_size,
        },
    };

    Ok(allocation)
}

fn handle_dynamic_segment(
    ph: &ProgramHeader,
    elf_file: &xmas_elf::ElfFile,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
) -> crate::Result<()> {
    log::trace!("parsing RELA info...");

    if let Some(rela_info) = ph.parse_rela(elf_file)? {
        let relas = unsafe {
            let ptr = phys_base.add(rela_info.offset as usize).as_raw()
                as *const xmas_elf::sections::Rela<P64>;

            slice::from_raw_parts(ptr, rela_info.count as usize)
        };

        log::trace!("applying relocations...");
        for rela in relas {
            apply_relocation(rela, phys_base, virt_base)?;
        }
    }

    Ok(())
}

fn apply_relocation(
    rela: &xmas_elf::sections::Rela<P64>,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
) -> crate::Result<()> {
    assert_eq!(
        rela.get_symbol_table_index(),
        0,
        "relocations using the symbol table are not supported"
    );

    const R_RISCV_RELATIVE: u32 = 3;

    match rela.get_type() {
        R_RISCV_RELATIVE => {
            // Calculate address at which to apply the relocation.
            // dynamic relocations offsets are relative to the virtual layout of the elf,
            // not the physical file
            let target = virt_base.add(rela.get_offset() as usize);

            // Calculate the value to store at the relocation target.
            let value = virt_base.offset(rela.get_addend() as isize);

            todo!()

            // log::trace!("reloc R_RISCV_RELATIVE offset: {:#x}; addend: {:#x} => target {target_phys:?} value {value:?}", rela.get_offset(), rela.get_addend());
            // unsafe { (target_phys.as_raw() as *mut usize).write_unaligned(value.as_raw()) };
        }
        _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
    }

    Ok(())
}

fn handle_relro_segment<A>(
    arch: &mut A,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
) -> crate::Result<()>
where
    A: pmm::Arch,
{
    let virt = {
        let start = virt_base.add(ph.virtual_address);

        start..start.add(ph.mem_size)
    };

    let virt_aligned = { virt.start.align_down(A::PAGE_SIZE)..virt.end.align_down(A::PAGE_SIZE) };

    log::debug!("Marking RELRO segment {virt_aligned:?} as read-only");
    arch.protect(virt_aligned, ArchFlags::READ)?;

    Ok(())
}

/// Map the kernel stacks for each hart.
// TODO add guard pages below each stack allocation
fn map_kernel_stacks<A>(
    arch: &mut A,
    frame_alloc: &mut BumpAllocator<A>,
    page_alloc: &mut PageAllocator<A>,
    machine_info: &MachineInfo,
    per_hart_stack_size: usize,
) -> crate::Result<Range<VirtualAddress>>
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    let stacks_phys = frame_alloc.allocate_frames(per_hart_stack_size * machine_info.cpus);

    let stacks_virt = page_alloc.reserve_range(
        per_hart_stack_size * A::PAGE_SIZE * machine_info.cpus,
        A::PAGE_SIZE,
    );

    log::trace!("Mapping stack region {stacks_virt:?}...");
    arch.map(
        stacks_virt.clone(),
        stacks_phys,
        ArchFlags::READ | ArchFlags::WRITE,
    )?;

    Ok(stacks_virt)
}

fn allocate_and_copy<A>(
    frame_alloc: &mut BumpAllocator<A>,
    src: PhysicalAddress,
    len: usize,
) -> crate::Result<PhysicalAddress>
where
    A: pmm::Arch,
{
    let frames = len.div_ceil(A::PAGE_SIZE);
    // FIXME use .map here instead
    let dst = frame_alloc.allocate_frames_contiguous(frames)?;

    unsafe {
        let src = slice::from_raw_parts_mut(src.as_raw() as *mut u8, len);

        let dst = slice::from_raw_parts_mut(dst.as_raw() as *mut u8, len);

        log::debug!("copy {len} bytes from {src:p} to {dst:p}");

        ptr::copy_nonoverlapping(src.as_mut_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(dst)
}

pub struct ProgramHeader<'a> {
    pub p_type: Type,
    pub p_flags: xmas_elf::program::Flags,
    pub align: usize,
    pub offset: usize,
    pub virtual_address: usize,
    pub file_size: usize,
    pub mem_size: usize,
    ph: xmas_elf::program::ProgramHeader<'a>,
}

impl ProgramHeader<'_> {
    pub fn parse_rela(&self, elf_file: &xmas_elf::ElfFile) -> crate::Result<Option<RelaInfo>> {
        let data = self.ph.get_data(elf_file).map_err(Error::Elf)?;
        let fields = match data {
            SegmentData::Dynamic32(_) => unimplemented!("32-bit elf files are not supported"),
            SegmentData::Dynamic64(fields) => fields,
            _ => return Ok(None),
        };

        let mut rela = None; // Address of Rela relocs
        let mut rela_size = None; // Total size of Rela relocs
        let mut rela_ent = None; // Size of one Rela reloc

        for field in fields {
            let tag = field.get_tag().map_err(Error::Elf)?;
            match tag {
                Tag::Rela => {
                    let ptr = field.get_ptr().map_err(Error::Elf)?;
                    let prev = rela.replace(ptr);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one Rela entry");
                    }
                }
                Tag::RelaSize => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_size.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaSize entry");
                    }
                }
                Tag::RelaEnt => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_ent.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaEnt entry");
                    }
                }

                Tag::Rel | Tag::RelSize | Tag::RelEnt => {
                    panic!("REL relocations are not supported")
                }
                Tag::RelrSize | Tag::Relr | Tag::RelrEnt => {
                    panic!("RELR relocations are not supported")
                }
                _ => {}
            }
        }

        if rela.is_none() && (rela_size.is_some() || rela_ent.is_some()) {
            panic!("Rela entry is missing but RelaSize or RelaEnt have been provided");
        }

        let Some(offset) = rela else {
            return Ok(None);
        };

        let total_size = rela_size.expect("RelaSize entry is missing");
        let entry_size = rela_ent.expect("RelaEnt entry is missing");

        Ok(Some(RelaInfo {
            offset,
            count: total_size / entry_size,
        }))
    }
}

struct RelaInfo {
    pub offset: u64,
    pub count: u64,
}

impl<'a> TryFrom<xmas_elf::program::ProgramHeader<'a>> for ProgramHeader<'a> {
    type Error = Error;

    fn try_from(ph: xmas_elf::program::ProgramHeader<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            p_type: ph.get_type().map_err(Error::Elf)?,
            p_flags: ph.flags(),
            align: usize::try_from(ph.align())?,
            offset: usize::try_from(ph.offset())?,
            virtual_address: usize::try_from(ph.virtual_addr())?,
            file_size: usize::try_from(ph.file_size())?,
            mem_size: usize::try_from(ph.mem_size())?,
            ph,
        })
    }
}

fn flags_for_segment(ph: &ProgramHeader) -> ArchFlags {
    let mut out = ArchFlags::empty();

    if ph.p_flags.is_read() {
        out |= ArchFlags::READ;
    }

    if ph.p_flags.is_write() {
        out |= ArchFlags::WRITE;
    }

    if ph.p_flags.is_execute() {
        out |= ArchFlags::EXECUTE;
    }

    assert!(
        !out.contains(ArchFlags::WRITE | ArchFlags::EXECUTE),
        "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
        ph.virtual_address,
        ph.virtual_address + ph.mem_size
    );

    out
}
