// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Compile-time page-size selectors.
//!
//! A [`PageSize`] marker names a *leaf* (block) granularity — `Size4KiB`,
//! `Size2MiB`, … — so callers of the page-table engine can state the mapping
//! granularity they want instead of coaxing it out of the auto-fit walk by
//! aligning addresses.
//!
//! The markers are deliberately **architecture-independent**: a byte size is a
//! byte size on every target, so they are defined once here and re-exported, and
//! calling code never needs `cfg`-gated size types. *Which* sizes a given
//! architecture can actually place as a leaf is expressed separately, by the
//! [`MapsAt`][crate::arch::MapsAt] bridge — a marker existing does not imply every
//! arch supports it. Naming `map::<Size512GiB>` on an arch without a 512 GiB leaf
//! level is therefore a clean unsatisfied-bound compile error.

mod sealed {
    pub trait Sealed {}
}

/// A page-table leaf granularity, named independently of any architecture.
///
/// A pure compile-time selector — the marker types are never instantiated as
/// values, only used as type parameters. Implemented only by the markers in this
/// module; the set is sealed.
pub trait PageSize: 'static + sealed::Sealed {
    /// Base-2 logarithm of [`BYTES`][Self::BYTES] — i.e. the number of low address
    /// bits a page of this size spans.
    const SHIFT: u8;

    /// The size of this page in bytes.
    const BYTES: usize = 1 << Self::SHIFT;
}

macro_rules! page_sizes {
    ($( $(#[$doc:meta])* $name:ident = $shift:literal ),* $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, Copy)]
            pub struct $name;

            impl sealed::Sealed for $name {}

            impl PageSize for $name {
                const SHIFT: u8 = $shift;
            }
        )*
    };
}

page_sizes! {
    /// 4 KiB — the translation granule on every architecture k23 currently targets.
    Size4KiB = 12,
    /// 2 MiB.
    Size2MiB = 21,
    /// 1 GiB.
    Size1GiB = 30,
    /// 512 GiB.
    Size512GiB = 39,
    /// 256 TiB.
    Size256TiB = 48,
}
