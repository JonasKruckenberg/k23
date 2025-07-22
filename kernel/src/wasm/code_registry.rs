// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use spin::{OnceLock, RwLock};

use crate::wasm::vm::CodeObject;

fn global_code() -> &'static RwLock<GlobalRegistry> {
    static GLOBAL_CODE: OnceLock<RwLock<GlobalRegistry>> = OnceLock::new();
    GLOBAL_CODE.get_or_init(Default::default)
}

type GlobalRegistry = BTreeMap<usize, (usize, Arc<CodeObject>)>;

/// Find which registered region of code contains the given program counter, and
/// what offset that PC is within that module's code.
pub fn lookup_code(pc: usize) -> Option<(Arc<CodeObject>, usize)> {
    let all_modules = global_code().read();
    let (_end, (start, module)) = all_modules.range(pc..).next()?;
    let text_offset = pc.checked_sub(*start)?;
    Some((module.clone(), text_offset))
}

/// Registers a new region of code.
///
/// Must not have been previously registered and must be `unregister`'d to
/// prevent leaking memory.
///
/// This is required to enable traps to work correctly since the signal handler
/// will lookup in the `GLOBAL_CODE` list to determine which a particular pc
/// is a trap or not.
pub fn register_code(code: &Arc<CodeObject>) {
    let text = code.text();
    if text.is_empty() {
        return;
    }
    let start = text.as_ptr() as usize;
    let end = start + text.len() - 1;
    let prev = global_code().write().insert(end, (start, code.clone()));
    assert!(prev.is_none());
}

/// Unregisters a code mmap from the global map.
///
/// Must have been previously registered with `register`.
pub fn unregister_code(code: &Arc<CodeObject>) {
    let text = code.text();
    if text.is_empty() {
        return;
    }
    let end = (text.as_ptr() as usize) + text.len() - 1;
    let code = global_code().write().remove(&end);
    assert!(code.is_some());
}
