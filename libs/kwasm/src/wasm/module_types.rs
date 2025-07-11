use crate::indices::{ModuleInternedRecGroupIndex, ModuleInternedTypeIndex};
use crate::wasm::type_convert::WasmparserTypeConverter;
use crate::wasm::{WasmCompositeTypeInner, WasmFuncType, TranslatedModule, WasmSubType};
use core::fmt;
use core::ops::Range;
use cranelift_entity::packed_option::PackedOption;
use cranelift_entity::{EntityRef, PrimaryMap, SecondaryMap};
use hashbrown::HashMap;
use wasmparser::{Validator, ValidatorId};

/// Types defined within a single WebAssembly module.
#[derive(Debug, Default)]
pub struct ModuleTypes {
    /// WASM types (functions for MVP as well as arrays and structs when the GC proposal is enabled).
    wasm_types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
    /// Recursion groups defined within this module (only used when the GC proposal is enabled).
    rec_groups: PrimaryMap<ModuleInternedRecGroupIndex, Range<ModuleInternedTypeIndex>>,
    /// Signatures of trampolines
    trampoline_types: SecondaryMap<ModuleInternedTypeIndex, PackedOption<ModuleInternedTypeIndex>>,
    /// Types that have already been interned.
    pub(super) seen_types: HashMap<wasmparser::types::CoreTypeId, ModuleInternedTypeIndex>,
}

impl fmt::Display for ModuleTypes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, ty) in self.wasm_types() {
            writeln!(f, "{index:?}: {ty}")?;
        }
        Ok(())
    }
}

impl ModuleTypes {
    /// Returns an iterator over all the WASM types (functions, arrays, and structs) defined in this module.
    pub fn wasm_types(&self) -> impl ExactSizeIterator<Item = (ModuleInternedTypeIndex, &WasmSubType)> {
        self.wasm_types.iter()
    }

    /// Returns the number of types WASM types defined in this module.
    pub fn len_types(&self) -> usize {
        self.wasm_types.len()
    }

    /// Get the WASM type specified by `index` if it exists.
    pub fn get_wasm_type(&self, ty: ModuleInternedTypeIndex) -> Option<&WasmSubType> {
        self.wasm_types.get(ty)
    }

    /// Get the elements within a defined recursion group.
    pub fn rec_group_elements(
        &self,
        rec_group: ModuleInternedRecGroupIndex,
    ) -> impl ExactSizeIterator<Item = ModuleInternedTypeIndex> + use<'_> {
        let range = &self.rec_groups[rec_group];
        (range.start.as_u32()..range.end.as_u32()).map(ModuleInternedTypeIndex::from_u32)
    }

    pub fn rec_groups(&self) -> impl ExactSizeIterator<Item = &'_ Range<ModuleInternedTypeIndex>> {
        self.rec_groups.values()
    }

    /// The trampoline function types that this module requires.
    ///
    /// Yields pairs of (1) a function type and (2) its associated trampoline
    /// type. They might be the same.
    pub fn trampoline_types(
        &self,
    ) -> impl Iterator<Item = (ModuleInternedTypeIndex, ModuleInternedTypeIndex)> + '_ {
        self.trampoline_types
            .iter()
            .filter_map(|(k, v)| v.expand().map(|v| (k, v)))
    }

    pub fn trampoline_type(&self, ty: ModuleInternedTypeIndex) -> ModuleInternedTypeIndex {
        debug_assert!(self.wasm_types[ty].is_func());
        self.trampoline_types[ty].unwrap()
    }
}

/// A recursion group that is currently being defined.
struct RecGroupInProgress {
    /// The index of this recursion group.
    rec_group_index: ModuleInternedRecGroupIndex,
    /// Index into the `wasm_types` list where this recursion group starts.
    start: ModuleInternedTypeIndex,
    /// Index into the `wasm_types` list where this recursion group ends.
    end: ModuleInternedTypeIndex,
}

pub struct ModuleTypesBuilder {
    /// The `wasmparser` validator ID this builder has been crated with. Mixing types from
    /// different validators since defined IDs are only unique within a single validator.
    validator_id: ValidatorId,
    /// The types being built.
    pub types: ModuleTypes,
    /// Recursion groups that have already interned.
    seen_rec_groups: HashMap<wasmparser::types::RecGroupId, ModuleInternedRecGroupIndex>,
    /// The recursion group currently being defined.
    rec_group_in_progress: Option<RecGroupInProgress>,
}

impl ModuleTypesBuilder {
    pub fn new(validator: &Validator) -> Self {
        Self {
            validator_id: validator.id(),
            types: ModuleTypes::default(),
            seen_rec_groups: HashMap::default(),
            rec_group_in_progress: None,
        }
    }

    /// Finish building the module types.
    pub fn finish(self) -> ModuleTypes {
        self.types
    }

    /// Define a new recursion group that we haven't already interned.
    fn define_new_rec_group(
        &mut self,
        module: &TranslatedModule,
        validator_types: wasmparser::types::TypesRef<'_>,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> ModuleInternedRecGroupIndex {
        self.start_rec_group(
            validator_types,
            validator_types.rec_group_elements(rec_group_id),
        );

        for id in validator_types.rec_group_elements(rec_group_id) {
            let ty = &validator_types[id];
            let wasm_ty = WasmparserTypeConverter::new(&self.types, module)
                .with_rec_group(validator_types, rec_group_id)
                .convert_sub_type(ty);
            self.wasm_sub_type_in_rec_group(id, wasm_ty);
        }

        let rec_group_index = self.end_rec_group(rec_group_id);

        rec_group_index
    }

    /// Start defining a new recursion group.
    fn start_rec_group(
        &mut self,
        validator_types: wasmparser::types::TypesRef<'_>,
        elems: impl ExactSizeIterator<Item = wasmparser::types::CoreTypeId>,
    ) {
        tracing::trace!("Starting rec group of length {}", elems.len());

        assert!(self.rec_group_in_progress.is_none());
        assert_eq!(validator_types.id(), self.validator_id);

        let len = elems.len();
        for (i, wasmparser_id) in elems.enumerate() {
            let interned = ModuleInternedTypeIndex::new(self.types.len_types() + i);
            tracing::trace!(
                "Reserving {interned:?} for {wasmparser_id:?} = {:?}",
                validator_types[wasmparser_id]
            );

            let old_entry = self.types.seen_types.insert(wasmparser_id, interned);
            debug_assert_eq!(
                old_entry, None,
                "should not have already inserted {wasmparser_id:?}"
            );
        }

        self.rec_group_in_progress = Some(RecGroupInProgress {
            rec_group_index: self.next_rec_group_index(),
            start: self.next_type_index(),
            end: ModuleInternedTypeIndex::new(self.types.len_types() + len),
        });
    }

    /// Finish defining a recursion group returning it's index.
    fn end_rec_group(
        &mut self,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> ModuleInternedRecGroupIndex {
        let RecGroupInProgress {
            rec_group_index,
            start,
            end,
        } = self
            .rec_group_in_progress
            .take()
            .expect("should be defining a rec group");

        tracing::trace!("Ending rec group {start:?}..{end:?}");

        debug_assert!(start.index() < self.types.len_types());
        debug_assert_eq!(
            end,
            self.next_type_index(),
            "should have defined the number of types declared in `start_rec_group`"
        );

        let idx = self.push_rec_group(Range::from(start..end));
        debug_assert_eq!(idx, rec_group_index);

        self.seen_rec_groups.insert(rec_group_id, rec_group_index);
        rec_group_index
    }

    /// Define a new type within the current recursion group.
    fn wasm_sub_type_in_rec_group(&mut self, id: wasmparser::types::CoreTypeId, ty: WasmSubType) {
        assert!(
            self.rec_group_in_progress.is_some(),
            "must be defining a rec group to define new types"
        );

        let module_interned_index = self.push_type(ty);
        debug_assert_eq!(
            self.types.seen_types.get(&id),
            Some(&module_interned_index),
            "should have reserved the right module-interned index for this wasmparser type already"
        );
    }

    /// Define a new recursion group, or return the existing one's index if it's already been defined.
    pub fn intern_rec_group(
        &mut self,
        module: &TranslatedModule,
        validator_types: wasmparser::types::TypesRef<'_>,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> ModuleInternedRecGroupIndex {
        assert_eq!(validator_types.id(), self.validator_id);

        if let Some(interned) = self.seen_rec_groups.get(&rec_group_id) {
            return *interned;
        }

        self.define_new_rec_group(module, validator_types, rec_group_id)
    }

    /// Returns the next return value of `push_rec_group`.
    fn next_rec_group_index(&self) -> ModuleInternedRecGroupIndex {
        self.types.rec_groups.next_key()
    }

    /// Returns the next return value of `push`.
    pub fn next_ty(&self) -> ModuleInternedTypeIndex {
        self.types.wasm_types.next_key()
    }

    /// Adds a new recursion group.
    pub fn push_rec_group(
        &mut self,
        range: Range<ModuleInternedTypeIndex>,
    ) -> ModuleInternedRecGroupIndex {
        self.types.rec_groups.push(range)
    }

    /// Returns the next return value of `push_type`.
    fn next_type_index(&self) -> ModuleInternedTypeIndex {
        self.types.wasm_types.next_key()
    }

    /// Adds a new type to this interned list of types.
    fn push_type(&mut self, wasm_sub_type: WasmSubType) -> ModuleInternedTypeIndex {
        self.types.wasm_types.push(wasm_sub_type)
    }
}
