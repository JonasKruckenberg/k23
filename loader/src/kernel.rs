use core::slice;
// use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use crate::error::Error;
use crate::kconfig;
use kmm::{BumpAllocator, FrameAllocator};
use loader_api::LoaderConfig;
use object::elf::{ProgramHeader64, PT_LOAD};
use object::read::elf::ProgramHeader;
use object::{Endianness, Object, ObjectSection};

// Include the generated kernel.rs file which contains
// the kernel binary and signature
include!(concat!(env!("OUT_DIR"), "/kernel.rs"));

pub struct Kernel<'a> {
    pub elf_file: object::read::elf::ElfFile64<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Kernel<'a> {
    // pub fn from_signed_and_compressed(
    //     verifying_key: &'static [u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
    //     compressed_kernel: &'a [u8],
    //     signature: &'static [u8; Signature::BYTE_SIZE],
    //     alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    // ) -> Self {
    //     log::info!("Verifying kernel signature...");
    //     let verifying_key = VerifyingKey::from_bytes(verifying_key).unwrap();
    //     let signature = Signature::from_slice(signature).unwrap();
    //
    //     verifying_key
    //         .verify(compressed_kernel, &signature)
    //         .expect("failed to verify kernel image signature");
    //
    //     Self::from_compressed(compressed_kernel, alloc)
    // }

    pub fn from_compressed(
        compressed: &'a [u8],
        alloc: &mut BumpAllocator<'_, kconfig::MEMORY_MODE>,
    ) -> crate::Result<Self> {
        log::info!("Decompressing kernel...");
        let (uncompressed_size, input) =
            lz4_flex::block::uncompressed_size(compressed).map_err(Error::Decompression)?;

        let uncompressed_kernel = unsafe {
            let frames = uncompressed_size.div_ceil(kconfig::PAGE_SIZE);
            let base = alloc.allocate_frames(frames)?;

            slice::from_raw_parts_mut(base.as_raw() as *mut u8, frames * kconfig::PAGE_SIZE)
        };

        lz4_flex::decompress_into(input, uncompressed_kernel).map_err(Error::Decompression)?;

        Self::from_bytes(uncompressed_kernel)
    }

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
