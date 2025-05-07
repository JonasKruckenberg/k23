// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::utils::enum_accessors;
use cranelift_entity::entity_impl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeIndex(u32);
entity_impl!(TypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncIndex(u32);
entity_impl!(FuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedFuncIndex(u32);
entity_impl!(DefinedFuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableIndex(u32);
entity_impl!(TableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedTableIndex(u32);
entity_impl!(DefinedTableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryIndex(u32);
entity_impl!(MemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedMemoryIndex(u32);
entity_impl!(DefinedMemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnedMemoryIndex(u32);
entity_impl!(OwnedMemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalIndex(u32);
entity_impl!(GlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedGlobalIndex(u32);
entity_impl!(DefinedGlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedTagIndex(u32);
entity_impl!(DefinedTagIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemIndex(u32);
entity_impl!(ElemIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataIndex(u32);
entity_impl!(DataIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncRefIndex(u32);
entity_impl!(FuncRefIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalIndex(u32);
entity_impl!(LocalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldIndex(u32);
entity_impl!(FieldIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagIndex(u32);
entity_impl!(TagIndex);

/// A reference to a label in a function. Only used for associating label names.
///
/// NOTE: These indices are local to the function they used in, they are also
/// **not** the same as the depth of their block. This means you cant just go
/// and take the relative branch depth of a `br` instruction and the label stack
/// height to get the label index.
/// According to the proposal the labels are assigned indices in the order their
/// blocks appear in the code.
///
/// Source:
/// <https://github.com/WebAssembly/extended-name-section/blob/main/proposals/extended-name-section/Overview.md#label-names>
///
/// ALSO NOTE: No existing tooling appears to emit label names, so this just doesn't
/// appear in the wild probably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelIndex(u32);
entity_impl!(LabelIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityIndex {
    Function(FuncIndex),
    Table(TableIndex),
    Memory(MemoryIndex),
    Global(GlobalIndex),
    Tag(TagIndex),
}

impl EntityIndex {
    enum_accessors! {
        e
        (Function(FuncIndex) is_func func unwrap_func *e)
        (Table(TableIndex) is_table table unwrap_table *e)
        (Memory(MemoryIndex) is_memory memory unwrap_memory *e)
        (Global(GlobalIndex) is_global global unwrap_global *e)
        (Tag(TagIndex) is_tag tag unwrap_tag *e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleInternedTypeIndex(u32);
entity_impl!(ModuleInternedTypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleInternedRecGroupIndex(u32);
entity_impl!(ModuleInternedRecGroupIndex);

#[repr(transparent)] // Used directly by JIT code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VMSharedTypeIndex(u32);
entity_impl!(VMSharedTypeIndex);

impl Default for VMSharedTypeIndex {
    #[inline]
    fn default() -> Self {
        Self(u32::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecGroupRelativeTypeIndex(u32);
entity_impl!(RecGroupRelativeTypeIndex);

/// An index pointing to a type that is canonicalized either within just a `Module` (types start out this way),
/// an entire `Engine` (required for runtime type checks) or a `RecGroup`
/// (only used during hash-consing to get a stable representation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CanonicalizedTypeIndex {
    /// An index within an engine, therefore canonicalized among all modules
    /// that can share types with each other.
    Engine(VMSharedTypeIndex),

    /// An index within the current Wasm module, canonicalized within just this
    /// current module.
    Module(ModuleInternedTypeIndex),

    /// An index within the containing type's rec group. This is only used when
    /// hashing and canonicalizing rec groups, and should never appear outside
    /// of the engine's type registry.
    RecGroup(RecGroupRelativeTypeIndex),
}

impl From<VMSharedTypeIndex> for CanonicalizedTypeIndex {
    fn from(index: VMSharedTypeIndex) -> Self {
        Self::Engine(index)
    }
}
impl From<ModuleInternedTypeIndex> for CanonicalizedTypeIndex {
    fn from(index: ModuleInternedTypeIndex) -> Self {
        Self::Module(index)
    }
}
impl From<RecGroupRelativeTypeIndex> for CanonicalizedTypeIndex {
    fn from(index: RecGroupRelativeTypeIndex) -> Self {
        Self::RecGroup(index)
    }
}

impl CanonicalizedTypeIndex {
    enum_accessors! {
        e
        (Module(ModuleInternedTypeIndex) is_module_type_index as_module_type_index unwrap_module_type_index *e)
        (Engine(VMSharedTypeIndex) is_engine_type_index as_engine_type_index unwrap_engine_type_index *e)
        (RecGroup(RecGroupRelativeTypeIndex) is_rec_group_type_index as_rec_group_type_index unwrap_rec_group_type_index *e)
    }
}
