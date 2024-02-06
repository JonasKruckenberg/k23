use crate::debug::{handle_debug_section, handle_name_section, DebugInfo};
use crate::traits::{ModuleTranslationEnvironment, TargetEnvironment};
use alloc::vec::Vec;
use cranelift_codegen::ir;
use cranelift_codegen::ir::{types, AbiParam};
use cranelift_entity::EntityRef;
use wasmparser::{DataIdx, ElemIdx, FuncIdx, GlobalIdx, MemIdx, TableIdx, TypeIdx};

pub fn translate_module<'wasm>(
    module: wasmparser::Module<'wasm>,
    env: &mut dyn ModuleTranslationEnvironment<'wasm>,
) -> crate::Result<()> {
    use wasmparser::Section;

    let mut debug_info = DebugInfo::default();

    for section in module.sections() {
        match section? {
            Section::Type(types) => {
                env.reserve_types(types.len())?;
                for (raw_idx, ty) in types.iter().enumerate() {
                    env.declare_type(TypeIdx::new(raw_idx), ty?)?;
                }
            }
            Section::Import(imports) => {
                for import in imports.iter() {
                    env.declare_import(import?)?;
                }
            }
            Section::Function(funcs) => {
                env.reserve_functions(funcs.len())?;

                for (raw_func_idx, type_idx) in funcs.iter().enumerate() {
                    let ty = env.lookup_type(type_idx?);

                    let mut sig = ir::Signature::new(env.target_config().default_call_conv);
                    sig.params = ty
                        .params()?
                        .iter()
                        .map(|res| abi_param_from_type(res?, env))
                        .collect::<crate::Result<Vec<_>>>()?;
                    sig.returns = ty
                        .results()?
                        .iter()
                        .map(|res| abi_param_from_type(res?, env))
                        .collect::<crate::Result<Vec<_>>>()?;

                    env.declare_function(FuncIdx::new(raw_func_idx), sig)?;
                }
            }
            Section::Table(tables) => {
                for (raw_idx, table_type) in tables.iter().enumerate() {
                    env.declare_table(TableIdx::new(raw_idx), table_type?)?;
                }
            }
            Section::Memory(memories) => {
                for (raw_idx, mem_type) in memories.iter().enumerate() {
                    env.declare_memory(MemIdx::new(raw_idx), mem_type?)?;
                }
            }
            Section::Global(globals) => {
                env.reserve_globals(globals.len())?;
                for (raw_idx, global) in globals.iter().enumerate() {
                    env.declare_global(GlobalIdx::new(raw_idx), global?)?;
                }
            }
            Section::Export(exports) => {
                for export in exports.iter() {
                    env.declare_export(export?)?;
                }
            }
            Section::Start(func_idx) => env.declare_start_function(func_idx)?,
            Section::Element(elements) => {
                for (raw_idx, element) in elements.iter().enumerate() {
                    env.declare_table_element(ElemIdx::new(raw_idx), element?)?;
                }
            }
            Section::Code(func_bodies) => {
                for (raw_idx, body) in func_bodies.iter().enumerate() {
                    env.declare_function_body(FuncIdx::new(raw_idx), body?)?;
                }
            }
            Section::Data(segments) => {
                for (raw_idx, segment) in segments.iter().enumerate() {
                    env.declare_data_segment(DataIdx::new(raw_idx), segment?)?;
                }
            }
            Section::DataCount(_) => {}
            Section::Custom(sec) => {
                if sec.name == "name" {
                    handle_name_section(&mut debug_info, sec)?;
                } else {
                    handle_debug_section(&mut debug_info, sec)?;
                }
            }
        }
    }

    env.declare_debug_info(debug_info)?;

    Ok(())
}

fn abi_param_from_type(
    ty: wasmparser::ValueType,
    env: &dyn TargetEnvironment,
) -> crate::Result<AbiParam> {
    use wasmparser::ValueType;

    let ty = match ty {
        ValueType::I32 => types::I32,
        ValueType::I64 => types::I64,
        ValueType::F32 => types::F32,
        ValueType::F64 => types::F64,
        ValueType::V128 => todo!("simd"),
        ValueType::FuncRef | ValueType::ExternRef => env.target_config().pointer_type(),
    };

    Ok(AbiParam::new(ty))
}
