//! `gimli` is a library for reading and writing the
//! [DWARF debugging format](https://dwarfstd.org/).
//!
//! See the [read](./read/index.html) and [write](./write/index.html) modules
//! for examples and API documentation.
//!
//! ## Cargo Features
//!
//! Cargo features that can be enabled with `gimli`:
//!
//! * `std`: Enabled by default. Use the `std` library. Disabling this feature
//!     allows using `gimli` in embedded environments that do not have access to
//!     `std`. Note that even when `std` is disabled, `gimli` still requires an
//!     implementation of the `alloc` crate.
//!
//! * `read`: Enabled by default. Enables the `read` module. Use of `std` is
//!     optional.
//!
//! * `write`: Enabled by default. Enables the `write` module. Always uses
//!     the `std` library.
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
// False positives.
#![allow(clippy::derive_partial_eq_without_eq)]
#![cfg_attr(not(test), no_std)]
#![feature(core_io_borrowed_buf)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::pedantic)]
#[allow(unused_imports)]
#[cfg(any(feature = "read", feature = "write"))]
#[macro_use]
extern crate alloc;

mod common;
pub use crate::common::*;

mod arch;
pub use crate::arch::*;

pub mod constants;
// For backwards compat.
pub use crate::constants::*;

mod endianity;
pub use crate::endianity::*;

pub mod leb128;

#[cfg(feature = "read-core")]
pub mod read;
// For backwards compat.
#[cfg(feature = "read-core")]
pub use crate::read::*;

#[cfg(feature = "write")]
pub mod write;

#[cfg(test)]
mod test_util;
