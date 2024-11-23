use loader_api::LoaderConfig;
use object::elf::{ProgramHeader64, PT_LOAD};
use object::read::elf::ProgramHeader;
use object::{Endianness, Object, ObjectSection};

/// The inlined kernel
pub static KERNEL_BYTES: KernelBytes = KernelBytes(*include_bytes!(env!("KERNEL")));

/// Wrapper type for the inlined bytes to ensure proper alignment
#[repr(C, align(4096))]
pub struct KernelBytes(pub [u8; include_bytes!(env!("KERNEL")).len()]);

/// The decompressed and parsed kernel ELF plus the embedded loader configuration data
pub struct Kernel<'a> {
    pub elf_file: object::read::elf::ElfFile64<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Kernel<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> crate::Result<Self> {
        let elf_file = object::read::elf::ElfFile::parse(bytes)?;

        let loader_config = unsafe {
            let section = elf_file
                .section_by_name(".loader_config")
                .expect("missing .loader_config section");
            let raw = section.data()?;

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
        use object::Endianness;

        let max_addr = self
            .loadable_program_headers()
            .map(|ph| ph.p_vaddr(Endianness::default()) + ph.p_memsz(Endianness::default()))
            .max()
            .unwrap_or(0);

        let min_addr = self
            .loadable_program_headers()
            .map(|ph| ph.p_vaddr(Endianness::default()))
            .min()
            .unwrap_or(0);

        max_addr - min_addr
    }

    /// Returns the largest alignment of any loadable segment in the kernel and by extension
    /// the overall alignment for the kernel.
    pub fn max_align(&self) -> u64 {
        let load_program_headers = self.loadable_program_headers();

        load_program_headers
            .map(|ph| ph.p_align(Endianness::default()))
            .max()
            .unwrap_or(1)
    }

    fn loadable_program_headers(&self) -> impl Iterator<Item = &ProgramHeader64<Endianness>> + '_ {
        self.elf_file
            .elf_program_headers()
            .iter()
            .filter(|ph| ph.p_type(Endianness::default()) == PT_LOAD)
    }
}
