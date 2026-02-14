// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! `addr2line` provides a cross-platform library for retrieving per-address debug information
//! from files with DWARF debug information. Given an address, it can return the file name,
//! line number, and function name associated with that address, as well as the inline call
//! stack leading to that address.
//!
//! At the lowest level, the library uses a [`Context`] to cache parsed information so that
//! multiple lookups are efficient. To create a `Context`, you first need to open and parse the
//! file using an object file parser such as [`object`](https://github.com/gimli-rs/object),
//! create a [`gimli::Dwarf`], and finally call [`Context::from_dwarf`].
//!
//! Location information is obtained with [`Context::find_location`] or
//! [`Context::find_location_range`]. Function information is obtained with
//! [`Context::find_frames`], which returns a frame for each inline function. Each frame
//! contains both name and location.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]
#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

use alloc::sync::Arc;
use core::ops::ControlFlow;

use gimli::ReaderOffset;

use crate::function::{Function, InlinedFunction, LazyFunctions};
use crate::line::{LazyLines, LineLocationRangeIter, Lines};
use crate::lookup::{LoopingLookup, SimpleLookup};
use crate::unit::{ResUnit, ResUnits, SupUnits};

mod maybe_small {
    pub type Vec<T> = smallvec::SmallVec<[T; 16]>;
    pub type IntoIter<T> = smallvec::IntoIter<[T; 16]>;
}

mod frame;
pub use frame::{Frame, FrameIter, FunctionName, Location};

mod function;
// mod lazy;
mod line;

mod lookup;
use kspin::OnceLock;
pub use lookup::{LookupContinuation, LookupResult, SplitDwarfLoad};

mod unit;
pub use unit::LocationRangeIter;

type Error = gimli::Error;

pub(crate) type LazyResult<T> = OnceLock<Result<T, Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebugFile {
    Primary,
    Supplementary,
    Dwo,
}

/// The state necessary to perform address to line translation.
///
/// Constructing a `Context` is somewhat costly, so users should aim to reuse `Context`s
/// when performing lookups for many addresses in the same executable.
pub struct Context<R: gimli::Reader> {
    sections: Arc<gimli::Dwarf<R>>,
    units: ResUnits<R>,
    sup_units: SupUnits<R>,
}

impl<R: gimli::Reader> Context<R> {
    /// Construct a new `Context` from DWARF sections.
    ///
    /// This method does not support using a supplementary object file.
    #[expect(clippy::too_many_arguments)]
    pub fn from_sections(
        debug_abbrev: gimli::DebugAbbrev<R>,
        debug_addr: gimli::DebugAddr<R>,
        debug_aranges: gimli::DebugAranges<R>,
        debug_info: gimli::DebugInfo<R>,
        debug_line: gimli::DebugLine<R>,
        debug_line_str: gimli::DebugLineStr<R>,
        debug_ranges: gimli::DebugRanges<R>,
        debug_rnglists: gimli::DebugRngLists<R>,
        debug_str: gimli::DebugStr<R>,
        debug_str_offsets: gimli::DebugStrOffsets<R>,
        debug_macinfo: gimli::DebugMacinfo<R>,
        debug_macro: gimli::DebugMacro<R>,
        debug_names: gimli::DebugNames<R>,
        default_section: R,
    ) -> Result<Self, Error> {
        Self::from_dwarf(gimli::Dwarf {
            debug_abbrev,
            debug_addr,
            debug_aranges,
            debug_info,
            debug_line,
            debug_line_str,
            debug_str,
            debug_str_offsets,
            debug_macinfo,
            debug_macro,
            debug_names,
            debug_types: default_section.clone().into(),
            locations: gimli::LocationLists::new(
                default_section.clone().into(),
                default_section.into(),
            ),
            ranges: gimli::RangeLists::new(debug_ranges, debug_rnglists),
            file_type: gimli::DwarfFileType::Main,
            sup: None,
            abbreviations_cache: gimli::AbbreviationsCache::new(),
        })
    }

    /// Construct a new `Context` from an existing [`gimli::Dwarf`] object.
    #[inline]
    pub fn from_dwarf(sections: gimli::Dwarf<R>) -> Result<Context<R>, Error> {
        Self::from_arc_dwarf(Arc::new(sections))
    }

    /// Construct a new `Context` from an existing [`gimli::Dwarf`] object.
    #[inline]
    pub fn from_arc_dwarf(sections: Arc<gimli::Dwarf<R>>) -> Result<Context<R>, Error> {
        let units = ResUnits::parse(&sections)?;
        let sup_units = if let Some(sup) = sections.sup.as_ref() {
            SupUnits::parse(sup)?
        } else {
            SupUnits::default()
        };
        Ok(Context {
            sections,
            units,
            sup_units,
        })
    }
}

impl<R: gimli::Reader> Context<R> {
    /// Find the source file and line corresponding to the given virtual memory address.
    pub fn find_location(&self, probe: u64) -> Result<Option<Location<'_>>, Error> {
        for unit in self.units.find(probe) {
            if let Some(location) = unit.find_location(probe, &self.sections)? {
                return Ok(Some(location));
            }
        }
        Ok(None)
    }

    /// Return source file and lines for a range of addresses. For each location it also
    /// returns the address and size of the range of the underlying instructions.
    pub fn find_location_range(
        &self,
        probe_low: u64,
        probe_high: u64,
    ) -> Result<LocationRangeIter<'_, R>, Error> {
        self.units
            .find_location_range(probe_low, probe_high, &self.sections)
    }

    /// Return an iterator for the function frames corresponding to the given virtual
    /// memory address.
    ///
    /// If the probe address is not for an inline function then only one frame is
    /// returned.
    ///
    /// If the probe address is for an inline function then the first frame corresponds
    /// to the innermost inline function.  Subsequent frames contain the caller and call
    /// location, until an non-inline caller is reached.
    pub fn find_frames(
        &self,
        probe: u64,
    ) -> LookupResult<impl LookupContinuation<Output = Result<FrameIter<'_, R>, Error>, Buf = R>>
    {
        let mut units_iter = self.units.find(probe);
        if let Some(unit) = units_iter.next() {
            LoopingLookup::new_lookup(unit.find_function_or_location(probe, self), move |r| {
                ControlFlow::Break(match r {
                    Err(e) => Err(e),
                    Ok((Some(function), location)) => {
                        let inlined_functions = function.find_inlined_functions(probe);
                        Ok(FrameIter::new_frames(
                            unit,
                            &self.sections,
                            function,
                            inlined_functions,
                            location,
                        ))
                    }
                    Ok((None, Some(location))) => Ok(FrameIter::new_location(location)),
                    Ok((None, None)) => match units_iter.next() {
                        Some(next_unit) => {
                            return ControlFlow::Continue(
                                next_unit.find_function_or_location(probe, self),
                            );
                        }
                        None => Ok(FrameIter::new_empty()),
                    },
                })
            })
        } else {
            LoopingLookup::new_complete(Ok(FrameIter::new_empty()))
        }
    }
}

impl<R: gimli::Reader> Context<R> {
    // Find the unit containing the given offset, and convert the offset into a unit offset.
    fn find_unit(
        &self,
        offset: gimli::DebugInfoOffset<R::Offset>,
        file: DebugFile,
    ) -> Result<(&gimli::Unit<R>, gimli::UnitOffset<R::Offset>), Error> {
        let unit = match file {
            DebugFile::Primary => self.units.find_offset(offset)?,
            DebugFile::Supplementary => self.sup_units.find_offset(offset)?,
            DebugFile::Dwo => return Err(gimli::Error::NoEntryAtGivenOffset(offset.0.into_u64())),
        };

        let unit_offset = offset
            .to_unit_offset(&unit.header)
            .ok_or(gimli::Error::NoEntryAtGivenOffset(offset.0.into_u64()))?;
        Ok((unit, unit_offset))
    }
}

struct RangeAttributes<R: gimli::Reader> {
    low_pc: Option<u64>,
    high_pc: Option<u64>,
    size: Option<u64>,
    ranges_offset: Option<gimli::RangeListsOffset<<R as gimli::Reader>::Offset>>,
}

impl<R: gimli::Reader> Default for RangeAttributes<R> {
    fn default() -> Self {
        RangeAttributes {
            low_pc: None,
            high_pc: None,
            size: None,
            ranges_offset: None,
        }
    }
}

impl<R: gimli::Reader> RangeAttributes<R> {
    fn for_each_range<F: FnMut(gimli::Range)>(
        &self,
        unit: gimli::UnitRef<R>,
        mut f: F,
    ) -> Result<bool, Error> {
        let mut added_any = false;
        let mut add_range = |range: gimli::Range| {
            if range.begin < range.end {
                f(range);
                added_any = true
            }
        };
        if let Some(ranges_offset) = self.ranges_offset {
            let mut range_list = unit.ranges(ranges_offset)?;
            while let Some(range) = range_list.next()? {
                add_range(range);
            }
        } else if let (Some(begin), Some(end)) = (self.low_pc, self.high_pc) {
            add_range(gimli::Range { begin, end });
        } else if let (Some(begin), Some(size)) = (self.low_pc, self.size) {
            // If `begin` is a -1 tombstone, this will overflow and the check in
            // `add_range` will ignore it.
            let end = begin.wrapping_add(size);
            add_range(gimli::Range { begin, end });
        }
        Ok(added_any)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn context_is_send() {
        fn assert_is_send<T: Send>() {}
        assert_is_send::<crate::Context<gimli::read::EndianSlice<'_, gimli::LittleEndian>>>();
    }
}
