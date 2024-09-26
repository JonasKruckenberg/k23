//! Support for building and parsing intermediate compilation artifacts in object format

use crate::kconfig;
use crate::runtime::compile::compiled_func::{CompiledFunction, RelocationTarget, TrapInfo};
use crate::runtime::compile::{CompileOutput, FunctionLoc};
use crate::runtime::translate::DebugInfo;
use crate::runtime::Engine;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;
use cranelift_codegen::control::ControlPlane;
use object::write::{
    Object, SectionId, StandardSegment, Symbol, SymbolId, SymbolSection, WritableBuffer,
};
use object::{LittleEndian, SectionKind, SymbolFlags, SymbolKind, SymbolScope, U32Bytes};

pub const ELFOSABI_K23: u8 = 223;
pub const ELF_K23_TRAPS: &str = ".k23.traps";
pub const ELF_K23_INFO: &str = ".k23.info";
pub const ELF_K23_BTI: &str = ".k23.bti";
pub const ELF_K23_ENGINE: &str = ".k23.engine";

pub const ELF_TEXT: &str = ".text";
pub const ELF_WASM_DATA: &str = ".rodata.wasm";
pub const ELF_WASM_NAMES: &str = ".name.wasm";
pub const ELF_WASM_DWARF: &str = ".k23.dwarf";

/// Builder for intermediate compilation artifacts in ELF format
pub struct ObjectBuilder<'obj> {
    result: Object<'obj>,
    dwarf_section: Option<SectionId>,
}

impl<'obj> ObjectBuilder<'obj> {
    pub fn new(obj: Object<'obj>) -> Self {
        ObjectBuilder {
            result: obj,
            dwarf_section: None,
        }
    }

    /// Constructs a new helper [`TextSectionBuilder`] which can be used to
    /// build and append the objects text section.
    pub fn text_builder(
        &mut self,
        text_builder: Box<dyn cranelift_codegen::TextSectionBuilder>,
    ) -> TextSectionBuilder<'_, 'obj> {
        TextSectionBuilder::new(&mut self.result, text_builder)
    }

    /// Creates the `ELF_K23_ENGINE` section and writes the current engine configuration into it
    #[allow(clippy::unused_self)]
    pub fn append_engine_info(&mut self, _engine: &Engine) {}

    pub fn append_debug_info(&mut self, info: &DebugInfo) {
        // let names_section = *self.names_section.get_or_insert_with(|| {
        //     self.result.add_section(
        //         self.result.segment_name(StandardSegment::Data).to_vec(),
        //         ELF_WASM_NAMES.as_bytes().to_vec(),
        //         SectionKind::ReadOnlyData,
        //     )
        // });

        // self.result.append_section_data(
        //     names_section,
        //     &postcard::to_allocvec(&info.names).unwrap(),
        //     1,
        // );

        self.append_dwarf_section(&info.dwarf.debug_abbrev);
        self.append_dwarf_section(&info.dwarf.debug_addr);
        self.append_dwarf_section(&info.dwarf.debug_info);
        self.append_dwarf_section(&info.dwarf.debug_line);
        self.append_dwarf_section(&info.dwarf.debug_line_str);
        self.append_dwarf_section(&info.dwarf.debug_str);
        self.append_dwarf_section(&info.dwarf.debug_str_offsets);
        if let Some(inner) = &info.dwarf.sup {
            self.append_dwarf_section(&inner.debug_str);
        }
        self.append_dwarf_section(&info.dwarf.debug_types);

        self.append_dwarf_section(&info.debug_loc);
        self.append_dwarf_section(&info.debug_loclists);
        self.append_dwarf_section(&info.debug_ranges);
        self.append_dwarf_section(&info.debug_rnglists);
        self.append_dwarf_section(&info.debug_cu_index);
        self.append_dwarf_section(&info.debug_tu_index);
    }

    fn append_dwarf_section<'b, T>(&mut self, section: &T)
    where
        T: gimli::Section<gimli::EndianSlice<'b, gimli::LittleEndian>>,
    {
        let data = section.reader().slice();
        if data.is_empty() {
            return;
        }

        let section_id = *self.dwarf_section.get_or_insert_with(|| {
            self.result.add_section(
                self.result.segment_name(StandardSegment::Debug).to_vec(),
                ELF_WASM_DWARF.as_bytes().to_vec(),
                SectionKind::Debug,
            )
        });

        self.result.append_section_data(section_id, data, 1);
    }

    // /// Appends various bits of metadata about the current module
    // pub fn append_module_artifacts(&mut self) {}

    /// Finished the object and flushes it into the given buffer
    pub fn finish<T: WritableBuffer>(self, buf: &mut T) -> object::write::Result<()> {
        self.result.emit(buf)
    }
}

pub struct TextSectionBuilder<'a, 'obj> {
    /// The object file that generated code will be placed into
    obj: &'a mut Object<'obj>,
    /// The text section ID in the object
    text_section: SectionId,
    /// The cranelift `TextSectionBuilder` that keeps the in-progress text section
    /// that we're building
    inner: Box<dyn cranelift_codegen::TextSectionBuilder>,
    /// Last offset within the text section
    len: u64,

    ctrl_plane: ControlPlane,
}

impl<'a, 'obj> TextSectionBuilder<'a, 'obj> {
    pub fn new(
        obj: &'a mut Object<'obj>,
        text_builder: Box<dyn cranelift_codegen::TextSectionBuilder>,
    ) -> Self {
        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            ELF_TEXT.as_bytes().to_vec(),
            SectionKind::Text,
        );

        Self {
            obj,
            text_section,
            inner: text_builder,
            ctrl_plane: ControlPlane::default(),
            len: 0,
        }
    }

    pub fn push_funcs<'b>(
        &mut self,
        funcs: impl ExactSizeIterator<Item = &'b CompileOutput> + 'b,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> Vec<(SymbolId, FunctionLoc)> {
        let mut ret = Vec::with_capacity(funcs.len());
        let mut traps = TrapSectionBuilder::default();

        for output in funcs {
            let (sym, range) =
                self.push_func(&output.symbol, &output.function, &resolve_reloc_target);

            traps.push_traps(&range, output.function.traps());

            let info = FunctionLoc {
                start: u32::try_from(range.start).unwrap(),
                length: u32::try_from(range.end - range.start).unwrap(),
            };

            ret.push((sym, info));
        }

        traps.append(self.obj);

        ret
    }

    /// Append the `func` with name `name` to this object.
    pub fn push_func(
        &mut self,
        name: &str,
        compiled_func: &CompiledFunction,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> (SymbolId, Range<u64>) {
        let body = compiled_func.buffer.data();
        let alignment = compiled_func.alignment;
        let body_len = body.len() as u64;
        let off = self
            .inner
            .append(true, body, alignment, &mut self.ctrl_plane);

        let symbol_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: off,
            size: body_len,
            kind: SymbolKind::Text,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(self.text_section),
            flags: SymbolFlags::None,
        });

        for r in compiled_func.relocations() {
            match r.target {
                RelocationTarget::Wasm(_) | RelocationTarget::Builtin(_) => {
                    let target = resolve_reloc_target(r.target);

                    // Ensure that we actually resolved the relocation
                    debug_assert!(self.inner.resolve_reloc(
                        off + u64::from(r.offset),
                        r.kind,
                        r.addend,
                        target
                    ));
                }
            }
        }

        self.len = off + body_len;

        (symbol_id, off..off + body_len)
    }

    pub fn append_padding(&mut self, padding: usize) {
        if padding == 0 {
            return;
        }
        self.inner
            .append(false, &vec![0; padding], 1, &mut self.ctrl_plane);
    }

    /// Finish building the text section and flush it into the object file
    pub fn finish(mut self) {
        let padding =
            kconfig::PAGE_SIZE - (usize::try_from(self.len).unwrap() % kconfig::PAGE_SIZE);
        // Add padding at the end so that the text section is fully page aligned
        self.append_padding(padding);

        let text = self.inner.finish(&mut self.ctrl_plane);

        self.obj
            .section_mut(self.text_section)
            .set_data(text, kconfig::PAGE_SIZE as u64);
    }
}

#[derive(Default)]
struct TrapSectionBuilder {
    offsets: Vec<U32Bytes<LittleEndian>>,
    traps: Vec<u8>,
}

impl TrapSectionBuilder {
    pub fn push_traps(
        &mut self,
        func: &Range<u64>,
        traps: impl ExactSizeIterator<Item = TrapInfo>,
    ) {
        let func_start = u32::try_from(func.start).unwrap();

        self.offsets.reserve_exact(traps.len());
        self.traps.reserve_exact(traps.len());

        for trap in traps {
            let pos = func_start + trap.offset;
            self.offsets.push(U32Bytes::new(LittleEndian, pos));
            self.traps.push(trap.code as u8);
        }
    }

    pub fn append(self, obj: &mut Object) {
        let traps_section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            ELF_K23_TRAPS.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );

        let amt = u32::try_from(self.traps.len()).unwrap();
        obj.append_section_data(traps_section, &amt.to_le_bytes(), 1);
        obj.append_section_data(traps_section, object::bytes_of_slice(&self.offsets), 1);
        obj.append_section_data(traps_section, &self.traps, 1);
    }
}
