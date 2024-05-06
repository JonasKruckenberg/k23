use crate::runtime::compile::{CompiledFunctionInfo, CompiledModuleInfo};
use crate::runtime::instantiate::{CodeMemory, GuestVec, Store};
use crate::runtime::translate::EntityType;
use crate::runtime::vmcontext::VMContextOffsets;
use crate::runtime::{build_module, translate, Engine};
use alloc::sync::Arc;
use core::ops::Range;
use cranelift_wasm::DefinedFuncIndex;

#[derive(Debug)]
pub struct Module<'wasm> {
    pub info: CompiledModuleInfo<'wasm>,
    pub code: Arc<CodeMemory>,
    pub offsets: VMContextOffsets,
}

pub struct ImportType<'wasm> {
    pub module: &'wasm str,
    pub field: &'wasm str,
    pub ty: EntityType,
}

pub struct ExportType<'wasm> {
    pub name: &'wasm str,
    pub ty: EntityType,
}

impl<'wasm> Module<'wasm> {
    pub fn from_bytes(engine: &Engine, store: &Store, bytes: &'wasm [u8]) -> Self {
        let mut guest_vec = GuestVec::new(store.allocator());
        let info = build_module(engine, bytes, &mut guest_vec).unwrap();
        log::trace!("compile output {:?}", guest_vec.as_ptr_range());

        let mut code = CodeMemory::new(guest_vec);
        code.publish().unwrap();

        Self {
            offsets: VMContextOffsets::for_module(engine.compiler().target_isa(), &info.module),
            info,
            code: Arc::new(code),
        }
    }

    pub fn imports(&self) -> Imports<'wasm, '_> {
        Imports {
            inner: self.info.module.imports(),
        }
    }

    pub fn exports(&self) -> Exports<'wasm, '_> {
        Exports {
            inner: self.info.module.exports(),
        }
    }

    pub fn image_range(&self) -> Range<*const u8> {
        self.code.as_slice().as_ptr_range()
    }
    pub fn text(&self) -> &[u8] {
        self.code.as_slice()
    }
    pub fn function_locations(&self) -> FunctionLocations {
        FunctionLocations {
            inner: self.info.funcs.iter(),
        }
    }
}

pub struct Imports<'wasm, 'module> {
    inner: translate::Imports<'wasm, 'module>,
}

impl<'wasm, 'module> Iterator for Imports<'wasm, 'module> {
    type Item = ImportType<'wasm>;

    fn next(&mut self) -> Option<Self::Item> {
        let (module, field, ty) = self.inner.next()?;

        Some(ImportType { module, field, ty })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'wasm, 'module> ExactSizeIterator for Imports<'wasm, 'module> {}

pub struct Exports<'wasm, 'module> {
    inner: translate::Exports<'wasm, 'module>,
}

impl<'wasm, 'module> Iterator for Exports<'wasm, 'module> {
    type Item = ExportType<'wasm>;

    fn next(&mut self) -> Option<Self::Item> {
        let (name, ty) = self.inner.next()?;

        Some(ExportType { name, ty })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'wasm, 'module> ExactSizeIterator for Exports<'wasm, 'module> {}

pub struct FunctionLocations<'a> {
    inner: cranelift_codegen::entity::Iter<'a, DefinedFuncIndex, CompiledFunctionInfo>,
}

impl<'a> Iterator for FunctionLocations<'a> {
    type Item = (u32, u32);

    fn next(&mut self) -> Option<Self::Item> {
        let (_, info) = self.inner.next()?;

        Some((info.wasm_func_loc.start, info.wasm_func_loc.length))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> ExactSizeIterator for FunctionLocations<'a> {}
