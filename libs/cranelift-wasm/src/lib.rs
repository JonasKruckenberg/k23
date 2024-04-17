#![cfg_attr(not(test), no_std)]
#![feature(error_in_core, trait_upcasting)]

extern crate alloc;

mod compiler;
mod debug;
mod error;
mod heap;
mod state;
mod table;
mod traits;
mod translate_function;
mod translate_inst;
mod translate_module;

pub use error::Error;

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub use compiler::Compiler;
pub use debug::{DebugInfo, NameSection};
pub use heap::{Heap, HeapData};
pub use state::{GlobalVariable, State};
pub use table::{Table, TableData, TableSize};
pub use traits::{FuncTranslationEnvironment, ModuleTranslationEnvironment, TargetEnvironment};
pub use translate_function::translate_function;
pub use translate_module::translate_module;
