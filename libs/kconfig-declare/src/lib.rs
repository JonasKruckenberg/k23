#![no_std]

pub use kconfig_declare_macros::*;

// #[doc(hidden)]
// #[cfg(feature = "collect-symbols")]
// pub use linkme;
//
// #[derive(Debug)]
// #[repr(C)]
// pub struct Symbol {
//     pub name: &'static str,
//     pub paths: &'static [&'static str],
//     pub description: &'static [&'static str],
//     pub default: &'static str,
//     pub file: &'static str,
//     pub line: u32,
//     pub column: u32,
// }
//
// #[cfg(feature = "collect-symbols")]
// #[doc(hidden)]
// #[linkme::distributed_slice]
// pub static SYMBOLS: [Symbol];
//
// #[cfg(feature = "collect-symbols")]
// pub fn symbols() -> &'static [Symbol] {
//     SYMBOLS.as_ref()
// }
//
// #[cfg(not(feature = "collect-symbols"))]
// pub fn symbols() -> &'static [Symbol] {
//     // eprintln!("`collect-symbols` feature is not enabled");
//     &[]
// }
