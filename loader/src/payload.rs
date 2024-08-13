use crate::kconfig;
use crate::machine_info::MachineInfo;
use bitflags::bitflags;
use core::ffi::CStr;
use core::slice;
use dtb_parser::{DevTree, Node, Visitor};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kstd::arch::riscv_features::{ElfRiscvAttribute, ElfRiscvAttributesSection, RiscvFeatures};
use loader_api::LoaderConfig;
use object::read::elf::ElfFile64;
use object::{Object, ObjectSection};
use vmm::{BumpAllocator, FrameAllocator, INIT};

pub struct Payload<'a> {
    pub elf_file: object::read::elf::ElfFile64<'a>,
    pub loader_config: &'a LoaderConfig,
}

impl<'a> Payload<'a> {
    pub fn from_signed_and_compressed(
        bytes: &'a [u8],
        verifying_key: &'static [u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
        alloc: &mut BumpAllocator<'_, INIT<kconfig::MEMORY_MODE>>,
    ) -> Self {
        log::info!("Verifying payload signature...");
        let verifying_key = VerifyingKey::from_bytes(verifying_key).unwrap();
        let (signature, compressed_payload) = bytes.split_at(Signature::BYTE_SIZE);
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

    #[cfg(target_arch = "riscv64")]
    pub fn assert_cpu_compatible(&self, fdt_ptr: *const u8) {
        let section = self
            .elf_file
            .section_by_name(".riscv.attributes")
            .expect(".riscv.attributes section is required");

        let section = ElfRiscvAttributesSection::new(&section.data().unwrap());

        for mut subsection in section {
            if subsection.name() == c"riscv" {
                let subsubsection = subsection.next().unwrap();
                assert!(subsection.next().is_none());

                for attr in subsubsection {
                    if let ElfRiscvAttribute::Arch(required_features) = attr {
                        let fdt = unsafe { DevTree::from_raw(fdt_ptr) }.unwrap();

                        let mut v = CpuFeaturesVisitor { required_features };
                        fdt.visit(&mut v).unwrap();
                    }
                }
            }
        }
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

struct CpuFeaturesVisitor {
    required_features: RiscvFeatures,
}

impl<'dt> Visitor<'dt> for CpuFeaturesVisitor {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name == "cpus" || name.is_empty() || name.starts_with("cpu@") {
            node.visit(self)?;
        }

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "riscv,isa" {
            let s = CStr::from_bytes_with_nul(value).unwrap().to_str().unwrap();
            let supported_features = RiscvFeatures::from_dtb_riscv_isa_str(s);

            assert!(supported_features.contains(self.required_features));
        }

        Ok(())
    }
}
