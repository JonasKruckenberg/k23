use crate::kconfig;
use core::slice;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use loader_api::LoaderConfig;
use object::{Object, ObjectSection};
use vmm::{BumpAllocator, FrameAllocator, INIT};

pub struct Payload<'a> {
    pub elf_file: object::read::elf::ElfFile64<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Payload<'a> {
    pub fn from_signed_and_compressed(
        verifying_key: &'static [u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
        compressed_payload: &'a [u8],
        signature: &'static [u8; Signature::BYTE_SIZE],
        alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    ) -> Self {
        log::info!("Verifying payload signature...");
        let verifying_key = VerifyingKey::from_bytes(verifying_key).unwrap();
        let signature = Signature::from_slice(signature).unwrap();

        verifying_key
            .verify(compressed_payload, &signature)
            .expect("failed to verify kernel image signature");

        Self::from_compressed(compressed_payload, alloc)
    }

    pub fn from_compressed(
        compressed: &'a [u8],
        alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    ) -> Self {
        log::info!("Decompressing payload...");
        let (uncompressed_size, input) = lz4_flex::block::uncompressed_size(compressed).unwrap();

        let uncompressed_payload = unsafe {
            let frames = uncompressed_size.div_ceil(kconfig::PAGE_SIZE);
            let base = alloc.allocate_frames(frames).unwrap();

            slice::from_raw_parts_mut(base.as_raw() as *mut u8, frames * kconfig::PAGE_SIZE)
        };

        lz4_flex::decompress_into(input, uncompressed_payload).unwrap();

        Self::from_bytes(uncompressed_payload)
    }

    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        let elf_file = object::read::elf::ElfFile::parse(bytes).unwrap();

        let loader_config = unsafe {
            let section = elf_file.section_by_name(".loader_config").unwrap();
            let raw = section.data().unwrap();

            let ptr: *const LoaderConfig = raw.as_ptr().cast();
            let cfg = &*ptr;

            cfg.assert_valid();
            cfg
        };

        Self {
            elf_file,
            loader_config,
        }
    }
}
