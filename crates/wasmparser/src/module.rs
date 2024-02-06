use crate::BinaryReader;
use crate::Section;

const WASM_MAGIC_BYTES: &[u8; 4] = b"\0asm";
const WASM_VERSION: u32 = 0x01;

pub struct Module<'a> {
    pub(crate) reader: BinaryReader<'a>,
}

impl<'a> Module<'a> {
    pub(crate) fn is_wasm_module(bytes: &[u8]) -> bool {
        const HEADER: [u8; 8] = [
            WASM_MAGIC_BYTES[0],
            WASM_MAGIC_BYTES[1],
            WASM_MAGIC_BYTES[2],
            WASM_MAGIC_BYTES[3],
            WASM_VERSION.to_le_bytes()[0],
            WASM_VERSION.to_le_bytes()[1],
            WASM_VERSION.to_le_bytes()[2],
            WASM_VERSION.to_le_bytes()[3],
        ];

        bytes.starts_with(&HEADER)
    }

    pub fn sections(&self) -> SectionsIter<'a> {
        SectionsIter {
            reader: self.reader.clone(),
            err: false,
        }
    }
}

pub struct SectionsIter<'a> {
    reader: BinaryReader<'a>,
    err: bool,
}

impl<'a> Iterator for SectionsIter<'a> {
    type Item = crate::Result<Section<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.err || self.reader.remaining_bytes().is_empty() {
            None
        } else {
            let res = self.reader.read_section();
            self.err = res.is_err();
            Some(res)
        }
    }
}
