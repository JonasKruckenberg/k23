//! A global registry of JIT-compiled code regions.
//!
//! This is used in the signal handler part of trap handling to determine which region of code a
//! faulting pc belongs to and by extension be able to retrieve trap and debugging information related
//! to it.

use crate::wasm::runtime::CodeMemory;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::lock_api::RwLock;
use spin::Once;

fn global_code() -> &'static RwLock<GlobalRegistry> {
    static GLOBAL_CODE: Once<RwLock<GlobalRegistry>> = Once::new();
    GLOBAL_CODE.call_once(Default::default)
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
    let text = code.text();
    if text.is_empty() {
        return;
    }
    let start = text.as_ptr() as usize;
    let end = start + text.len() - 1;
    let prev = global_code().write().insert(end, (start, code.clone()));
    assert!(prev.is_none());
}
