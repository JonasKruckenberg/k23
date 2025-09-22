// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::pin::Pin;
use core::ptr;
use core::ptr::NonNull;

use anyhow::{Context, bail};
use cfg_if::cfg_if;
use wasmparser::WasmFeatures;

use crate::store::{StoreOpaque, Stored};
use crate::types::HeapTypeInner;
use crate::utils::wasm_unsupported;
use crate::vm::{ExportedGlobal, VMGlobalImport};
use crate::{Func, GlobalType, Mutability, Ref, Val, ValType, vm};

#[derive(Clone, Copy, Debug)]
pub struct Global(Stored<vm::ExportedGlobal>);

impl Global {
    pub fn ty(self, store: &StoreOpaque) -> GlobalType {
        let export = store.get_global(self.0).unwrap();
        GlobalType::from_wasm(store.engine(), &export.global)
    }

    pub fn get(&self, store: Pin<&mut StoreOpaque>) -> crate::Result<Val> {
        // Safety: TODO
        unsafe {
            let export = store.get_global(self.0).unwrap();
            let def = export.definition.as_ref();

            match self.ty(&*store).content() {
                ValType::I32 => Ok(Val::I32(*def.as_i32())),
                ValType::I64 => Ok(Val::I64(*def.as_i64())),
                ValType::F32 => Ok(Val::F32(*def.as_u32())),
                ValType::F64 => Ok(Val::F64(*def.as_u64())),
                ValType::V128 => {
                    cfg_if! {
                        if #[cfg(feature = "simd")] {
                            Ok(Val::V128(def.get_u128()))
                        } else {
                            wasm_unsupported!(WasmFeatures::SIMD, "enable `simd` feature")
                        }
                    }
                }
                ValType::Ref(ref_ty) => {
                    let reference: Ref = match ref_ty.heap_type().inner() {
                        HeapTypeInner::Func | HeapTypeInner::ConcreteFunc(_) => {
                            Func::from_vm_func_ref(store, NonNull::new(def.as_func_ref()).unwrap())
                                .into()
                        }
                        HeapTypeInner::NoFunc => Ref::Func(None),
                        _ => todo!(),
                    };
                    Ok(reference.into())
                }
            }
        }
    }

    pub fn set(&self, mut store: Pin<&mut StoreOpaque>, val: Val) -> crate::Result<()> {
        let global_ty = self.ty(&*store);
        if global_ty.mutability() != Mutability::Var {
            bail!("immutable global cannot be set");
        }
        val.ensure_matches_ty(store.as_mut(), global_ty.content())
            .context("type mismatch: attempt to set global to value of wrong type")?;

        // Safety: TODO
        unsafe {
            let def = store
                .as_mut()
                .get_global_mut(self.0)
                .unwrap()
                .definition
                .as_mut();
            match val {
                Val::I32(i) => *def.as_i32_mut() = i,
                Val::I64(i) => *def.as_i64_mut() = i,
                Val::F32(f) => *def.as_u32_mut() = f,
                Val::F64(f) => *def.as_u64_mut() = f,
                #[cfg(feature = "simd")]
                Val::V128(i) => def.set_u128(i),
                Val::FuncRef(f) => {
                    *def.as_func_ref_mut() =
                        f.map_or(ptr::null_mut(), |f| f.vm_func_ref(&*store).as_ptr());
                }
            }
        }

        Ok(())
    }

    pub(crate) fn from_exported_global(
        store: Pin<&mut StoreOpaque>,
        export: ExportedGlobal,
    ) -> Self {
        let stored = store.add_global(export);
        Self(stored)
    }

    pub(crate) fn as_vmglobal_import(&self, store: Pin<&mut StoreOpaque>) -> VMGlobalImport {
        todo!()
    }
}
