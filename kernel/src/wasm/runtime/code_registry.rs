// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::runtime::CodeMemory;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use sync::{OnceLock, RwLock};

fn global_code() -> &'static RwLock<GlobalRegistry> {
    static GLOBAL_CODE: OnceLock<RwLock<GlobalRegistry>> = OnceLock::new();
    GLOBAL_CODE.get_or_init(Default::default)
}

type GlobalRegistry = BTreeMap<usize, (usize, Arc<CodeMemory>)>;

/// Find which registered region of code contains the given program counter, and
/// what offset that PC is within that module's code.
pub fn lookup_code(pc: usize) -> Option<(Arc<CodeMemory>, usize)> {
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
/// This is used by trap handling to determine which region of code a faulting
/// address.
pub fn register_code(code: &Arc<CodeMemory>) {
    let text = code.text_range();
    if text.is_empty() {
        return;
    }
    let prev = global_code()
        .write()
        .insert(text.start.get(), (text.end.get(), code.clone()));
    assert!(prev.is_none());
}
