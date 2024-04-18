//! This library contains code that is common to both the `cranelift-cranelift-codegen` and
//! `cranelift-cranelift-codegen-meta` libraries.
#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

pub mod constant_hash;
pub mod constants;

/// Version number of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
