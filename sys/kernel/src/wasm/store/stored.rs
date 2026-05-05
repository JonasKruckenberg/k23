// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

#[derive(Debug, Default)]
pub struct StoredData {
    pub(super) funcs: Vec<crate::wasm::func::FuncData>,
    pub(super) tables: Vec<crate::wasm::vm::ExportedTable>,
    pub(super) globals: Vec<crate::wasm::vm::ExportedGlobal>,
    pub(super) instances: Vec<crate::wasm::instance::InstanceData>,
    pub(super) memories: Vec<crate::wasm::vm::ExportedMemory>,
    pub(super) tags: Vec<crate::wasm::vm::ExportedTag>,
}

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $add:ident, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
        $(
            impl super::StoreOpaque {
                pub(in crate::wasm) fn $add(&mut self, val: $ty) -> Stored<$ty> {
                    let $bind = self;
                    let index = $field.len();
                    $field.push(val);
                    Stored::new(index)
                }

                pub(in crate::wasm) fn $has(&self, index: Stored<$ty>) -> bool {
                    let $bind = self;
                    $field.get(index.index).is_some()
                }

                pub(in crate::wasm) fn $get(&self, index: Stored<$ty>) -> Option<&$ty> {
                    let $bind = self;
                    $field.get(index.index)
                }

                pub(in crate::wasm) fn $get_mut(&mut self, index: Stored<$ty>) -> Option<&mut $ty> {
                    let $bind = self;
                    $field.get_mut(index.index)
                }
            }

            impl ::core::ops::Index<Stored<$ty>> for super::StoreOpaque {
                type Output = $ty;

                fn index(&self, index: Stored<$ty>) -> &Self::Output {
                    self.$get(index).unwrap()
                }
            }

            impl ::core::ops::IndexMut<Stored<$ty>> for super::StoreOpaque {
                fn index_mut(&mut self, index: Stored<$ty>) -> &mut Self::Output {
                    self.$get_mut(index).unwrap()
                }
            }
        )*
    };
}

stored_impls! {
    s
    (crate::wasm::instance::InstanceData, add_instance, has_instance, get_instance, get_instance_mut, s.stored.instances)
    (crate::wasm::func::FuncData, add_function, has_function, get_function, get_function_mut, s.stored.funcs)
    (crate::wasm::vm::ExportedTable, add_table, has_table, get_table, get_table_mut, s.stored.tables)
    (crate::wasm::vm::ExportedMemory, add_memory, has_memory, get_memory, get_memory_mut, s.stored.memories)
    (crate::wasm::vm::ExportedGlobal, add_global, has_global, get_global, get_global_mut, s.stored.globals)
    (crate::wasm::vm::ExportedTag, add_tag, has_tag, get_tag, get_tag_mut, s.stored.tags)
}

pub struct Stored<T> {
    index: usize,
    _m: PhantomData<T>,
}

impl<T> Stored<T> {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            _m: PhantomData,
        }
    }
}

impl<T> Clone for Stored<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Stored<T> {}

impl<T> fmt::Debug for Stored<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Stored").field(&self.index).finish()
    }
}
