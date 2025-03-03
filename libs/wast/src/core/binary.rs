use crate::component::Component;
use crate::core::*;
use crate::encode::Encode;
use crate::token::*;
use crate::Wat;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::marker;
#[cfg(feature = "dwarf")]
use core::path::Path;

/// Options that can be specified when encoding a component or a module to
/// customize what the final binary looks like.
///
/// Methods such as [`Module::encode`], [`Wat::encode`], and
/// [`Component::encode`] will use the default options.
#[derive(Default)]
pub struct EncodeOptions<'a> {
    #[cfg(feature = "dwarf")]
    dwarf_info: Option<(&'a Path, &'a str, GenerateDwarf)>,

    _marker: marker::PhantomData<&'a str>,
}

#[cfg(feature = "dwarf")]
mod dwarf;

#[cfg(not(feature = "dwarf"))]
mod dwarf_disabled;
#[cfg(not(feature = "dwarf"))]
use self::dwarf_disabled as dwarf;

/// Configuration of how DWARF debugging information may be generated.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum GenerateDwarf {
    /// Only generate line tables to map binary offsets back to source
    /// locations.
    Lines,

    /// Generate full debugging information for both line numbers and
    /// variables/locals/operands.
    Full,
}

impl<'a> EncodeOptions<'a> {
    /// Creates a new set of default encoding options.
    pub fn new() -> EncodeOptions<'a> {
        EncodeOptions::default()
    }

    /// Enables emission of DWARF debugging information in the final binary.
    ///
    /// This method will use the `file` specified as the source file for the
    /// `*.wat` file whose `contents` must also be supplied here. These are
    /// used to calculate filenames/line numbers and are referenced from the
    /// generated DWARF.
    #[cfg(feature = "dwarf")]
    pub fn dwarf(&mut self, file: &'a Path, contents: &'a str, style: GenerateDwarf) -> &mut Self {
        self.dwarf_info = Some((file, contents, style));
        self
    }

    /// Encodes the given [`Module`] with these options.
    ///
    /// For more information see [`Module::encode`].
    pub fn encode_module(
        &self,
        module: &mut Module<'_>,
    ) -> core::result::Result<Vec<u8>, crate::Error> {
        module.resolve()?;
        Ok(match &module.kind {
            ModuleKind::Text(fields) => encode(&module.id, &module.name, fields, self),
            ModuleKind::Binary(blobs) => blobs.iter().flat_map(|b| b.iter().cloned()).collect(),
        })
    }

    /// Encodes the given [`Component`] with these options.
    ///
    /// For more information see [`Component::encode`].
    pub fn encode_component(
        &self,
        component: &mut Component<'_>,
    ) -> core::result::Result<Vec<u8>, crate::Error> {
        component.resolve()?;
        Ok(crate::component::binary::encode(component, self))
    }

    /// Encodes the given [`Wat`] with these options.
    ///
    /// For more information see [`Wat::encode`].
    pub fn encode_wat(&self, wat: &mut Wat<'_>) -> core::result::Result<Vec<u8>, crate::Error> {
        match wat {
            Wat::Module(m) => self.encode_module(m),
            Wat::Component(c) => self.encode_component(c),
        }
    }
}

pub(crate) fn encode(
    module_id: &Option<Id<'_>>,
    module_name: &Option<NameAnnotation<'_>>,
    fields: &[ModuleField<'_>],
    opts: &EncodeOptions,
) -> Vec<u8> {
    use CustomPlace::*;
    use CustomPlaceAnchor::*;

    let mut types = Vec::new();
    let mut imports = Vec::new();
    let mut funcs = Vec::new();
    let mut tables = Vec::new();
    let mut memories = Vec::new();
    let mut globals = Vec::new();
    let mut exports = Vec::new();
    let mut start = Vec::new();
    let mut elem = Vec::new();
    let mut data = Vec::new();
    let mut tags = Vec::new();
    let mut customs = Vec::new();
    for field in fields {
        match field {
            ModuleField::Type(i) => types.push(RecOrType::Type(i)),
            ModuleField::Rec(i) => types.push(RecOrType::Rec(i)),
            ModuleField::Import(i) => imports.push(i),
            ModuleField::Func(i) => funcs.push(i),
            ModuleField::Table(i) => tables.push(i),
            ModuleField::Memory(i) => memories.push(i),
            ModuleField::Global(i) => globals.push(i),
            ModuleField::Export(i) => exports.push(i),
            ModuleField::Start(i) => start.push(i),
            ModuleField::Elem(i) => elem.push(i),
            ModuleField::Data(i) => data.push(i),
            ModuleField::Tag(i) => tags.push(i),
            ModuleField::Custom(i) => customs.push(i),
        }
    }

    let mut e = Encoder {
        wasm: Vec::new(),
        tmp: Vec::new(),
        customs: &customs,
    };
    e.wasm.extend(b"\0asm");
    e.wasm.extend(b"\x01\0\0\0");

    e.custom_sections(BeforeFirst);

    e.section_list(1, Type, &types);
    e.section_list(2, Import, &imports);

    let functys = funcs.iter().map(|f| &f.ty).collect::<Vec<_>>();
    e.section_list(3, Func, &functys);
    e.section_list(4, Table, &tables);
    e.section_list(5, Memory, &memories);
    e.section_list(13, Tag, &tags);
    e.section_list(6, Global, &globals);
    e.section_list(7, Export, &exports);
    e.custom_sections(Before(Start));
    if let Some(start) = start.first() {
        e.section(8, start);
    }
    e.custom_sections(After(Start));
    e.section_list(9, Elem, &elem);
    if needs_data_count(&funcs) {
        e.section(12, &data.len());
    }

    // Prepare to and emit the code section. This is where DWARF may optionally
    // be emitted depending on configuration settings. Note that `code_section`
    // will internally emit the branch hints section if necessary.
    let names = find_names(module_id, module_name, fields);
    let num_import_funcs = imports
        .iter()
        .filter(|i| matches!(i.item.kind, ItemKind::Func(..)))
        .count() as u32;
    let mut dwarf = dwarf::Dwarf::new(num_import_funcs, opts, &names, &types);
    e.code_section(&funcs, num_import_funcs, dwarf.as_mut());

    e.section_list(11, Data, &data);

    if !names.is_empty() {
        e.section(0, &("name", names));
    }
    e.custom_sections(AfterLast);
    if let Some(dwarf) = &mut dwarf {
        dwarf.emit(&mut e);
    }

    return e.wasm;

    fn needs_data_count(funcs: &[&crate::core::Func<'_>]) -> bool {
        funcs
            .iter()
            .filter_map(|f| match &f.kind {
                FuncKind::Inline { expression, .. } => Some(expression),
                _ => None,
            })
            .flat_map(|e| e.instrs.iter())
            .any(|i| i.needs_data_count())
    }
}

struct Encoder<'a> {
    wasm: Vec<u8>,
    tmp: Vec<u8>,
    customs: &'a [&'a Custom<'a>],
}

impl Encoder<'_> {
    fn section(&mut self, id: u8, section: &dyn Encode) {
        self.tmp.truncate(0);
        section.encode(&mut self.tmp);
        self.wasm.push(id);
        self.tmp.encode(&mut self.wasm);
    }

    fn custom_sections(&mut self, place: CustomPlace) {
        for entry in self.customs.iter() {
            if entry.place() == place {
                let mut data = Vec::new();
                entry.encode(&mut data);
                self.custom_section(entry.name(), &data);
            }
        }
    }

    fn custom_section(&mut self, name: &str, data: &[u8]) {
        self.tmp.truncate(0);
        name.encode(&mut self.tmp);
        self.tmp.extend_from_slice(data);
        self.wasm.push(0);
        self.tmp.encode(&mut self.wasm);
    }

    fn section_list(&mut self, id: u8, anchor: CustomPlaceAnchor, list: &[impl Encode]) {
        self.custom_sections(CustomPlace::Before(anchor));
        if !list.is_empty() {
            self.section(id, &list)
        }
        self.custom_sections(CustomPlace::After(anchor));
    }

    /// Encodes the code section of a wasm module module while additionally
    /// handling the branch hinting proposal.
    ///
    /// The branch hinting proposal requires to encode the offsets of the
    /// instructions relative from the beginning of the function. Here we encode
    /// each instruction and we save its offset. If needed, we use this
    /// information to build the branch hint section and insert it before the
    /// code section.
    ///
    /// The `list` provided is the list of functions that are emitted into the
    /// code section. The `func_index` provided is the initial index of defined
    /// functions, so it's the count of imported functions. The `dwarf` field is
    /// optionally used to track debugging information.
    fn code_section<'a>(
        &'a mut self,
        list: &[&'a Func<'_>],
        mut func_index: u32,
        mut dwarf: Option<&mut dwarf::Dwarf>,
    ) {
        self.custom_sections(CustomPlace::Before(CustomPlaceAnchor::Code));

        if !list.is_empty() {
            let mut branch_hints = Vec::new();
            let mut code_section = Vec::new();

            list.len().encode(&mut code_section);
            for func in list.iter() {
                let hints = func.encode(&mut code_section, dwarf.as_deref_mut());
                if !hints.is_empty() {
                    branch_hints.push(FunctionBranchHints { func_index, hints });
                }
                func_index += 1;
            }

            // Branch hints section has to be inserted before the Code section
            // Insert the section only if we have some hints
            if !branch_hints.is_empty() {
                self.section(0, &("metadata.code.branch_hint", branch_hints));
            }

            // Finally, insert the Code section from the tmp buffer
            self.wasm.push(10);
            code_section.encode(&mut self.wasm);

            if let Some(dwarf) = &mut dwarf {
                dwarf.set_code_section_size(code_section.len());
            }
        }
        self.custom_sections(CustomPlace::After(CustomPlaceAnchor::Code));
    }
}

impl Encode for FunctionType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.params.len().encode(e);
        for (_, _, ty) in self.params.iter() {
            ty.encode(e);
        }
        self.results.encode(e);
    }
}

impl Encode for StructType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.fields.len().encode(e);
        for field in self.fields.iter() {
            field.ty.encode(e);
            (field.mutable as i32).encode(e);
        }
    }
}

impl Encode for ArrayType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.ty.encode(e);
        (self.mutable as i32).encode(e);
    }
}

impl Encode for ExportType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.name.encode(e);
        self.item.encode(e);
    }
}

enum RecOrType<'a> {
    Type(&'a Type<'a>),
    Rec(&'a Rec<'a>),
}

impl Encode for RecOrType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            RecOrType::Type(ty) => ty.encode(e),
            RecOrType::Rec(rec) => rec.encode(e),
        }
    }
}

impl Encode for Type<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match (&self.parent, self.final_type) {
            (Some(parent), Some(true)) => {
                // Type is final with a supertype
                e.push(0x4f);
                e.push(0x01);
                parent.encode(e);
            }
            (Some(parent), Some(false) | None) => {
                // Type is not final and has a declared supertype
                e.push(0x50);
                e.push(0x01);
                parent.encode(e);
            }
            (None, Some(false)) => {
                // Sub was used without any declared supertype
                e.push(0x50);
                e.push(0x00);
            }
            (None, _) => {} // No supertype, sub wasn't used
        }
        if self.def.shared {
            e.push(0x65);
        }
        match &self.def.kind {
            InnerTypeKind::Func(func) => {
                e.push(0x60);
                func.encode(e)
            }
            InnerTypeKind::Struct(r#struct) => {
                e.push(0x5f);
                r#struct.encode(e)
            }
            InnerTypeKind::Array(array) => {
                e.push(0x5e);
                array.encode(e)
            }
        }
    }
}

impl Encode for Rec<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        e.push(0x4e);
        self.types.len().encode(e);
        for ty in &self.types {
            ty.encode(e);
        }
    }
}

impl Encode for Option<Id<'_>> {
    fn encode(&self, _e: &mut Vec<u8>) {
        // used for parameters in the tuple impl as well as instruction labels
    }
}

impl Encode for ValType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            ValType::I32 => e.push(0x7f),
            ValType::I64 => e.push(0x7e),
            ValType::F32 => e.push(0x7d),
            ValType::F64 => e.push(0x7c),
            ValType::V128 => e.push(0x7b),
            ValType::Ref(ty) => {
                ty.encode(e);
            }
        }
    }
}

impl Encode for HeapType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            HeapType::Abstract { shared, ty } => {
                if *shared {
                    e.push(0x65);
                }
                ty.encode(e)
            }
            // Note that this is encoded as a signed leb128 so be sure to cast
            // to an i64 first
            HeapType::Concrete(Index::Num(n, _)) => i64::from(*n).encode(e),
            HeapType::Concrete(Index::Id(n)) => {
                panic!("unresolved index in emission: {:?}", n)
            }
        }
    }
}

impl Encode for AbstractHeapType {
    fn encode(&self, e: &mut Vec<u8>) {
        use AbstractHeapType::*;
        match self {
            Func => e.push(0x70),
            Extern => e.push(0x6f),
            Exn => e.push(0x69),
            Any => e.push(0x6e),
            Eq => e.push(0x6d),
            Struct => e.push(0x6b),
            Array => e.push(0x6a),
            I31 => e.push(0x6c),
            NoFunc => e.push(0x73),
            NoExtern => e.push(0x72),
            NoExn => e.push(0x74),
            None => e.push(0x71),
        }
    }
}

impl Encode for RefType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            // Binary abbreviations (i.e., short form), for when the ref is
            // nullable.
            RefType {
                nullable: true,
                heap: HeapType::Abstract { shared, ty },
            } => {
                if *shared {
                    e.push(0x65);
                }
                ty.encode(e);
            }

            // Generic 'ref null <heaptype>' encoding (i.e., long form).
            RefType {
                nullable: true,
                heap,
            } => {
                e.push(0x63);
                heap.encode(e);
            }

            // Generic 'ref <heaptype>' encoding.
            RefType {
                nullable: false,
                heap,
            } => {
                e.push(0x64);
                heap.encode(e);
            }
        }
    }
}

impl Encode for StorageType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            StorageType::I8 => e.push(0x78),
            StorageType::I16 => e.push(0x77),
            StorageType::Val(ty) => {
                ty.encode(e);
            }
        }
    }
}

impl Encode for Import<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.module.encode(e);
        self.field.encode(e);
        self.item.encode(e);
    }
}

impl Encode for ItemSig<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match &self.kind {
            ItemKind::Func(f) => {
                e.push(0x00);
                f.encode(e);
            }
            ItemKind::Table(f) => {
                e.push(0x01);
                f.encode(e);
            }
            ItemKind::Memory(f) => {
                e.push(0x02);
                f.encode(e);
            }
            ItemKind::Global(f) => {
                e.push(0x03);
                f.encode(e);
            }
            ItemKind::Tag(f) => {
                e.push(0x04);
                f.encode(e);
            }
        }
    }
}

impl<T> Encode for TypeUse<'_, T> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.index
            .as_ref()
            .expect("TypeUse should be filled in by this point")
            .encode(e)
    }
}

impl Encode for Index<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            Index::Num(n, _) => n.encode(e),
            Index::Id(n) => panic!("unresolved index in emission: {:?}", n),
        }
    }
}

impl Encode for TableType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.elem.encode(e);

        let mut flags = 0;
        if self.limits.max.is_some() {
            flags |= 1 << 0;
        }
        if self.shared {
            flags |= 1 << 1;
        }
        if self.limits.is64 {
            flags |= 1 << 2;
        }
        e.push(flags);
        self.limits.min.encode(e);
        if let Some(max) = self.limits.max {
            max.encode(e);
        }
    }
}

impl Encode for MemoryType {
    fn encode(&self, e: &mut Vec<u8>) {
        let mut flags = 0;
        if self.limits.max.is_some() {
            flags |= 1 << 0;
        }
        if self.shared {
            flags |= 1 << 1;
        }
        if self.limits.is64 {
            flags |= 1 << 2;
        }
        if self.page_size_log2.is_some() {
            flags |= 1 << 3;
        }
        e.push(flags);
        self.limits.min.encode(e);
        if let Some(max) = self.limits.max {
            max.encode(e);
        }
        if let Some(p) = self.page_size_log2 {
            p.encode(e);
        }
    }
}

impl Encode for GlobalType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.ty.encode(e);
        let mut flags = 0;
        if self.mutable {
            flags |= 0b01;
        }
        if self.shared {
            flags |= 0b10;
        }
        e.push(flags);
    }
}

impl Encode for Table<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        assert!(self.exports.names.is_empty());
        match &self.kind {
            TableKind::Normal {
                ty,
                init_expr: None,
            } => ty.encode(e),
            TableKind::Normal {
                ty,
                init_expr: Some(init_expr),
            } => {
                e.push(0x40);
                e.push(0x00);
                ty.encode(e);
                init_expr.encode(e, None);
            }
            _ => panic!("TableKind should be normal during encoding"),
        }
    }
}

impl Encode for Memory<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        assert!(self.exports.names.is_empty());
        match &self.kind {
            MemoryKind::Normal(t) => t.encode(e),
            _ => panic!("MemoryKind should be normal during encoding"),
        }
    }
}

impl Encode for Global<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        assert!(self.exports.names.is_empty());
        self.ty.encode(e);
        match &self.kind {
            GlobalKind::Inline(expr) => {
                let _hints = expr.encode(e, None);
            }
            _ => panic!("GlobalKind should be inline during encoding"),
        }
    }
}

impl Encode for Export<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.name.encode(e);
        self.kind.encode(e);
        self.item.encode(e);
    }
}

impl Encode for ExportKind {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            ExportKind::Func => e.push(0x00),
            ExportKind::Table => e.push(0x01),
            ExportKind::Memory => e.push(0x02),
            ExportKind::Global => e.push(0x03),
            ExportKind::Tag => e.push(0x04),
        }
    }
}

impl Encode for Elem<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match (&self.kind, &self.payload) {
            (
                ElemKind::Active {
                    table: Index::Num(0, _),
                    offset,
                },
                ElemPayload::Indices(_),
            ) => {
                e.push(0x00);
                offset.encode(e, None);
            }
            (ElemKind::Passive, ElemPayload::Indices(_)) => {
                e.push(0x01); // flags
                e.push(0x00); // extern_kind
            }
            (ElemKind::Active { table, offset }, ElemPayload::Indices(_)) => {
                e.push(0x02); // flags
                table.encode(e);
                offset.encode(e, None);
                e.push(0x00); // extern_kind
            }
            (ElemKind::Declared, ElemPayload::Indices(_)) => {
                e.push(0x03); // flags
                e.push(0x00); // extern_kind
            }
            (
                ElemKind::Active {
                    table: Index::Num(0, _),
                    offset,
                },
                ElemPayload::Exprs {
                    ty:
                        RefType {
                            nullable: true,
                            heap:
                                HeapType::Abstract {
                                    shared: false,
                                    ty: AbstractHeapType::Func,
                                },
                        },
                    ..
                },
            ) => {
                e.push(0x04);
                offset.encode(e, None);
            }
            (ElemKind::Passive, ElemPayload::Exprs { ty, .. }) => {
                e.push(0x05);
                ty.encode(e);
            }
            (ElemKind::Active { table, offset }, ElemPayload::Exprs { ty, .. }) => {
                e.push(0x06);
                table.encode(e);
                offset.encode(e, None);
                ty.encode(e);
            }
            (ElemKind::Declared, ElemPayload::Exprs { ty, .. }) => {
                e.push(0x07); // flags
                ty.encode(e);
            }
        }

        self.payload.encode(e);
    }
}

impl Encode for ElemPayload<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            ElemPayload::Indices(v) => v.encode(e),
            ElemPayload::Exprs { exprs, ty: _ } => {
                exprs.len().encode(e);
                for expr in exprs {
                    expr.encode(e, None);
                }
            }
        }
    }
}

impl Encode for Data<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match &self.kind {
            DataKind::Passive => e.push(0x01),
            DataKind::Active {
                memory: Index::Num(0, _),
                offset,
            } => {
                e.push(0x00);
                offset.encode(e, None);
            }
            DataKind::Active { memory, offset } => {
                e.push(0x02);
                memory.encode(e);
                offset.encode(e, None);
            }
        }
        self.data.iter().map(|l| l.len()).sum::<usize>().encode(e);
        for val in self.data.iter() {
            val.push_onto(e);
        }
    }
}

impl Func<'_> {
    /// Encodes the function into `e` while returning all branch hints with
    /// known relative offsets after encoding.
    ///
    /// The `dwarf` field is optional and used to track debugging information
    /// for each instruction.
    fn encode(&self, e: &mut Vec<u8>, mut dwarf: Option<&mut dwarf::Dwarf>) -> Vec<BranchHint> {
        assert!(self.exports.names.is_empty());
        let (expr, locals) = match &self.kind {
            FuncKind::Inline { expression, locals } => (expression, locals),
            _ => panic!("should only have inline functions in emission"),
        };

        if let Some(dwarf) = &mut dwarf {
            let index = match self.ty.index.as_ref().unwrap() {
                Index::Num(n, _) => *n,
                _ => unreachable!(),
            };
            dwarf.start_func(self.span, index, locals);
        }

        // Encode the function into a temporary vector because functions are
        // prefixed with their length. The temporary vector, when encoded,
        // encodes its length first then the body.
        let mut tmp = Vec::new();
        locals.encode(&mut tmp);
        let branch_hints = expr.encode(&mut tmp, dwarf.as_deref_mut());
        tmp.encode(e);

        if let Some(dwarf) = &mut dwarf {
            dwarf.end_func(tmp.len(), e.len());
        }

        branch_hints
    }
}

impl Encode for Box<[Local<'_>]> {
    fn encode(&self, e: &mut Vec<u8>) {
        let mut locals_compressed = Vec::<(u32, ValType)>::new();
        for local in self.iter() {
            if let Some((cnt, prev)) = locals_compressed.last_mut() {
                if *prev == local.ty {
                    *cnt += 1;
                    continue;
                }
            }
            locals_compressed.push((1, local.ty));
        }
        locals_compressed.encode(e);
    }
}

impl Expression<'_> {
    /// Encodes this expression into `e` and optionally tracks debugging
    /// information for each instruction in `dwarf`.
    ///
    /// Returns all branch hints, if any, found while parsing this function.
    fn encode(&self, e: &mut Vec<u8>, mut dwarf: Option<&mut dwarf::Dwarf>) -> Vec<BranchHint> {
        let mut hints = Vec::with_capacity(self.branch_hints.len());
        let mut next_hint = self.branch_hints.iter().peekable();

        for (i, instr) in self.instrs.iter().enumerate() {
            // Branch hints are stored in order of increasing `instr_index` so
            // check to see if the next branch hint matches this instruction's
            // index.
            if let Some(hint) = next_hint.next_if(|h| h.instr_index == i) {
                hints.push(BranchHint {
                    branch_func_offset: u32::try_from(e.len()).unwrap(),
                    branch_hint_value: hint.value,
                });
            }

            // If DWARF is enabled then track this instruction's binary offset
            // and source location.
            if let Some(dwarf) = &mut dwarf {
                if let Some(span) = self.instr_spans.as_ref().map(|s| s[i]) {
                    dwarf.instr(e.len(), span);
                }
            }

            // Finally emit the instruction and move to the next.
            instr.encode(e);
        }
        e.push(0x0b);

        hints
    }
}

impl Encode for BlockType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        // block types using an index are encoded as an sleb, not a uleb
        if let Some(Index::Num(n, _)) = &self.ty.index {
            return i64::from(*n).encode(e);
        }
        let ty = self
            .ty
            .inline
            .as_ref()
            .expect("function type not filled in");
        if ty.params.is_empty() && ty.results.is_empty() {
            return e.push(0x40);
        }
        if ty.params.is_empty() && ty.results.len() == 1 {
            return ty.results[0].encode(e);
        }
        panic!("multi-value block types should have an index");
    }
}

impl Encode for LaneArg {
    fn encode(&self, e: &mut Vec<u8>) {
        self.lane.encode(e);
    }
}

impl Encode for MemArg<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match &self.memory {
            Index::Num(0, _) => {
                self.align.trailing_zeros().encode(e);
                self.offset.encode(e);
            }
            _ => {
                (self.align.trailing_zeros() | (1 << 6)).encode(e);
                self.memory.encode(e);
                self.offset.encode(e);
            }
        }
    }
}

impl Encode for Ordering {
    fn encode(&self, buf: &mut Vec<u8>) {
        let flag: u8 = match self {
            Ordering::SeqCst => 0,
            Ordering::AcqRel => 1,
        };
        flag.encode(buf);
    }
}

impl<T> Encode for Ordered<T>
where
    T: Encode,
{
    fn encode(&self, buf: &mut Vec<u8>) {
        self.ordering.encode(buf);
        self.inner.encode(buf);
    }
}

impl Encode for LoadOrStoreLane<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.memarg.encode(e);
        self.lane.encode(e);
    }
}

impl Encode for CallIndirect<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.ty.encode(e);
        self.table.encode(e);
    }
}

impl Encode for TableInit<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.elem.encode(e);
        self.table.encode(e);
    }
}

impl Encode for TableCopy<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.dst.encode(e);
        self.src.encode(e);
    }
}

impl Encode for TableArg<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.dst.encode(e);
    }
}

impl Encode for MemoryArg<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.mem.encode(e);
    }
}

impl Encode for MemoryInit<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.data.encode(e);
        self.mem.encode(e);
    }
}

impl Encode for MemoryCopy<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.dst.encode(e);
        self.src.encode(e);
    }
}

impl Encode for BrTableIndices<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.labels.encode(e);
        self.default.encode(e);
    }
}

impl Encode for F32 {
    fn encode(&self, e: &mut Vec<u8>) {
        e.extend_from_slice(&self.bits.to_le_bytes());
    }
}

impl Encode for F64 {
    fn encode(&self, e: &mut Vec<u8>) {
        e.extend_from_slice(&self.bits.to_le_bytes());
    }
}

#[derive(Default)]
struct Names<'a> {
    module: Option<&'a str>,
    funcs: Vec<(u32, &'a str)>,
    func_idx: u32,
    locals: Vec<(u32, Vec<(u32, &'a str)>)>,
    labels: Vec<(u32, Vec<(u32, &'a str)>)>,
    globals: Vec<(u32, &'a str)>,
    global_idx: u32,
    memories: Vec<(u32, &'a str)>,
    memory_idx: u32,
    tables: Vec<(u32, &'a str)>,
    table_idx: u32,
    tags: Vec<(u32, &'a str)>,
    tag_idx: u32,
    types: Vec<(u32, &'a str)>,
    type_idx: u32,
    data: Vec<(u32, &'a str)>,
    data_idx: u32,
    elems: Vec<(u32, &'a str)>,
    elem_idx: u32,
    fields: Vec<(u32, Vec<(u32, &'a str)>)>,
}

fn find_names<'a>(
    module_id: &Option<Id<'a>>,
    module_name: &Option<NameAnnotation<'a>>,
    fields: &[ModuleField<'a>],
) -> Names<'a> {
    fn get_name<'a>(id: &Option<Id<'a>>, name: &Option<NameAnnotation<'a>>) -> Option<&'a str> {
        name.as_ref().map(|n| n.name).or(id.and_then(|id| {
            if id.is_gensym() {
                None
            } else {
                Some(id.name())
            }
        }))
    }

    enum Name {
        Type,
        Global,
        Func,
        Memory,
        Table,
        Tag,
        Elem,
        Data,
    }

    let mut ret = Names {
        module: get_name(module_id, module_name),
        ..Default::default()
    };
    let mut names = Vec::new();
    for field in fields {
        // Extract the kind/id/name from whatever kind of field this is...
        let (kind, id, name) = match field {
            ModuleField::Import(i) => (
                match i.item.kind {
                    ItemKind::Func(_) => Name::Func,
                    ItemKind::Table(_) => Name::Table,
                    ItemKind::Memory(_) => Name::Memory,
                    ItemKind::Global(_) => Name::Global,
                    ItemKind::Tag(_) => Name::Tag,
                },
                &i.item.id,
                &i.item.name,
            ),
            ModuleField::Global(g) => (Name::Global, &g.id, &g.name),
            ModuleField::Table(t) => (Name::Table, &t.id, &t.name),
            ModuleField::Memory(m) => (Name::Memory, &m.id, &m.name),
            ModuleField::Tag(t) => (Name::Tag, &t.id, &t.name),
            ModuleField::Type(t) => (Name::Type, &t.id, &t.name),
            ModuleField::Rec(r) => {
                for ty in &r.types {
                    names.push((Name::Type, &ty.id, &ty.name, field));
                }
                continue;
            }
            ModuleField::Elem(e) => (Name::Elem, &e.id, &e.name),
            ModuleField::Data(d) => (Name::Data, &d.id, &d.name),
            ModuleField::Func(f) => (Name::Func, &f.id, &f.name),
            ModuleField::Export(_) | ModuleField::Start(_) | ModuleField::Custom(_) => continue,
        };
        names.push((kind, id, name, field));
    }

    for (kind, id, name, field) in names {
        // .. and using the kind we can figure out where to place this name
        let (list, idx) = match kind {
            Name::Func => (&mut ret.funcs, &mut ret.func_idx),
            Name::Table => (&mut ret.tables, &mut ret.table_idx),
            Name::Memory => (&mut ret.memories, &mut ret.memory_idx),
            Name::Global => (&mut ret.globals, &mut ret.global_idx),
            Name::Tag => (&mut ret.tags, &mut ret.tag_idx),
            Name::Type => (&mut ret.types, &mut ret.type_idx),
            Name::Elem => (&mut ret.elems, &mut ret.elem_idx),
            Name::Data => (&mut ret.data, &mut ret.data_idx),
        };
        if let Some(name) = get_name(id, name) {
            list.push((*idx, name));
        }

        // Handle module locals separately from above
        if let ModuleField::Func(f) = field {
            let mut local_names = Vec::new();
            let mut label_names = Vec::new();
            let mut local_idx = 0;
            let mut label_idx = 0;

            // Consult the inline type listed for local names of parameters.
            // This is specifically preserved during the name resolution
            // pass, but only for functions, so here we can look at the
            // original source's names.
            if let Some(ty) = &f.ty.inline {
                for (id, name, _) in ty.params.iter() {
                    if let Some(name) = get_name(id, name) {
                        local_names.push((local_idx, name));
                    }
                    local_idx += 1;
                }
            }
            if let FuncKind::Inline {
                locals, expression, ..
            } = &f.kind
            {
                for local in locals.iter() {
                    if let Some(name) = get_name(&local.id, &local.name) {
                        local_names.push((local_idx, name));
                    }
                    local_idx += 1;
                }

                for i in expression.instrs.iter() {
                    match i {
                        Instruction::If(block)
                        | Instruction::Block(block)
                        | Instruction::Loop(block)
                        | Instruction::Try(block)
                        | Instruction::TryTable(TryTable { block, .. }) => {
                            if let Some(name) = get_name(&block.label, &block.label_name) {
                                label_names.push((label_idx, name));
                            }
                            label_idx += 1;
                        }
                        _ => {}
                    }
                }
            }
            if !local_names.is_empty() {
                ret.locals.push((*idx, local_names));
            }
            if !label_names.is_empty() {
                ret.labels.push((*idx, label_names));
            }
        }

        // Handle struct fields separately from above
        if let ModuleField::Type(ty) = field {
            let mut field_names = vec![];
            match &ty.def.kind {
                InnerTypeKind::Func(_) | InnerTypeKind::Array(_) => {}
                InnerTypeKind::Struct(ty_struct) => {
                    for (idx, field) in ty_struct.fields.iter().enumerate() {
                        if let Some(name) = get_name(&field.id, &None) {
                            field_names.push((idx as u32, name))
                        }
                    }
                }
            }
            if !field_names.is_empty() {
                ret.fields.push((*idx, field_names))
            }
        }

        *idx += 1;
    }

    ret
}

impl Names<'_> {
    fn is_empty(&self) -> bool {
        self.module.is_none()
            && self.funcs.is_empty()
            && self.locals.is_empty()
            && self.labels.is_empty()
            && self.globals.is_empty()
            && self.memories.is_empty()
            && self.tables.is_empty()
            && self.types.is_empty()
            && self.elems.is_empty()
            && self.data.is_empty()
            && self.fields.is_empty()
            && self.tags.is_empty()
        // NB: specifically don't check modules/instances since they're
        // not encoded for now.
    }
}

impl Encode for Names<'_> {
    fn encode(&self, dst: &mut Vec<u8>) {
        let mut tmp = Vec::new();

        let mut subsec = |id: u8, data: &mut Vec<u8>| {
            dst.push(id);
            data.encode(dst);
            data.truncate(0);
        };

        if let Some(id) = self.module {
            id.encode(&mut tmp);
            subsec(0, &mut tmp);
        }
        if !self.funcs.is_empty() {
            self.funcs.encode(&mut tmp);
            subsec(1, &mut tmp);
        }
        if !self.locals.is_empty() {
            self.locals.encode(&mut tmp);
            subsec(2, &mut tmp);
        }
        if !self.labels.is_empty() {
            self.labels.encode(&mut tmp);
            subsec(3, &mut tmp);
        }
        if !self.types.is_empty() {
            self.types.encode(&mut tmp);
            subsec(4, &mut tmp);
        }
        if !self.tables.is_empty() {
            self.tables.encode(&mut tmp);
            subsec(5, &mut tmp);
        }
        if !self.memories.is_empty() {
            self.memories.encode(&mut tmp);
            subsec(6, &mut tmp);
        }
        if !self.globals.is_empty() {
            self.globals.encode(&mut tmp);
            subsec(7, &mut tmp);
        }
        if !self.elems.is_empty() {
            self.elems.encode(&mut tmp);
            subsec(8, &mut tmp);
        }
        if !self.data.is_empty() {
            self.data.encode(&mut tmp);
            subsec(9, &mut tmp);
        }
        if !self.fields.is_empty() {
            self.fields.encode(&mut tmp);
            subsec(10, &mut tmp);
        }
        if !self.tags.is_empty() {
            self.tags.encode(&mut tmp);
            subsec(11, &mut tmp);
        }
    }
}

impl Encode for Id<'_> {
    fn encode(&self, dst: &mut Vec<u8>) {
        assert!(!self.is_gensym());
        self.name().encode(dst);
    }
}

impl Encode for TryTable<'_> {
    fn encode(&self, dst: &mut Vec<u8>) {
        self.block.encode(dst);
        self.catches.encode(dst);
    }
}

impl Encode for TryTableCatch<'_> {
    fn encode(&self, dst: &mut Vec<u8>) {
        let flag_byte: u8 = match self.kind {
            TryTableCatchKind::Catch(..) => 0,
            TryTableCatchKind::CatchRef(..) => 1,
            TryTableCatchKind::CatchAll => 2,
            TryTableCatchKind::CatchAllRef => 3,
        };
        flag_byte.encode(dst);
        match self.kind {
            TryTableCatchKind::Catch(tag) | TryTableCatchKind::CatchRef(tag) => {
                tag.encode(dst);
            }
            TryTableCatchKind::CatchAll | TryTableCatchKind::CatchAllRef => {}
        }
        self.label.encode(dst);
    }
}

impl Encode for V128Const {
    fn encode(&self, dst: &mut Vec<u8>) {
        dst.extend_from_slice(&self.to_le_bytes());
    }
}

impl Encode for I8x16Shuffle {
    fn encode(&self, dst: &mut Vec<u8>) {
        dst.extend_from_slice(&self.lanes);
    }
}

impl Encode for SelectTypes<'_> {
    fn encode(&self, dst: &mut Vec<u8>) {
        match &self.tys {
            Some(list) => {
                dst.push(0x1c);
                list.encode(dst);
            }
            None => dst.push(0x1b),
        }
    }
}

impl Encode for Custom<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            Custom::Raw(r) => r.encode(e),
            Custom::Producers(p) => p.encode(e),
            Custom::Dylink0(p) => p.encode(e),
        }
    }
}

impl Encode for RawCustomSection<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        for list in self.data.iter() {
            e.extend_from_slice(list);
        }
    }
}

impl Encode for Producers<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.fields.encode(e);
    }
}

impl Encode for Dylink0<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        for section in self.subsections.iter() {
            e.push(section.id());
            let mut tmp = Vec::new();
            section.encode(&mut tmp);
            tmp.encode(e);
        }
    }
}

impl Encode for Dylink0Subsection<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            Dylink0Subsection::MemInfo {
                memory_size,
                memory_align,
                table_size,
                table_align,
            } => {
                memory_size.encode(e);
                memory_align.encode(e);
                table_size.encode(e);
                table_align.encode(e);
            }
            Dylink0Subsection::Needed(libs) => libs.encode(e),
            Dylink0Subsection::ExportInfo(list) => list.encode(e),
            Dylink0Subsection::ImportInfo(list) => list.encode(e),
        }
    }
}

struct FunctionBranchHints {
    func_index: u32,
    hints: Vec<BranchHint>,
}

struct BranchHint {
    branch_func_offset: u32,
    branch_hint_value: u32,
}

impl Encode for FunctionBranchHints {
    fn encode(&self, e: &mut Vec<u8>) {
        self.func_index.encode(e);
        self.hints.encode(e);
    }
}

impl Encode for BranchHint {
    fn encode(&self, e: &mut Vec<u8>) {
        self.branch_func_offset.encode(e);
        1u32.encode(e);
        self.branch_hint_value.encode(e);
    }
}

impl Encode for Tag<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.ty.encode(e);
        match &self.kind {
            TagKind::Inline() => {}
            _ => panic!("TagKind should be inline during encoding"),
        }
    }
}

impl Encode for TagType<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        match self {
            TagType::Exception(ty) => {
                e.push(0x00);
                ty.encode(e);
            }
        }
    }
}

impl Encode for StructAccess<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.r#struct.encode(e);
        self.field.encode(e);
    }
}

impl Encode for ArrayFill<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.array.encode(e);
    }
}

impl Encode for ArrayCopy<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.dest_array.encode(e);
        self.src_array.encode(e);
    }
}

impl Encode for ArrayInit<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.array.encode(e);
        self.segment.encode(e);
    }
}

impl Encode for ArrayNewFixed<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.array.encode(e);
        self.length.encode(e);
    }
}

impl Encode for ArrayNewData<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.array.encode(e);
        self.data_idx.encode(e);
    }
}

impl Encode for ArrayNewElem<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        self.array.encode(e);
        self.elem_idx.encode(e);
    }
}

impl Encode for RefTest<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        e.push(0xfb);
        if self.r#type.nullable {
            e.push(0x15);
        } else {
            e.push(0x14);
        }
        self.r#type.heap.encode(e);
    }
}

impl Encode for RefCast<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        e.push(0xfb);
        if self.r#type.nullable {
            e.push(0x17);
        } else {
            e.push(0x16);
        }
        self.r#type.heap.encode(e);
    }
}

fn br_on_cast_flags(from_nullable: bool, to_nullable: bool) -> u8 {
    let mut flag = 0;
    if from_nullable {
        flag |= 1 << 0;
    }
    if to_nullable {
        flag |= 1 << 1;
    }
    flag
}

impl Encode for BrOnCast<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        e.push(0xfb);
        e.push(0x18);
        e.push(br_on_cast_flags(
            self.from_type.nullable,
            self.to_type.nullable,
        ));
        self.label.encode(e);
        self.from_type.heap.encode(e);
        self.to_type.heap.encode(e);
    }
}

impl Encode for BrOnCastFail<'_> {
    fn encode(&self, e: &mut Vec<u8>) {
        e.push(0xfb);
        e.push(0x19);
        e.push(br_on_cast_flags(
            self.from_type.nullable,
            self.to_type.nullable,
        ));
        self.label.encode(e);
        self.from_type.heap.encode(e);
        self.to_type.heap.encode(e);
    }
}
