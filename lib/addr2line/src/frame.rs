// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::iter;

use fallible_iterator::FallibleIterator;

use crate::{Error, Function, InlinedFunction, ResUnit, maybe_small};

/// A source location.
pub struct Location<'a> {
    /// The file name.
    pub file: Option<&'a str>,
    /// The line number.
    pub line: Option<u32>,
    /// The column number.
    ///
    /// A value of `Some(0)` indicates the left edge.
    pub column: Option<u32>,
}

/// A function frame.
pub struct Frame<'ctx, R: gimli::Reader> {
    /// The DWARF unit offset corresponding to the DIE of the function.
    pub dw_die_offset: Option<gimli::UnitOffset<R::Offset>>,
    /// The name of the function.
    pub function: Option<FunctionName<R>>,
    /// The source location corresponding to this frame.
    pub location: Option<Location<'ctx>>,
}

/// An iterator over function frames.
pub struct FrameIter<'ctx, R>(FrameIterState<'ctx, R>)
where
    R: gimli::Reader;

enum FrameIterState<'ctx, R>
where
    R: gimli::Reader,
{
    Empty,
    Location(Option<Location<'ctx>>),
    Frames(FrameIterFrames<'ctx, R>),
}

struct FrameIterFrames<'ctx, R>
where
    R: gimli::Reader,
{
    unit: &'ctx ResUnit<R>,
    sections: &'ctx gimli::Dwarf<R>,
    function: &'ctx Function<R>,
    inlined_functions: iter::Rev<maybe_small::IntoIter<&'ctx InlinedFunction<R>>>,
    next: Option<Location<'ctx>>,
}

impl<'ctx, R> FrameIter<'ctx, R>
where
    R: gimli::Reader + 'ctx,
{
    pub(crate) fn new_empty() -> Self {
        FrameIter(FrameIterState::Empty)
    }

    pub(crate) fn new_location(location: Location<'ctx>) -> Self {
        FrameIter(FrameIterState::Location(Some(location)))
    }

    pub(crate) fn new_frames(
        unit: &'ctx ResUnit<R>,
        sections: &'ctx gimli::Dwarf<R>,
        function: &'ctx Function<R>,
        inlined_functions: maybe_small::Vec<&'ctx InlinedFunction<R>>,
        location: Option<Location<'ctx>>,
    ) -> Self {
        FrameIter(FrameIterState::Frames(FrameIterFrames {
            unit,
            sections,
            function,
            inlined_functions: inlined_functions.into_iter().rev(),
            next: location,
        }))
    }
}

impl<'ctx, R> FallibleIterator for FrameIter<'ctx, R>
where
    R: gimli::Reader + 'ctx,
{
    type Item = Frame<'ctx, R>;
    type Error = Error;

    #[inline]
    fn next(&mut self) -> Result<Option<Frame<'ctx, R>>, Error> {
        let frames = match &mut self.0 {
            FrameIterState::Empty => return Ok(None),
            FrameIterState::Location(location) => {
                // We can't move out of a mutable reference, so use `take` instead.
                let location = location.take();
                self.0 = FrameIterState::Empty;
                return Ok(Some(Frame {
                    dw_die_offset: None,
                    function: None,
                    location,
                }));
            }
            FrameIterState::Frames(frames) => frames,
        };

        let loc = frames.next.take();
        let func = match frames.inlined_functions.next() {
            Some(func) => func,
            None => {
                let frame = Frame {
                    dw_die_offset: Some(frames.function.dw_die_offset),
                    function: frames.function.name.clone().map(|name| FunctionName {
                        name,
                        language: frames.unit.lang,
                    }),
                    location: loc,
                };
                self.0 = FrameIterState::Empty;
                return Ok(Some(frame));
            }
        };

        let mut next = Location {
            file: None,
            line: if func.call_line != 0 {
                Some(func.call_line)
            } else {
                None
            },
            column: if func.call_column != 0 {
                Some(func.call_column)
            } else {
                None
            },
        };
        if let Some(call_file) = func.call_file
            && let Some(lines) = frames.unit.parse_lines(frames.sections)?
        {
            next.file = lines.file(call_file);
        }
        frames.next = Some(next);

        Ok(Some(Frame {
            dw_die_offset: Some(func.dw_die_offset),
            function: func.name.clone().map(|name| FunctionName {
                name,
                language: frames.unit.lang,
            }),
            location: loc,
        }))
    }
}

/// A function name.
pub struct FunctionName<R: gimli::Reader> {
    /// The name of the function.
    pub name: R,
    /// The language of the compilation unit containing this function.
    pub language: Option<gimli::DwLang>,
}
