use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::{arch, Error};
use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{ptr, slice};
use loader_api::TlsTemplate;
use pmm::frame_alloc::{FrameAllocator, NonContiguousFrames};
use pmm::{AddressSpace, Flush, PhysicalAddress, VirtualAddress, KIB, MIB};
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};
use xmas_elf::P64;

pub struct KernelAddressSpace {
    pub aspace: AddressSpace,

    /// The entry point address of the kernel
    entry: VirtualAddress,

    /// Memory region allocated for kernel TLS regions, as well as the template TLS to use for
    /// initializing them.
    pub maybe_tls_allocation: Option<TlsAllocation>,
    /// Memory region allocated for kernel stacks
    pub stacks_virt: Range<VirtualAddress>,
    /// The size of each stack in bytes
    per_hart_stack_size: usize,

    pub kernel_virt: Range<VirtualAddress>,
    pub heap_virt: Option<Range<VirtualAddress>>,
}

impl KernelAddressSpace {
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

    /// Active address space.
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
    pub unsafe fn activate(&self) {
        self.aspace.activate();
    }
}

/// Initialize the kernel address space, this will map the kernel ELF file into virtual memory,
/// and map stack & TLS regions for each hart.
pub fn init_kernel_aspace(
    mut aspace: AddressSpace,
    flush: &mut Flush,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    kernel: &Kernel,
    minfo: &MachineInfo,
) -> crate::Result<KernelAddressSpace> {
    let kernel_virt = page_alloc.allocate(
        Layout::from_size_align(kernel.mem_size() as usize, kernel.max_align() as usize).unwrap(),
    );

    let maybe_tls_allocation = map_elf(
        &mut aspace,
        frame_alloc,
        page_alloc,
        &kernel.elf_file,
        minfo,
        kernel_virt.start,
        flush,
    )?;

    // Map stacks for kernel
    let per_hart_stack_size_pages = usize::try_from(kernel.loader_config.kernel_stack_size_pages)?;
    let stacks_virt = map_kernel_stacks(
        &mut aspace,
        frame_alloc,
        page_alloc,
        minfo,
        per_hart_stack_size_pages,
        flush,
    )?;

    let heap_virt = if let Some(heap_size_pages) = kernel.loader_config.kernel_heap_size_pages {
        let heap_size_pages = usize::try_from(heap_size_pages)?;

        let heap_virt =
            map_kernel_heap(&mut aspace, frame_alloc, page_alloc, heap_size_pages, flush)?;

        Some(heap_virt)
    } else {
        None
    };

    let frame_usage = frame_alloc.frame_usage();
    log::debug!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * arch::PAGE_SIZE) / KIB,
        (frame_usage.total * arch::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    Ok(KernelAddressSpace {
        aspace,
        entry: kernel_virt
            .start
            .add(usize::try_from(kernel.elf_file.header.pt2.entry_point())?),
        maybe_tls_allocation,
        stacks_virt,
        per_hart_stack_size: per_hart_stack_size_pages * arch::PAGE_SIZE,
        kernel_virt,
        heap_virt,
    })
}

/// Map an ELF file into virtual memory at the given `virt_base` offset.
fn map_elf(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    elf_file: &xmas_elf::ElfFile,
    minfo: &MachineInfo,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<Option<TlsAllocation>> {
    let phys_base = PhysicalAddress::new(elf_file.input.as_ptr() as usize);
    assert!(
        phys_base.is_aligned(arch::PAGE_SIZE),
        "Loaded ELF file is not sufficiently aligned"
    );

    let mut maybe_tls_allocation = None;

    // Load the segments into virtual memory.
    for ph in elf_file.program_iter() {
        match ph.get_type().unwrap() {
            Type::Load => handle_load_segment(
                aspace,
                frame_alloc,
                &ProgramHeader::try_from(ph)?,
                phys_base,
                virt_base,
                flush,
            )?,
            Type::Tls => {
                let old = maybe_tls_allocation.replace(handle_tls_segment(
                    aspace,
                    frame_alloc,
                    page_alloc,
                    &ProgramHeader::try_from(ph)?,
                    virt_base,
                    minfo,
                    flush,
                )?);
                assert!(old.is_none(), "multiple TLS segments not supported");
            }
            _ => {}
        }
    }

    // Apply relocations in virtual memory.
    for ph in elf_file.program_iter() {
        if ph.get_type().unwrap() == Type::Dynamic {
            handle_dynamic_segment(
                &ProgramHeader::try_from(ph).unwrap(),
                &elf_file,
                phys_base,
                virt_base,
            )?;
        }
    }

    // Mark some memory regions as read-only after relocations have been
    // applied.
    for ph in elf_file.program_iter() {
        if ph.get_type().unwrap() == Type::GnuRelro {
            handle_relro_segment(
                aspace,
                &ProgramHeader::try_from(ph).unwrap(),
                virt_base,
                flush,
            )?;
        }
    }

    Ok(maybe_tls_allocation)
}

/// Map an ELF LOAD segment.
fn handle_load_segment(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    ph: &ProgramHeader,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
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
    aspace.map_contiguous(
        frame_alloc,
        virt.start,
        phys.start,
        NonZeroUsize::new(phys.end.as_raw() - phys.start.as_raw()).unwrap(),
        flags,
        flush,
    )?;

    if ph.file_size < ph.mem_size {
        handle_bss_section(aspace, frame_alloc, ph, flags, phys_base, virt_base, flush)?;
    }

    Ok(())
}

/// BSS sections are special, since they take up virtual memory that is not present in the "physical" elf file.
///
/// Usually, this means just allocating zeroed frames and mapping them "in between" the pages
/// backed by the elf file. However, quite often the boundary between DATA and BSS sections is
/// *not* page aligned (since that would unnecessarily bloat the elf file) which means for us
/// that we need special handling for the last DATA page that is only partially filled with data
/// and partially filled with zeroes. Here's how we do this:
///
/// 1. We calculate the size of the segments zero initialized part.
/// 2. We then figure out whether the boundary is page-aligned or if there are DATA bytes we need to account for.
///     2.1. IF there are data bytes to account for, we allocate a zeroed frame,
///     2.2. we then copy over the relevant data from the DATA section into the new frame
///     2.3. and lastly replace last page previously mapped by `handle_load_segment` to stitch things up.
/// 3. If the BSS section is larger than that one page, we allocate additional zeroed frames and map them in.
fn handle_bss_section(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    ph: &ProgramHeader,
    flags: pmm::Flags,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
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

        let new_frame = frame_alloc
            .allocate_contiguous_zeroed(
                Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
            )
            .ok_or(pmm::Error::OutOfMemory)?;

        unsafe {
            let src = slice::from_raw_parts(
                aspace.phys_to_virt(last_frame).as_raw() as *mut u8,
                data_bytes_before_zero,
            );

            let dst = slice::from_raw_parts_mut(
                aspace.phys_to_virt(new_frame).as_raw() as *mut u8,
                data_bytes_before_zero,
            );

            log::debug!("copying {data_bytes_before_zero} bytes from {src:p} to {dst:p}...");
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }

        aspace.remap_contiguous(
            last_page,
            new_frame,
            NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
            flush,
        )?;
    }

    log::trace!("zero_start {zero_start:?} zero_end {zero_end:?}");
    let (additional_virt_base, additional_len) = {
        // zero_start either lies at a page boundary OR somewhere within the first page
        // by aligning up, we move it to the beginning of the *next* page.
        let start = zero_start.align_up(ph.align);
        let end = zero_end.align_up(ph.align);
        (start, end.as_raw() - start.as_raw())
    };

    if additional_len > 0 {
        let additional_phys = NonContiguousFrames::new_zeroed(
            frame_alloc,
            Layout::from_size_align(additional_len, arch::PAGE_SIZE).unwrap(),
            aspace.physical_memory_offset(),
        );

        log::trace!(
            "mapping additional zeros {additional_virt_base:?}..{:?}",
            additional_virt_base.add(additional_len)
        );
        aspace.map(additional_virt_base, additional_phys, flags, flush)?;
    }

    Ok(())
}

/// Map the kernel thread-local storage (TLS) memory regions.
fn handle_tls_segment(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
    minfo: &MachineInfo,
    flush: &mut Flush,
) -> crate::Result<TlsAllocation> {
    let per_hart_size_pages = ph.mem_size.div_ceil(arch::PAGE_SIZE);
    let layout = Layout::from_size_align(
        per_hart_size_pages * arch::PAGE_SIZE * minfo.cpus,
        arch::PAGE_SIZE,
    )
    .unwrap();

    let phys =
        NonContiguousFrames::new_zeroed(frame_alloc, layout, aspace.physical_memory_offset());
    let virt = page_alloc.allocate(layout);

    log::trace!("Mapping TLS region {virt:?}...");
    aspace.map(
        virt.start,
        phys,
        pmm::Flags::READ | pmm::Flags::WRITE,
        flush,
    )?;

    Ok(TlsAllocation {
        virt,
        per_hart_size: per_hart_size_pages,
        tls_template: TlsTemplate {
            start_addr: virt_base.add(ph.virtual_address),
            mem_size: ph.mem_size,
            file_size: ph.file_size,
        },
    })
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

        // TODO memory fence here

        log::trace!("applying relocations in virtual memory...");
        for rela in relas {
            apply_relocation(rela, virt_base)?;
        }
    }

    Ok(())
}

fn apply_relocation(
    rela: &xmas_elf::sections::Rela<P64>,
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

            // log::trace!("reloc R_RISCV_RELATIVE offset: {:#x}; addend: {:#x} => target {target:?} value {value:?}", rela.get_offset(), rela.get_addend());
            unsafe {
                (target.as_raw() as *mut usize).write_unaligned(value.as_raw());
            }
        }
        _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
    }

    Ok(())
}

fn handle_relro_segment(
    aspace: &mut AddressSpace,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
    let virt = {
        let start = virt_base.add(ph.virtual_address);

        start..start.add(ph.mem_size)
    };

    let virt_aligned =
        { virt.start.align_down(arch::PAGE_SIZE)..virt.end.align_down(arch::PAGE_SIZE) };

    log::debug!("Marking RELRO segment {virt_aligned:?} as read-only");
    aspace.protect(
        virt_aligned.start,
        NonZeroUsize::new(virt_aligned.end.as_raw() - virt_aligned.start.as_raw()).unwrap(),
        pmm::Flags::READ,
        flush,
    )?;

    Ok(())
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

/// Map the kernel stacks for each hart.
// TODO add guard pages below each stack allocation
fn map_kernel_stacks(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    machine_info: &MachineInfo,
    per_hart_stack_size_pages: usize,
    flush: &mut Flush,
) -> crate::Result<Range<VirtualAddress>> {
    let layout = Layout::from_size_align(
        per_hart_stack_size_pages * arch::PAGE_SIZE * machine_info.cpus,
        arch::PAGE_SIZE,
    )
    .unwrap();

    // The stacks region doesn't need to be zeroed, since we will be filling it with
    // the canary pattern anyway
    let phys = NonContiguousFrames::new(frame_alloc, layout);
    let virt = page_alloc.allocate(layout);

    log::trace!("Mapping stack region {virt:?}...");
    aspace.map(
        virt.start,
        phys,
        pmm::Flags::READ | pmm::Flags::WRITE,
        flush,
    )?;

    Ok(virt)
}

/// Allocate and map the kernel heap.
fn map_kernel_heap(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    heap_size_pages: usize,
    flush: &mut Flush,
) -> crate::Result<Range<VirtualAddress>> {
    let layout = Layout::from_size_align(
        heap_size_pages * arch::PAGE_SIZE,
        pmm::arch::page_size_for_level(1),
    )
    .unwrap();

    // Since the kernel heap region is likely quite large and should only be exposed through Rusts
    // allocator APIs, we don't zero it here. Instead, it should be zeroed on demand by the allocator.
    let phys = NonContiguousFrames::new(frame_alloc, layout);
    let virt = page_alloc.allocate(layout);

    log::trace!("Mapping heap region {virt:?}...");
    aspace.map(
        virt.start,
        phys,
        pmm::Flags::READ | pmm::Flags::WRITE,
        flush,
    )?;

    Ok(virt)
}

struct ProgramHeader<'a> {
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

fn flags_for_segment(ph: &ProgramHeader) -> pmm::Flags {
    let mut out = pmm::Flags::empty();

    if ph.p_flags.is_read() {
        out |= pmm::Flags::READ;
    }

    if ph.p_flags.is_write() {
        out |= pmm::Flags::WRITE;
    }

    if ph.p_flags.is_execute() {
        out |= pmm::Flags::EXECUTE;
    }

    assert!(
        !out.contains(pmm::Flags::WRITE | pmm::Flags::EXECUTE),
        "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
        ph.virtual_address,
        ph.virtual_address + ph.mem_size
    );

    out
}
