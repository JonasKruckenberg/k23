use xmas_elf::program::{ProgramHeader, Type};
use loader_api::LoaderConfig;
use crate::error::Error;

/// The inlined kernel
pub static KERNEL_BYTES: KernelBytes = KernelBytes(*include_bytes!(env!("KERNEL")));

/// Wrapper type for the inlined bytes to ensure proper alignment
#[repr(C, align(4096))]
pub struct KernelBytes(pub [u8; include_bytes!(env!("KERNEL")).len()]);

/// The decompressed and parsed kernel ELF plus the embedded loader configuration data
pub struct Kernel<'a> {
    pub elf_file: xmas_elf::ElfFile<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Kernel<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> crate::Result<Self> {
        let elf_file = xmas_elf::ElfFile::new(bytes).map_err(Error::Elf)?;

        let loader_config = unsafe {
            let section = elf_file
                .find_section_by_name(".loader_config")
                .expect("missing .loader_config section");
            let raw = section.raw_data(&elf_file);

            let ptr: *const LoaderConfig = raw.as_ptr().cast();
            let cfg = &*ptr;

            cfg.assert_valid();
            cfg
        };

        Ok(Self {
            elf_file,
            loader_config,
        })
    }

    /// Returns the size of the kernel in memory.
    pub fn mem_size(&self) -> u64 {
        let max_addr = self
            .loadable_program_headers()
            .map(|ph| ph.virtual_addr() + ph.mem_size())
            .max()
            .unwrap_or(0);

        let min_addr = self
            .loadable_program_headers()
            .map(|ph| ph.virtual_addr())
            .min()
            .unwrap_or(0);

        max_addr - min_addr
    }

    /// Returns the largest alignment of any loadable segment in the kernel and by extension
    /// the overall alignment for the kernel.
    pub fn max_align(&self) -> u64 {
        let load_program_headers = self.loadable_program_headers();

        load_program_headers
            .map(|ph| ph.align())
            .max()
            .unwrap_or(1)
    }

    fn loadable_program_headers(&self) -> impl Iterator<Item = ProgramHeader> + '_ {
        self.elf_file
            .program_iter()
            .filter(|ph| ph.get_type().unwrap() == Type::Load)
    }

    pub fn debug_print_elf(&self) -> crate::Result<()> {
        log::trace!("Idx Name              Offset   Vaddr            Filesz   Memsz");
        for (idx, sec) in self.elf_file.section_iter().enumerate() {
            log::trace!(
                "{idx:>3} {name:<17} {offset:#08x} {vaddr:#016x} {filesz:#08x} {memsz:#08x}",
                name = sec.get_name(&self.elf_file).unwrap_or(""),
                offset = sec.offset(),
                vaddr = sec.address(),
                filesz = sec.entry_size(),
                memsz = sec.size(),
            );
        }
        Ok(())
    }
}
