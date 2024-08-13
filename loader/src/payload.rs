use crate::kconfig;
use crate::machine_info::MachineInfo;
use bitflags::bitflags;
use core::ffi::CStr;
use core::slice;
use dtb_parser::{DevTree, Node, Visitor};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
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

        let section = Section::new(&section.data().unwrap());

        for subsection in section {
            for subsubsection in subsection {
                for attr in subsubsection {
                    if let Attribute::Arch(required_features) = attr {
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

struct Section<'a> {
    reader: Reader<'a>,
}

impl<'a> Section<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        let mut reader = Reader { buf };
        assert_eq!(reader.read_u8(), b'A');
        Self { reader }
    }
}

impl<'a> Iterator for Section<'a> {
    type Item = SubSection<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.is_empty() {
            return None;
        }

        let len = self.reader.read_u32();
        let buf = self.reader.read_bytes(len as usize - 4); // account for len
        let mut reader = Reader { buf };
        let name = reader.read_nul_terminated_str();

        Some(SubSection { reader, name })
    }
}

struct SubSection<'a> {
    reader: Reader<'a>,
    name: &'a CStr,
}

impl<'a> Iterator for SubSection<'a> {
    type Item = SubSubSection<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.is_empty() {
            return None;
        }

        let tag = self.reader.read_uleb128();
        let len = self.reader.read_u32();
        let buf = self.reader.read_bytes(len as usize - 5); // account for tag & len
        let reader = Reader { buf };

        Some(SubSubSection { reader, tag })
    }
}

struct SubSubSection<'a> {
    reader: Reader<'a>,
    tag: u64,
}

impl<'a> Iterator for SubSubSection<'a> {
    type Item = Attribute;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.reader.is_empty() {
            let tag = self.reader.read_uleb128();

            match tag {
                4 => return Some(Attribute::StackAlign(self.reader.read_uleb128())),
                5 => {
                    return Some(Attribute::Arch(RiscvFeatures::from_elf_str(
                        self.reader.read_nul_terminated_str().to_str().unwrap(),
                    )))
                }
                6 => return Some(Attribute::UnalignedAccess(self.reader.read_uleb128())),
                8 => return Some(Attribute::PrivSpec(self.reader.read_uleb128())),
                10 => return Some(Attribute::PrivSpecMinor(self.reader.read_uleb128())),
                12 => return Some(Attribute::PrivSpecRevision(self.reader.read_uleb128())),
                14 => return Some(Attribute::AtomicAbi(self.reader.read_uleb128())),
                16 => return Some(Attribute::X3RegUsage(self.reader.read_uleb128())),
                _ if (tag % 128) < 64 => panic!("unsupported tag"),
                _ => {}
            }
        }

        None
    }
}

#[derive(Debug)]
enum Attribute {
    StackAlign(u64),
    Arch(RiscvFeatures),
    UnalignedAccess(u64),
    PrivSpec(u64),
    PrivSpecMinor(u64),
    PrivSpecRevision(u64),
    AtomicAbi(u64),
    X3RegUsage(u64),
}

bitflags! {
    #[derive(Debug, Copy, Clone)]
    struct RiscvFeatures: u64 {
        const I = 1 << 0;
        const M = 1 << 1;
        const A = 1 << 2;
        const F = 1 << 3;
        const D = 1 << 4;
        const C = 1 << 5;
        const H = 1 << 6;
        const ZIC64B = 1 << 7;
        const ZICBOM = 1 << 8;
        const ZICBOP = 1 << 9;
        const ZICBOZ = 1 << 10;
        const ZICCAMOA = 1 << 11;
        const ZICCIF = 1 << 12;
        const ZICCLSM = 1 << 13;
        const ZICCRSE = 1 << 14;
        const ZICNTR = 1 << 15;
        const ZICSR = 1 << 16;
        const ZIFENCEI = 1 << 17;
        const ZIHINTNTL = 1 << 18;
        const ZIHINTPAUSE = 1 << 19;
        const ZIHPM = 1 << 20;
        const ZA64RS = 1 << 21;
        const ZAWRS = 1 << 22;
        const ZFA = 1 << 23;
        const ZCA = 1 << 24;
        const ZCD = 1 << 25;
        const ZBA = 1 << 26;
        const ZBB = 1 << 27;
        const ZBC = 1 << 28;
        const ZBS = 1 << 29;
        const SSCCPTR = 1 << 30;
        const SSCOUNTERENW = 1 << 31;
        const SSTC = 1 << 32;
        const SSTVALA = 1 << 33;
        const SSTVECD = 1 << 34;
        const SVADU = 1 << 35;
    }
}

impl RiscvFeatures {
    fn from_str(ext: &str) -> Self {
        match ext {
            "rv64imafdch" => {
                RiscvFeatures::I
                    | RiscvFeatures::M
                    | RiscvFeatures::A
                    | RiscvFeatures::F
                    | RiscvFeatures::D
                    | RiscvFeatures::C
                    | RiscvFeatures::H
            }
            "i" | "rv64i" => RiscvFeatures::I,
            "m" => RiscvFeatures::M,
            "a" => RiscvFeatures::A,
            "f" => RiscvFeatures::F,
            "d" => RiscvFeatures::D,
            "c" => RiscvFeatures::C,
            "h" => RiscvFeatures::H,
            "zic64b" => RiscvFeatures::ZIC64B,
            "zicbom" => RiscvFeatures::ZICBOM,
            "zicbop" => RiscvFeatures::ZICBOP,
            "zicboz" => RiscvFeatures::ZICBOZ,
            "ziccamoa" => RiscvFeatures::ZICCAMOA,
            "ziccif" => RiscvFeatures::ZICCIF,
            "zicclsm" => RiscvFeatures::ZICCLSM,
            "ziccrse" => RiscvFeatures::ZICCRSE,
            "zicntr" => RiscvFeatures::ZICNTR,
            "zicsr" => RiscvFeatures::ZICSR,
            "zifencei" => RiscvFeatures::ZIFENCEI,
            "zihintntl" => RiscvFeatures::ZIHINTNTL,
            "zihintpause" => RiscvFeatures::ZIHINTPAUSE,
            "zihpm" => RiscvFeatures::ZIHPM,
            "za64rs" => RiscvFeatures::ZA64RS,
            "zawrs" => RiscvFeatures::ZAWRS,
            "zfa" => RiscvFeatures::ZFA,
            "zca" => RiscvFeatures::ZCA,
            "zcd" => RiscvFeatures::ZCD,
            "zba" => RiscvFeatures::ZBA,
            "zbb" => RiscvFeatures::ZBB,
            "zbc" => RiscvFeatures::ZBC,
            "zbs" => RiscvFeatures::ZBS,
            "ssccptr" => RiscvFeatures::SSCCPTR,
            "sscounterenw" => RiscvFeatures::SSCOUNTERENW,
            "sstc" => RiscvFeatures::SSTC,
            "sstvala" => RiscvFeatures::SSTVALA,
            "sstvecd" => RiscvFeatures::SSTVECD,
            "svadu" => RiscvFeatures::SVADU,
            _ => {
                log::warn!("unknown extension {ext:?}");
                RiscvFeatures::empty()
            }
        }
    }

    pub fn from_elf_str(s: &str) -> Self {
        let parts = s.split('_');
        let mut out = Self::empty();

        for part in parts {
            let (ext, _) = part.split_at(part.len() - 3);
            out |= Self::from_str(ext);
        }

        out
    }

    pub fn from_dtb_riscv_isa(s: &str) -> Self {
        let exts = s.split('_');
        let mut out = Self::empty();
        for ext in exts {
            out |= Self::from_str(ext)
        }
        out
    }
}

const CONTINUATION_BIT: u8 = 1 << 7;

#[inline]
fn low_bits_of_byte(byte: u8) -> u8 {
    byte & !CONTINUATION_BIT
}

#[derive(Debug)]
struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn read_bytes(&mut self, len: usize) -> &'a [u8] {
        let (buf, rest) = self.buf.split_at(len);
        self.buf = rest;
        buf
    }

    fn read_u8(&mut self) -> u8 {
        let b = self.read_bytes(1);
        b[0]
    }

    fn read_u32(&mut self) -> u32 {
        let b = self.read_bytes(4);
        u32::from_le_bytes(b.try_into().unwrap())
    }

    fn read_nul_terminated_str(&mut self) -> &'a CStr {
        let str = CStr::from_bytes_until_nul(self.buf).unwrap();

        let (_, rest) = self.buf.split_at(str.count_bytes() + 1);
        self.buf = rest;
        str
    }

    fn read_uleb128(&mut self) -> u64 {
        let mut result = 0;
        let mut shift = 0;

        loop {
            let mut buf = self.read_bytes(1);

            if shift == 63 && buf[0] != 0x00 && buf[0] != 0x01 {
                while buf[0] & CONTINUATION_BIT != 0 {
                    buf = self.read_bytes(1);
                }
                panic!("overflow")
            }

            let low_bits = low_bits_of_byte(buf[0]) as u64;
            result |= low_bits << shift;

            if buf[0] & CONTINUATION_BIT == 0 {
                return result;
            }

            shift += 7;
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
            let supported_features = RiscvFeatures::from_dtb_riscv_isa(s);

            assert!(supported_features.contains(self.required_features));
        }

        Ok(())
    }
}
