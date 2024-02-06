use alloc::collections::BTreeMap;
use cranelift_codegen::gimli;
use wasmparser::{NameSectionReader, NameSubsection};

type GimliSlice<'a> = gimli::EndianSlice<'a, gimli::LittleEndian>;

#[derive(Debug, Default)]
pub struct DebugInfo<'wasm> {
    pub producers: Option<&'wasm str>,
    pub names: NameSection<'wasm>,
    pub dwarf: gimli::Dwarf<GimliSlice<'wasm>>,
    pub debug_loc: gimli::DebugLoc<GimliSlice<'wasm>>,
    debug_loclists: gimli::DebugLocLists<GimliSlice<'wasm>>,
    pub debug_ranges: gimli::DebugRanges<GimliSlice<'wasm>>,
    pub debug_rnglists: gimli::DebugRngLists<GimliSlice<'wasm>>,
}

#[derive(Debug, Default)]
pub struct NameSection<'wasm> {
    pub module: Option<&'wasm str>,
    pub func: BTreeMap<u32, &'wasm str>,
    pub locals: BTreeMap<u32, BTreeMap<u32, &'wasm str>>,
    pub labels: BTreeMap<u32, BTreeMap<u32, &'wasm str>>,
    pub table: BTreeMap<u32, &'wasm str>,
    pub memory: BTreeMap<u32, &'wasm str>,
    pub global: BTreeMap<u32, &'wasm str>,
    pub elem: BTreeMap<u32, &'wasm str>,
    pub data: BTreeMap<u32, &'wasm str>,
}

pub fn handle_name_section<'wasm>(
    info: &mut DebugInfo<'wasm>,
    custom_section: wasmparser::CustomSection<'wasm>,
) -> crate::Result<()> {
    fn parse_name_map_into<'a>(
        reader: wasmparser::VecReader<'a, wasmparser::Naming<'a>>,
        dst: &mut BTreeMap<u32, &'a str>,
    ) -> crate::Result<()> {
        for pair in reader.iter() {
            let naming = pair?;
            dst.insert(naming.index, naming.name);
        }
        Ok(())
    }

    fn parse_indirect_name_map_into<'a>(
        reader: wasmparser::VecReader<'a, wasmparser::IndirectNaming<'a>>,
        dst: &mut BTreeMap<u32, BTreeMap<u32, &'a str>>,
    ) -> crate::Result<()> {
        for pair in reader.iter() {
            let naming = pair?;
            let mut map = BTreeMap::new();

            for pair in naming.names.iter() {
                let naming = pair?;
                map.insert(naming.index, naming.name);
            }

            dst.insert(naming.index, map);
        }
        Ok(())
    }

    let reader = NameSectionReader::new(custom_section.bytes);
    let names = &mut info.names;

    for subsection in reader.subsections() {
        match subsection? {
            NameSubsection::Module(name) => names.module = Some(name),
            NameSubsection::Function(r) => parse_name_map_into(r, &mut names.func)?,
            NameSubsection::Local(r) => parse_indirect_name_map_into(r, &mut names.locals)?,
            NameSubsection::Label(r) => parse_indirect_name_map_into(r, &mut names.labels)?,
            NameSubsection::Table(r) => parse_name_map_into(r, &mut names.table)?,
            NameSubsection::Memory(r) => parse_name_map_into(r, &mut names.memory)?,
            NameSubsection::Global(r) => parse_name_map_into(r, &mut names.global)?,
            NameSubsection::Elem(r) => parse_name_map_into(r, &mut names.elem)?,
            NameSubsection::Data(r) => parse_name_map_into(r, &mut names.data)?,
        }
    }

    Ok(())
}

pub fn handle_debug_section<'wasm>(
    info: &mut DebugInfo<'wasm>,
    sec: wasmparser::CustomSection<'wasm>,
) -> crate::Result<()> {
    let dwarf = &mut info.dwarf;
    let endian = gimli::LittleEndian;
    let data = sec.bytes;

    match sec.name {
        "producers" => {
            info.producers = Some(core::str::from_utf8(sec.bytes)?);
        }
        "target_features" => {
            log::debug!("target features {}", core::str::from_utf8(sec.bytes)?);
        }

        ".debug_abbrev" => dwarf.debug_abbrev = gimli::DebugAbbrev::new(data, endian),
        ".debug_addr" => dwarf.debug_addr = gimli::DebugAddr::from(GimliSlice::new(data, endian)),
        ".debug_info" => dwarf.debug_info = gimli::DebugInfo::new(data, endian),
        ".debug_line" => dwarf.debug_line = gimli::DebugLine::new(data, endian),
        ".debug_line_str" => dwarf.debug_line_str = gimli::DebugLineStr::new(data, endian),
        ".debug_str_offsets" => {
            dwarf.debug_str_offsets = gimli::DebugStrOffsets::from(GimliSlice::new(data, endian))
        }
        ".debug_types" => dwarf.debug_types = gimli::DebugTypes::new(data, endian),
        ".debug_loc" => info.debug_loc = gimli::DebugLoc::new(data, endian),
        ".debug_loclists" => info.debug_loclists = gimli::DebugLocLists::new(data, endian),
        ".debug_ranges" => info.debug_ranges = gimli::DebugRanges::new(data, endian),
        ".debug_rnglists" => info.debug_rnglists = gimli::DebugRngLists::new(data, endian),
        _ => {}
    }

    Ok(())
}
