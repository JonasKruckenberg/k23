use crate::arch::EntryFlags;
use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use crate::runtime::codegen::{
    FunctionLoc, ELF_K23_INFO, ELF_K23_TRAPS, ELF_TEXT, ELF_WASM_DATA, ELF_WASM_DWARF,
    ELF_WASM_NAMES,
};
use crate::runtime::guest_memory::AlignedVec;
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;
use object::{File, Object, ObjectSection};
use vmm::{AddressRangeExt, Flush, Mapper, VirtualAddress};

pub struct CodeMemory {
    inner: AlignedVec<u8, { kconfig::PAGE_SIZE }>,
    published: bool,

    text: Range<VirtualAddress>,
    wasm_data: Range<VirtualAddress>,
    func_name_data: Range<VirtualAddress>,
    trap_data: Range<VirtualAddress>,
    dwarf: Range<VirtualAddress>,
    info: Range<VirtualAddress>,
}

impl CodeMemory {
    pub fn new(vec: AlignedVec<u8, { kconfig::PAGE_SIZE }>) -> Self {
        let obj = File::parse(vec.as_slice()).expect("failed to parse compilation artifact");

        let mut text = None;
        let mut wasm_data = Range::default();
        let mut func_name_data = Range::default();
        let mut trap_data = Range::default();
        let mut dwarf = Range::default();
        let mut info = Range::default();

        for section in obj.sections() {
            let name = section.name().unwrap();
            let range = unsafe {
                let range = section.data().unwrap().as_ptr_range();

                VirtualAddress::new(range.start as usize)..VirtualAddress::new(range.end as usize)
            };

            // Double-check that sections are all aligned properly.
            if section.align() != 0 && range.size() != 0 {
                debug_assert!(
                    range.is_aligned(usize::try_from(section.align()).unwrap()),
                    "section `{}` isn't aligned to {:#x} ({range:?})",
                    section.name().unwrap_or("ERROR"),
                    section.align(),
                );
            }

            match name {
                ELF_TEXT => {
                    debug_assert!(
                        range.is_aligned(kconfig::PAGE_SIZE),
                        "text section isn't aligned to PAGE_SIZE"
                    );

                    text = Some(range);
                }
                ELF_WASM_DATA => wasm_data = range,
                ELF_WASM_NAMES => func_name_data = range,
                ELF_WASM_DWARF => dwarf = range,

                ELF_K23_TRAPS => trap_data = range,
                ELF_K23_INFO => info = range,
                _ => {}
            }
        }

        Self {
            inner: vec,
            published: false,

            text: text.expect("object file had no text section"),
            wasm_data,
            func_name_data,
            trap_data,
            dwarf,
            info,
        }
    }

    pub fn publish(&mut self) -> Result<(), vmm::Error> {
        debug_assert!(!self.published);
        self.published = true;

        if self.inner.is_empty() {
            return Ok(());
        }

        let alloc = self.inner.allocator();

        with_frame_alloc(|frame_alloc| -> Result<(), vmm::Error> {
            let mut mapper: Mapper<kconfig::MEMORY_MODE> =
                Mapper::from_address(alloc.asid(), alloc.root_table(), frame_alloc);
            let mut flush = Flush::empty(alloc.asid());

            mapper.set_flags_for_range(
                self.text.clone(),
                EntryFlags::READ | EntryFlags::EXECUTE,
                &mut flush,
            )?;

            flush.flush()?;
            Ok(())
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        self.inner.as_slice()
    }

    pub fn resolve_function_loc(&self, func_loc: FunctionLoc) -> VirtualAddress {
        let addr = self.text.start.add(func_loc.start as usize);

        log::trace!(
            "resolve_function_loc {func_loc:?}, text {:?} => {:?}",
            self.text,
            addr
        );

        // Assert the function location actually lies in our text section
        debug_assert!(
            self.text.start <= addr && self.text.end > addr.add(func_loc.length as usize)
        );

        addr
    }
}

impl fmt::Debug for CodeMemory {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeMemory")
            .field("inner", &self.inner.as_ptr_range())
            .field("published", &self.published)
            .field("text", &self.text)
            .field("wasm_data", &self.wasm_data)
            .field("func_name_data", &self.func_name_data)
            .field("trap_data", &self.trap_data)
            .field("dwarf", &self.dwarf)
            .field("info", &self.info)
            .finish()
    }
}
