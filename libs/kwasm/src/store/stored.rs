// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use crate::vm;

#[derive(Debug, Default)]
pub struct StoredData {
    pub(super) instances: Vec<crate::instance::InstanceData>,
    pub(super) functions: Vec<crate::func::FuncData>,
    pub(super) tables: Vec<vm::ExportedTable>,
    pub(super) memories: Vec<vm::ExportedMemory>,
    pub(super) globals: Vec<vm::ExportedGlobal>,
    pub(super) tags: Vec<vm::ExportedTag>,
}

pub struct Stored<T> {
    index: usize,
    _m: PhantomData<T>,
}

// ===== impl Stored =====

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

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $add:ident, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
        $(
            impl super::StoreOpaque {
                pub(crate) fn $add(self: ::core::pin::Pin<&mut Self>, val: $ty) -> Stored<$ty> {
                    let $bind = self.project();
                    let index = $field.len();
                    $field.push(val);
                    Stored::new(index)
                }

                pub(crate) fn $has(&self, index: Stored<$ty>) -> bool {
                    let $bind = self;
                    $field.get(index.index).is_some()
                }

                pub(crate) fn $get(&self, index: Stored<$ty>) -> Option<&$ty> {
                    let $bind = self;
                    $field.get(index.index)
                }

                pub(crate) fn $get_mut(self: ::core::pin::Pin<&mut Self>, index: Stored<$ty>) -> Option<&mut $ty> {
                    let $bind = self.project();
                    $field.get_mut(index.index)
                }
            }
        )*
    };
}

stored_impls! {
    s
    (crate::instance::InstanceData, add_instance, has_instance, get_instance, get_instance_mut, s.stored.instances)
    (crate::func::FuncData, add_function, has_function, get_function, get_function_mut, s.stored.functions)
    (vm::ExportedTable, add_table, has_table, get_table, get_table_mut, s.stored.tables)
    (vm::ExportedMemory, add_memory, has_memory, get_memory, get_memory_mut, s.stored.memories)
    (vm::ExportedGlobal, add_global, has_global, get_global, get_global_mut, s.stored.globals)
    (vm::ExportedTag, add_tag, has_tag, get_tag, get_tag_mut, s.stored.tags)
}
