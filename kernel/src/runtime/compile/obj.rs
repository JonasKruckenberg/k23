//! Support for building and parsing intermediate compilation artifacts in object format

use crate::kconfig;
use crate::runtime::compile::{CompileOutput, CompiledFunction, FunctionLoc, RelocationTarget};
use crate::runtime::engine::Engine;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::TextSectionBuilder;
use object::write::{
    Object, SectionId, StandardSegment, Symbol, SymbolId, SymbolSection, WritableBuffer,
};
use object::{SectionKind, SymbolFlags, SymbolKind, SymbolScope};

pub const ELFOSABI_K23: u8 = 223;
pub const ELF_K23_TRAPS: &str = ".k23.traps";
pub const ELF_K23_INFO: &'static str = ".k23.info";
pub const ELF_K23_BTI: &str = ".k23.bti";
pub const ELF_K23_ENGINE: &str = ".k23.engine";

pub const ELF_TEXT: &str = ".text";
pub const ELF_WASM_DATA: &str = ".rodata.wasm";
pub const ELF_WASM_NAMES: &str = ".name.wasm";
pub const ELF_WASM_DWARF: &str = ".k23.dwarf";

/// Builder for intermediate compilation artifacts in ELF format
pub struct ObjectBuilder<'obj> {
    result: Object<'obj>,

    rodata_section: SectionId,
    names_section: Option<SectionId>,
    dwarf_section: Option<SectionId>,
}

impl<'obj> ObjectBuilder<'obj> {
    pub fn new(mut obj: Object<'obj>) -> Self {
        let rodata_section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            ELF_WASM_DATA.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );

        ObjectBuilder {
            result: obj,
            rodata_section,
            names_section: None,
            dwarf_section: None,
        }
    }

    /// Constructs a new helper [`TextSectionBuilder`] which can be used to
    /// build and append the objects text section.
    pub fn text_builder(
        &mut self,
        text_builder: Box<dyn TextSectionBuilder>,
    ) -> ObjectTextBuilder<'_, 'obj> {
        ObjectTextBuilder::new(&mut self.result, text_builder)
    }

    /// Creates the `ELF_K23_ENGINE` section and writes the current engine configuration into it
    pub fn append_engine_info(&mut self, _engine: &Engine) {}

    /// Appends various bits of metadata about the current module
    pub fn append_module_artifacts(&mut self) {}

    /// Finished the object and flushes it into the given buffer
    pub fn finish<T: WritableBuffer>(self, buf: &mut T) -> object::write::Result<()> {
        self.result.emit(buf)
    }
}

pub struct ObjectTextBuilder<'a, 'obj> {
    /// The object file that generated code will be placed into
    obj: &'a mut Object<'obj>,
    /// The text section ID in the object
    text_section: SectionId,
    /// The cranelift `TextSectionBuilder` that keeps the in-progress text section
    /// that we're building
    text_builder: Box<dyn TextSectionBuilder>,
    /// Last offset within the text section
    len: u64,

    ctrl_plane: ControlPlane,
}

impl<'a, 'obj> ObjectTextBuilder<'a, 'obj> {
    pub fn new(obj: &'a mut Object<'obj>, text_builder: Box<dyn TextSectionBuilder>) -> Self {
        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            ELF_TEXT.as_bytes().to_vec(),
            SectionKind::Text,
        );

        Self {
            obj,
            text_section,
            text_builder,
            ctrl_plane: Default::default(),
            len: 0,
        }
    }

    pub fn append_funcs<'b>(
        &mut self,
        funcs: impl ExactSizeIterator<Item = &'b CompileOutput> + 'b,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> Vec<(SymbolId, FunctionLoc)> {
        let mut ret = Vec::with_capacity(funcs.len());

        for output in funcs {
            let (sym, range) =
                self.append_func(&output.symbol, &output.function, &resolve_reloc_target);

            let info = FunctionLoc {
                start: u32::try_from(range.start).unwrap(),
                length: u32::try_from(range.end - range.start).unwrap(),
            };

            ret.push((sym, info));
        }

        ret
    }

    /// Append the `func` with name `name` to this object.
    pub fn append_func(
        &mut self,
        name: &str,
        compiled_func: &CompiledFunction,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> (SymbolId, Range<u64>) {
        let body = compiled_func.buffer.data();
        let alignment = compiled_func.alignment;
        let body_len = body.len() as u64;
        let off = self
            .text_builder
            .append(true, &body, alignment, &mut self.ctrl_plane);

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
                    debug_assert!(self.text_builder.resolve_reloc(
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
        self.text_builder
            .append(false, &vec![0; padding], 1, &mut self.ctrl_plane);
    }

    /// Finish building the text section and flush it into the object file
    pub fn finish(mut self) {
        let padding = kconfig::PAGE_SIZE - (self.len as usize % kconfig::PAGE_SIZE);
        // // Add padding at the end so that the text section is fully page aligned
        self.append_padding(padding);

        let text = self.text_builder.finish(&mut self.ctrl_plane);

        self.obj
            .section_mut(self.text_section)
            .set_data(text, kconfig::PAGE_SIZE as u64);
    }
}
