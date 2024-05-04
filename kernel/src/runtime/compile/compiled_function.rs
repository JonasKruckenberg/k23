use crate::runtime::{BuiltinFunctionIndex, FilePos};
use crate::runtime::{NS_WASM_BUILTIN, NS_WASM_FUNC};
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::ir::{ExternalName, StackSlots, UserExternalName, UserExternalNameRef};
use cranelift_codegen::{
    binemit, Final, FinalizedMachReloc, FinalizedRelocTarget, MachBufferFinalized,
    ValueLabelsRanges,
};
use cranelift_wasm::FuncIndex;

#[derive(Debug)]
pub struct CompiledFunction {
    /// The machine code buffer for this function.
    pub buffer: MachBufferFinalized<Final>,
    /// What names each name ref corresponds to.
    name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
    /// The alignment for the compiled function.
    pub alignment: u32,
    /// The metadata for the compiled function, including unwind information
    /// the function address map.
    pub metadata: CompiledFunctionMetadata,
}

#[derive(Debug, Default)]
pub struct CompiledFunctionMetadata {
    /// Mapping of value labels and their locations.
    pub value_labels_ranges: ValueLabelsRanges,
    /// Allocated stack slots.
    pub sized_stack_slots: StackSlots,
    /// Start source location.
    pub start_srcloc: FilePos,
    /// End source location.
    pub end_srcloc: FilePos,
}

#[derive(Debug)]
pub enum RelocationTarget {
    Wasm(FuncIndex),
    Builtin(BuiltinFunctionIndex),
}

#[derive(Debug)]
pub struct Relocation {
    pub kind: binemit::Reloc,
    pub target: RelocationTarget,
    pub addend: binemit::Addend,
    pub offset: binemit::CodeOffset,
}

impl CompiledFunction {
    pub fn new(
        buffer: MachBufferFinalized<Final>,
        name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
        alignment: u32,
    ) -> Self {
        Self {
            buffer,
            name_map,
            alignment,
            metadata: Default::default(),
        }
    }

    /// Returns an iterator to the function's relocation information.
    pub fn relocations(&self) -> impl Iterator<Item = Relocation> + '_ {
        self.buffer
            .relocs()
            .iter()
            .map(|r| mach_reloc_to_reloc(r, &self.name_map))
    }

    /// Get a reference to the compiled function metadata.
    pub fn metadata(&self) -> &CompiledFunctionMetadata {
        &self.metadata
    }
}

fn mach_reloc_to_reloc(
    reloc: &FinalizedMachReloc,
    name_map: &PrimaryMap<UserExternalNameRef, UserExternalName>,
) -> Relocation {
    let &FinalizedMachReloc {
        offset,
        kind,
        ref target,
        addend,
    } = reloc;

    let target = match *target {
        FinalizedRelocTarget::ExternalName(ExternalName::User(user_func_ref)) => {
            let name = &name_map[user_func_ref];
            match name.namespace {
                // A reference to another jit'ed WASM function
                NS_WASM_FUNC => RelocationTarget::Wasm(FuncIndex::from_u32(name.index)),
                // A reference to a WASM builtin
                NS_WASM_BUILTIN => {
                    RelocationTarget::Builtin(BuiltinFunctionIndex::from_u32(name.index))
                }
                _ => panic!("unknown namespace {}", name.namespace),
            }
        }
        FinalizedRelocTarget::ExternalName(ExternalName::LibCall(libcall)) => {
            // cranelift libcalls are a lot like wasm builtins, they are emitted for instructions
            // that have no ISA equivalent and would be too complicated to emit as JIT code
            todo!("libcalls {libcall:?}")
        }
        _ => panic!("unsupported relocation target {target:?}"),
    };

    Relocation {
        kind,
        target,
        addend,
        offset,
    }
}
