#![no_std]
#![no_main]
#![feature(naked_functions)]
#![expect(
    incomplete_features,
    reason = "generic_const_exprs is incomplete, but used by page_alloc"
)]
#![feature(generic_const_exprs)]
#![feature(maybe_uninit_slice)]

mod arch;
mod boot_info;
mod error;
mod kernel;
mod machine_info;
mod page_alloc;
mod panic;
mod vm;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;

pub const ENABLE_KASLR: bool = false;
