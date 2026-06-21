// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub mod both_endian;
pub mod datetime;
pub mod directory;
pub mod str;
pub mod volume;

pub use both_endian::*;
pub use datetime::*;
pub use directory::*;
pub use str::*;
pub use volume::*;

pub const SECTOR_SIZE: usize = 2048;
/// El Torito uses 512-byte "virtual" sectors for boot image sector counts.
pub const VIRTUAL_SECTOR_SIZE: u32 = 512;
