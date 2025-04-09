// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::Func;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::{GlobalType, HeapTypeInner, Mutability, ValType};
use crate::wasm::values::{Ref, Val};
use crate::wasm::vm::{ExportedGlobal, VMGlobalDefinition, VMGlobalImport, VmPtr};
use anyhow::{Context, bail};
use core::ptr;
use core::ptr::NonNull;

#[derive(Clone, Copy, Debug)]
pub struct Global(Stored<ExportedGlobal>);

impl Global {
    pub fn new(store: &mut StoreOpaque, ty: GlobalType, val: Val) -> crate::Result<Self> {
        val.ensure_matches_ty(store, ty.content())?;
        
        // Safety: we checked above that the types match
        let definition = unsafe {
            let vmval = val.to_vmval(store)?;

            let def = VMGlobalDefinition::from_vmval(store, ty.content().to_wasm_type(), vmval)?;
            store.add_host_global(def)
        };

        let stored = store.add_global(ExportedGlobal {
            definition,
            vmctx: None,
            global: ty.to_wasm_global(),
        });
        Ok(Self(stored))
    }

    pub fn ty(self, store: &StoreOpaque) -> GlobalType {
        let export = &store[self.0];
        GlobalType::from_wasm_global(store.engine(), &export.global)
    }

    pub fn get(&self, store: &mut StoreOpaque) -> Val {
        // Safety: TODO
        unsafe {
            let export = &store[self.0];
            let def = export.definition.as_ref();

            match self.ty(store).content() {
                ValType::I32 => Val::I32(*def.as_i32()),
                ValType::I64 => Val::I64(*def.as_i64()),
                ValType::F32 => Val::F32(*def.as_u32()),
                ValType::F64 => Val::F64(*def.as_u64()),
                ValType::V128 => Val::V128(def.get_u128()),
                ValType::Ref(ref_ty) => {
                    let reference: Ref = match ref_ty.heap_type().inner {
                        HeapTypeInner::Func | HeapTypeInner::ConcreteFunc(_) => {
                            Func::from_vm_func_ref(store, NonNull::new(def.as_func_ref()).unwrap())
                                .into()
                        }
                        HeapTypeInner::NoFunc => Ref::Func(None),
                        _ => todo!(),
                    };
                    reference.into()
                }
            }
        }
    }

    pub fn set(&self, store: &mut StoreOpaque, val: Val) -> crate::Result<()> {
        let global_ty = self.ty(store);
        if global_ty.mutability() != Mutability::Var {
            bail!("immutable global cannot be set");
        }
        val.ensure_matches_ty(store, global_ty.content())
            .context("type mismatch: attempt to set global to value of wrong type")?;

        // Safety: TODO
        unsafe {
            let def = store[self.0].definition.as_mut();
            match val {
                Val::I32(i) => *def.as_i32_mut() = i,
                Val::I64(i) => *def.as_i64_mut() = i,
                Val::F32(f) => *def.as_u32_mut() = f,
                Val::F64(f) => *def.as_u64_mut() = f,
                Val::V128(i) => def.set_u128(i),
                Val::FuncRef(f) => {
                    *def.as_func_ref_mut() =
                        f.map_or(ptr::null_mut(), |f| f.vm_func_ref(store).as_ptr());
                }
            }
        }

        Ok(())
    }

    pub(super) fn from_exported_global(store: &mut StoreOpaque, export: ExportedGlobal) -> Self {
        let stored = store.add_global(export);
        Self(stored)
    }
    pub(super) fn as_vmglobal_import(self, store: &mut StoreOpaque) -> VMGlobalImport {
        let export = &store[self.0];
        VMGlobalImport {
            from: VmPtr::from(export.definition),
        }
    }
}
