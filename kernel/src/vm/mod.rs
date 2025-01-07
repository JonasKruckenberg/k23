use crate::ensure;
use crate::error::Error;
use crate::frame_alloc::{Frame, FrameAllocator};
use crate::vm::address_space::AddressSpace;
use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::alloc::Layout;
use core::fmt::Formatter;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::range::Range;
use core::{fmt, mem, slice};
use loader_api::BootInfo;
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use pin_project::pin_project;
use xmas_elf::program::Type;

mod address_space;
mod address_space_region;

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct PageFaultFlags: u8 {
        /// The fault was caused by a memory read
        const READ = 1 << 0;
        /// The fault was caused by a memory write
        const WRITE = 1 << 1;
        /// The fault was caused by an instruction fetch
        const INSTRUCTION = 1 << 3;
    }
}

impl PageFaultFlags {
    pub fn is_valid(&self) -> bool {
        self.contains(PageFaultFlags::READ) != self.contains(PageFaultFlags::WRITE)
    }

    pub fn cause_is_read(&self) -> bool {
        self.contains(PageFaultFlags::READ)
    }
    pub fn cause_is_write(&self) -> bool {
        self.contains(PageFaultFlags::WRITE)
    }
    pub fn cause_is_instr_fetch(&self) -> bool {
        self.contains(PageFaultFlags::INSTRUCTION)
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Permissions: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

impl From<PageFaultFlags> for Permissions {
    fn from(value: PageFaultFlags) -> Self {
        let mut out = Permissions::empty();
        if value.contains(PageFaultFlags::WRITE) {
            out |= Permissions::WRITE;
        } else {
            out |= Permissions::READ;
        }
        if value.contains(PageFaultFlags::INSTRUCTION) {
            out |= Permissions::EXECUTE;
        }
        out
    }
}

impl From<Permissions> for mmu::Flags {
    fn from(value: Permissions) -> Self {
        let mut out = mmu::Flags::empty();
        out.set(mmu::Flags::READ, value.contains(Permissions::READ));
        out.set(mmu::Flags::WRITE, value.contains(Permissions::WRITE));
        out.set(mmu::Flags::EXECUTE, value.contains(Permissions::EXECUTE));
        out
    }
}

pub fn test(boot_info: &BootInfo) -> crate::Result<()> {
    let mut aspace = AddressSpace::new_kernel(None);
    reserve_wired_regions(&mut aspace, boot_info)?;

    for region in aspace.regions.iter() {
        log::trace!(
            "{:<40} {}..{} {}",
            region.name,
            region.range.start,
            region.range.end,
            region.permissions
        )
    }

    let layout = Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap();
    let vmo = Arc::new(Vmo::Paged(PagedVmo {
        pages: PageList::zeroed_with_capacity(layout.size()),
    }));
    log::trace!("{vmo:?}");

    let range = aspace
        .map(layout, vmo, 0, Permissions::READ, "Test".to_string())?
        .range;

    aspace
        .page_fault(range.start.checked_add(3 * PAGE_SIZE).unwrap(), PageFaultFlags::WRITE)
        .unwrap();

    Ok(())
}

fn reserve_wired_regions(aspace: &mut AddressSpace, boot_info: &BootInfo) -> crate::Result<()> {
    // reserve the physical memory map
    aspace.reserve(
        boot_info.physical_memory_map,
        Permissions::READ | Permissions::WRITE,
        "Physical Memory Map".to_string(),
    )?;

    let own_elf = unsafe {
        let base = VirtualAddress::from_phys(
            boot_info.kernel_elf.start,
            boot_info.physical_address_offset,
        )
        .unwrap();

        slice::from_raw_parts(base.as_ptr(), boot_info.kernel_elf.size())
    };
    let own_elf = xmas_elf::ElfFile::new(own_elf).unwrap();

    for ph in own_elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load {
            continue;
        }

        let virt = boot_info
            .kernel_virt
            .start
            .checked_add(ph.virtual_addr() as usize)
            .unwrap();

        let mut permissions = Permissions::empty();
        if ph.flags().is_read() {
            permissions |= Permissions::READ;
        }
        if ph.flags().is_write() {
            permissions |= Permissions::WRITE;
        }
        if ph.flags().is_execute() {
            permissions |= Permissions::EXECUTE;
        }

        assert!(
            !permissions.contains(Permissions::WRITE | Permissions::EXECUTE),
            "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
            ph.virtual_addr(),
            ph.virtual_addr() + ph.mem_size()
        );

        aspace.reserve(
            Range {
                start: virt.align_down(PAGE_SIZE),
                end: virt
                    .checked_add(ph.mem_size() as usize)
                    .unwrap()
                    .checked_align_up(PAGE_SIZE)
                    .unwrap(),
            },
            permissions,
            format!("Kernel {permissions} Segment"),
        )?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum Vmo {
    Wired(WiredVmo),
    Paged(PagedVmo),
}

#[derive(Debug)]
pub struct WiredVmo {
    range: Range<PhysicalAddress>,
}

impl WiredVmo {
    fn lookup_contiguous(&self, range: Range<usize>) -> crate::Result<Range<PhysicalAddress>> {
        ensure!(
            range.start % PAGE_SIZE == 0,
            Error::InvalidArgument,
            "range is not PAGE_SIZE aligned"
        );
        let start = self.range.start.checked_add(range.start).unwrap();
        let end = self.range.start.checked_add(range.end).unwrap();

        ensure!(
            self.range.start <= start && self.range.end >= end,
            Error::AccessDenied,
            "requested range is out of bounds"
        );

        Ok(Range::from(start..end))
    }
}

#[derive(Debug)]
pub struct PagedVmo {
    pages: PageList, // TODO WAVLTree of frames sorted by their phys address
}

impl PagedVmo {
    fn require_owned_page(&self, offset: usize) -> crate::Result<&Frame> {
        if let Some(page) = self.pages.lookup(offset) {
            match page {
                Page::Frame(_frame) => {
                    // -> IF FRAME IS OWNED BY VMO (how do we figure out the frame owner?)
                    //     -> Return frame
                    // -> IF DIFFERENT OWNER
                    //     -> do copy on write
                    //         -> allocate new frame
                    //         -> copy from frame to new frame
                    //         -> replace frame
                    //         -> drop old frame clone (should free if refcount == 1)
                    //         -> return new frame
                    todo!()
                }
                Page::Zero => {
                    todo!("clone the zero frame")
                }
            }
        } else {
            log::debug!("TODO request bytes from source (later when we actually have sources)");
            Err(Error::AccessDenied)
        }
    }

    pub fn require_read_page(&self, offset: usize) -> crate::Result<&Frame> {
        if let Some(page) = self.pages.lookup(offset) {
            match page {
                Page::Frame(frame) => Ok(unsafe { frame.as_ref() }),
                Page::Zero => {
                    todo!("clone the zero frame")
                }
            }
        } else {
            log::debug!("TODO request bytes from source (later when we actually have sources)");
            Err(Error::AccessDenied)
        }
    }
}

pub struct Batch {
    // mmu: Arc<Mutex<mmu::AddressSpace>>,
    range: Range<VirtualAddress>,
    flags: mmu::Flags,
    phys: Vec<(PhysicalAddress, usize)>,
}

impl Drop for Batch {
    fn drop(&mut self) {
        if !self.phys.is_empty() {
            log::error!("batch was not flushed before dropping");
            // panic_unwind::panic_in_drop!("batch was not flushed before dropping");
        }
    }
}

impl Batch {
    pub fn new() -> Self {
        Self {
            range: Default::default(),
            flags: mmu::Flags::empty(),
            phys: vec![],
        }
    }

    pub fn append(
        &mut self,
        base: VirtualAddress,
        phys: (PhysicalAddress, usize),
        flags: mmu::Flags,
    ) -> crate::Result<()> {
        log::trace!("appending {phys:?} at {base:?} with flags {flags:?}");
        if !self.can_append(base) || self.flags != flags {
            self.flush()?;
            self.flags = flags;
            self.range = Range::from(base..base.checked_add(phys.1).unwrap());
        } else {
            self.range.end = self.range.end.checked_add(phys.1).unwrap();
        }

        self.phys.push(phys);

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        log::trace!("flushing batch {:?} {:?}...", self.range, self.phys);
        if self.phys.is_empty() {
            return Ok(());
        }

        log::trace!(
            "materializing changes to MMU {:?} {:?} {:?}",
            self.range,
            self.phys,
            self.flags
        );
        // let mut mmu = self.mmu.lock();
        // let mut flush = Flush::empty(mmu.asid());
        // let iter = BatchFramesIter {
        //     iter: self.phys.drain(..),
        // };
        // mmu.map(self.range.start, iter, self.flags, &mut flush)?;
        todo!();

        self.range = Range::from(self.range.end..self.range.end);

        Ok(())
    }

    pub fn ignore(&mut self) {
        self.phys.clear();
    }

    fn can_append(&self, virt: VirtualAddress) -> bool {
        self.range.end == virt
    }
}

struct BatchFramesIter<'a> {
    iter: vec::Drain<'a, (PhysicalAddress, usize)>,
    alloc: &'static FrameAllocator,
}
impl mmu::frame_alloc::FramesIterator for BatchFramesIter<'_> {
    fn alloc_mut(&mut self) -> &mut dyn mmu::frame_alloc::FrameAllocator {
        &mut self.alloc
    }
}
impl Iterator for BatchFramesIter<'_> {
    type Item = (PhysicalAddress, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

const FANOUT: usize = 16;

fn offset_to_node_offset(offset: usize) -> usize {
    (offset) & 0usize.wrapping_sub(PAGE_SIZE * FANOUT)
}

fn offset_to_node_index(offset: usize) -> usize {
    (offset >> mmu::arch::PAGE_SHIFT) % FANOUT
}

#[derive(Default)]
struct PageList {
    nodes: wavltree::WAVLTree<PageListNode>,
}

impl Drop for PageList {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for PageList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageList")
            .field_with("nodes", |f| {
                let mut f = f.debug_list();
                self.nodes.iter().for_each(|node| {
                    f.entry(node);
                });
                f.finish()
            })
            .finish()
    }
}

impl PageList {
    pub fn zeroed_with_capacity(capacity: usize) -> Self {
        let mut nodes: wavltree::WAVLTree<PageListNode> = wavltree::WAVLTree::new();

        let mut offset = 0;
        while offset < capacity {
            let node = nodes
                .entry(&offset_to_node_offset(offset))
                .or_insert_with(|| {
                    Box::pin(PageListNode {
                        links: Default::default(),
                        offset,
                        pages: [const { None }; FANOUT],
                    })
                });

            node.project().pages[offset_to_node_index(offset)] = Some(Page::Zero);

            offset += PAGE_SIZE;
        }

        Self { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn lookup(&self, offset: usize) -> Option<&Page> {
        let node_offset = offset_to_node_offset(offset);
        let node = self.nodes.find(&node_offset).get()?;

        let page = node.pages.get(offset_to_node_index(offset))?;
        page.as_ref()
    }

    pub fn lookup_mut(&mut self, offset: usize) -> Option<Pin<&mut Page>> {
        let node_offset = offset_to_node_offset(offset);
        let node =
            unsafe { Pin::into_inner_unchecked(self.nodes.find_mut(&node_offset).get_mut()?) };

        let page = node.pages.get_mut(offset_to_node_index(offset))?;
        page.as_mut()
            .map(|page| unsafe { Pin::new_unchecked(page) })
    }

    #[must_use]
    pub fn replace(&mut self, offset: usize, new_frame: NonNull<Frame>) -> Option<Page> {
        let node_offset = offset_to_node_offset(offset);
        let mut node = self.nodes.find_mut(&node_offset).get_mut()?;

        node.pages
            .get_mut(offset_to_node_index(offset))?
            .replace(Page::Frame(new_frame))
    }

    #[must_use]
    pub fn remove(&mut self, offset: usize) -> Option<Page> {
        let node_offset = offset_to_node_offset(offset);
        let mut node = self.nodes.find_mut(&node_offset).get_mut()?;

        mem::take(node.pages.get_mut(offset_to_node_index(offset))?)
    }

    pub fn clear(&mut self) {
        // TODO return frames to freelist
        todo!()
    }
}

#[derive(Debug)]
enum Page {
    Zero,
    Frame(NonNull<Frame>),
}

#[pin_project]
#[derive(Debug)]
struct PageListNode {
    links: wavltree::Links<PageListNode>,
    offset: usize,
    pages: [Option<Page>; FANOUT],
}

unsafe impl wavltree::Linked for PageListNode {
    type Handle = Pin<Box<PageListNode>>;
    type Key = usize;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.offset
    }
}
