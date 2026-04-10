// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! #k23VM - k23 WebAssembly Virtual Machine

mod builtins;
mod code_registry;
mod compile;
mod cranelift;
mod engine;
mod func;
mod global;
mod indices;
mod instance;
mod linker;
mod memory;
mod module;
mod store;
mod table;
mod tag;
mod translate;
mod trap;
pub mod trap_handler;
mod type_registry;
mod types;
mod utils;
mod values;
mod vm;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use anyhow::Context;
use base64::Engine as _;
pub use engine::Engine;
pub use func::Func;
pub use global::Global;
use hashbrown::HashMap;
pub use instance::Instance;
pub use linker::Linker;
pub use memory::Memory;
pub use module::Module;
pub use store::Store;
pub use table::Table;
pub use tag::Tag;
pub use trap::TrapKind;
pub use values::Val;
pub use vm::{ConstExprEvaluator, PlaceholderAllocatorDontUse};

use crate::device_tree::DeviceTree;
use crate::wasm::store::StoreOpaque;
use crate::wasm::utils::{enum_accessors, owned_enum_accessors};
use crate::{shell, state};

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G).
pub const WASM32_MAX_SIZE: u64 = 1 << 32;
/// Maximum size, in bytes of WebAssembly stacks.
pub const MAX_WASM_STACK: usize = 512 * 1024;

/***************** Settings *******************************************/
/// Whether lowerings for relaxed simd instructions are forced to
/// be deterministic.
pub const RELAXED_SIMD_DETERMINISTIC: bool = false;
/// 2 GiB of guard pages
/// TODO why does this help to eliminate bounds checks?
pub const DEFAULT_OFFSET_GUARD_SIZE: u64 = 0x8000_0000;
/// The absolute maximum size of a memory in bytes
pub const MEMORY_MAX: usize = 1 << 32;
/// The absolute maximum size of a table in elements
pub const TABLE_MAX: usize = 1 << 10;

/// A WebAssembly external value which is just any type that can be imported or exported between modules.
#[derive(Clone, Debug)]
pub enum Extern {
    Func(Func),
    Table(Table),
    Memory(Memory),
    Global(Global),
    Tag(Tag),
}

impl From<Func> for Extern {
    fn from(f: Func) -> Self {
        Extern::Func(f)
    }
}

impl From<Table> for Extern {
    fn from(t: Table) -> Self {
        Extern::Table(t)
    }
}

impl From<Memory> for Extern {
    fn from(m: Memory) -> Self {
        Extern::Memory(m)
    }
}

impl From<Global> for Extern {
    fn from(g: Global) -> Self {
        Extern::Global(g)
    }
}

impl From<Tag> for Extern {
    fn from(t: Tag) -> Self {
        Extern::Tag(t)
    }
}

impl Extern {
    /// # Safety
    ///
    /// The caller must ensure `export` is a valid export within `store`.
    pub(crate) unsafe fn from_export(export: vm::Export, store: &mut StoreOpaque) -> Self {
        // Safety: ensured by caller
        unsafe {
            match export {
                vm::Export::Function(e) => Extern::Func(Func::from_exported_function(store, e)),
                vm::Export::Table(e) => Extern::Table(Table::from_exported_table(store, e)),
                vm::Export::Memory(e) => Extern::Memory(Memory::from_exported_memory(store, e)),
                vm::Export::Global(e) => Extern::Global(Global::from_exported_global(store, e)),
                vm::Export::Tag(e) => Extern::Tag(Tag::from_exported_tag(store, e)),
            }
        }
    }

    enum_accessors! {
        e
        (Func(&Func) is_func get_func unwrap_func e)
        (Table(&Table) is_table get_table unwrap_table e)
        (Memory(&Memory) is_memory get_memory unwrap_memory e)
        (Global(&Global) is_global get_global unwrap_global e)
    }

    owned_enum_accessors! {
        e
        (Func(Func) into_func e)
        (Table(Table) into_table e)
        (Memory(Memory) into_memory e)
        (Global(Global) into_global e)
    }
}

#[derive(Debug)]
pub struct UnstableLocalRegistry {
    map: HashMap<String, Module>,
}

impl UnstableLocalRegistry {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn from_devtree(devtree: &DeviceTree, engine: &Engine) -> anyhow::Result<Self> {
        let chosen = devtree.find_by_path("/chosen").unwrap();

        let Some(prop) = chosen.property("bootargs") else {
            return Ok(Self::new());
        };

        let mut me = Self::new();
        let mut validator = wasmparser::Validator::new();

        log::trace!("bootargs {}", prop.as_str()?);
        let parts = prop.as_str()?.trim().split(';');
        for part in parts {
            if let Some(current) = part.strip_prefix("unstable_preload=") {
                let (module_name, module_body_base64) = current
                    .split_once('=')
                    .context("malformed preload directice")?;

                tracing::trace!(
                    "decoding unstable preload module {module_name} {module_body_base64}"
                );

                me.insert_base64(module_name, module_body_base64, engine, &mut validator)?;
            }
        }

        Ok(me)
    }

    pub fn insert_base64(
        &mut self,
        module_name: &str,
        module_body_base64: &str,
        engine: &Engine,
        validator: &mut wasmparser::Validator,
    ) -> anyhow::Result<()> {
        let estimated_len = base64::decoded_len_estimate(module_body_base64.len());
        tracing::trace!("decoding base64... (estimated size in-memory: {estimated_len} bytes)");

        let mut bytes = Vec::with_capacity(estimated_len);

        base64::prelude::BASE64_STANDARD
            .decode_vec(module_body_base64, &mut bytes)
            .map_err(|err| anyhow::anyhow!("failed to decode base64 encoded wasm module {err}"))?;

        tracing::trace!("parsing unstable preload module...");

        let module = Module::from_bytes(engine, validator, &bytes)
            .context("failed to parse preloaded Wasm module")?;

        self.map.insert(module_name.to_owned(), module);

        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Module> {
        self.map.get(name)
    }
}

pub const UNSTABLE_EVAL: shell::Command = shell::Command::new("unstable-eval")
    .with_usage("<MODULE NAME> <FUNC NAME> <ARG...>")
    .with_help(r"evaluate a WebAssembly application.

Expects a module identifier, function identifier within that module, and repeated function arguments.
Modules must be registered as base64 through the `unstable_preload` boot argument.
")
    .with_fn(|ctx| {
        let mut parts = ctx.command().split_whitespace();

        let module_name = parts
            .next()
            .ok_or_else(|| ctx.other_error("missing module identifier"))?;

        let func_name = parts
            .next()
            .ok_or_else(|| ctx.other_error("missing module identifier"))?;

        let global = state::global();

        let module = global
            .unstable_local_registry
            .get(module_name)
            .ok_or_else(|| ctx.other_error("module not registered"))?;

        let linker = Linker::new(&global.engine);
        let mut store = Store::new(&global.engine, &PlaceholderAllocatorDontUse, ());
        let mut const_eval = ConstExprEvaluator::default();

        let instance = linker
            .instantiate(&mut store, &mut const_eval, module)
            .map_err(|err| {
                tracing::error!("failed to instantiate module {module_name:?}. {err}");
                ctx.other_error("failed to instantiate module")
            })?;

        let func = instance.get_func(&mut store, func_name).ok_or_else(|| {
            tracing::error!(
                "failed to retrieve function {func_name:?} from module {module_name:?}."
            );
            ctx.other_error("failed to retrieve function from module")
        })?;

        let func_ty = func.ty(&store);

        let params: Vec<_> = func_ty
            .params()
            .zip(parts)
            .map(|(ty, arg)| match ty {
                types::ValType::I32 => Ok(Val::I32(arg.parse::<i32>().unwrap())),
                types::ValType::I64 => Ok(Val::I64(arg.parse::<i64>().unwrap())),
                types::ValType::F32 => Ok(Val::F32(arg.parse::<f32>().unwrap().to_bits())),
                types::ValType::F64 => Ok(Val::F64(arg.parse::<f64>().unwrap().to_bits())),
                types::ValType::V128 => Ok(Val::V128(arg.parse::<u128>().unwrap())),
                types::ValType::Ref(_) => {
                    Err(
                        ctx.other_error("cannot specify `Ref` type argument on the command-line!")
                    )
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut results = vec![Val::null_func_ref(); func_ty.results().len()];

        func.call(&mut store, &params, &mut results)
            .map_err(|err| {
                tracing::error!("wasm execution error! {err}");
                ctx.other_error("failed to execute wasm module")
            })?;

        tracing::info!("results: {results:?}");

        Ok(())
    });
