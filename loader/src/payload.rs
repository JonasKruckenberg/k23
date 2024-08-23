use core::slice;
// use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use crate::error::Error;
use crate::kconfig;
use kmm::{BumpAllocator, FrameAllocator, INIT};
use loader_api::LoaderConfig;
use object::{Object, ObjectSection};

// Include the generated payload.rs file which contains
// the payload binary and signature
include!(concat!(env!("OUT_DIR"), "/payload.rs"));

pub struct Payload<'a> {
    pub elf_file: object::read::elf::ElfFile64<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Payload<'a> {
    // pub fn from_signed_and_compressed(
    //     verifying_key: &'static [u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
    //     compressed_payload: &'a [u8],
    //     signature: &'static [u8; Signature::BYTE_SIZE],
    //     alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    // ) -> Self {
    //     log::info!("Verifying payload signature...");
    //     let verifying_key = VerifyingKey::from_bytes(verifying_key).unwrap();
    //     let signature = Signature::from_slice(signature).unwrap();
    //
    //     verifying_key
    //         .verify(compressed_payload, &signature)
    //         .expect("failed to verify kernel image signature");
    //
    //     Self::from_compressed(compressed_payload, alloc)
    // }

    pub fn from_compressed(
        compressed: &'a [u8],
        alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    ) -> crate::Result<Self> {
        log::info!("Decompressing payload...");
        let (uncompressed_size, input) =
            lz4_flex::block::uncompressed_size(compressed).map_err(Error::Decompression)?;

        let uncompressed_payload = unsafe {
            let frames = uncompressed_size.div_ceil(kconfig::PAGE_SIZE);
            let base = alloc.allocate_frames(frames)?;

            slice::from_raw_parts_mut(base.as_raw() as *mut u8, frames * kconfig::PAGE_SIZE)
        };

        lz4_flex::decompress_into(input, uncompressed_payload).map_err(Error::Decompression)?;

        Self::from_bytes(uncompressed_payload)
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
}
