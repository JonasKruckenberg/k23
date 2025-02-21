// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![expect(tail_expr_drop_order, reason = "vetted")]

mod symbolize;

use arrayvec::ArrayVec;
use core::fmt;
use core::fmt::Formatter;
use fallible_iterator::FallibleIterator;
use unwind2::FrameIter;

pub use crate::symbolize::SymbolizeContext;

#[derive(Clone)]
pub struct Backtrace<'a, 'data, const MAX_FRAMES: usize> {
    symbolize_ctx: &'a SymbolizeContext<'data>,
    pub frames: ArrayVec<usize, MAX_FRAMES>,
    pub frames_omitted: usize,
}

impl<'a, 'data, const MAX_FRAMES: usize> Backtrace<'a, 'data, MAX_FRAMES> {
    /// Captures a backtrace at the callsite of this function, returning an owned representation.
    ///
    /// The returned object is almost entirely self-contained. It can be cloned, or send to other threads.
    ///
    /// Note that this step is quite cheap, contrary to the `Backtrace` implementation in the standard
    /// library this resolves the symbols (the expensive step) lazily, so this struct can be constructed
    /// in performance sensitive codepaths and only later resolved.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`unwind2::Error`] if walking the stack fails.
    pub fn capture(ctx: &'a SymbolizeContext<'data>) -> Result<Self, unwind2::Error> {
        let mut frames = ArrayVec::new();
        let mut frames_omitted: usize = 0;

        let mut iter = FrameIter::new();
        while let Some(frame) = iter.next()? {
            if frames.try_push(frame.ip()).is_err() {
                frames_omitted += 1;
            }
        }

        Ok(Self {
            symbolize_ctx: ctx,
            frames,
            frames_omitted,
        })
    }
}

impl<const MAX_FRAMES: usize> fmt::Display for Backtrace<'_, '_, MAX_FRAMES> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "stack backtrace:")?;

        let mut frame_idx: i32 = 0;
        for ip in &self.frames {
            let mut syms = self
                .symbolize_ctx
                .resolve_unsynchronized(*ip as u64)
                .unwrap();

            while let Some(sym) = syms.next().unwrap() {
                write!(f, "{frame_idx}: {address:#x}    -", address = ip)?;
                if let Some(name) = sym.name() {
                    writeln!(f, "      {name}")?;
                } else {
                    writeln!(f, "      <unknown>")?;
                }
                if let Some(filename) = sym.filename() {
                    write!(f, "      at {filename}")?;
                    if let Some(lineno) = sym.lineno() {
                        write!(f, ":{lineno}")?;
                    } else {
                        write!(f, "??")?;
                    }
                    if let Some(colno) = sym.colno() {
                        writeln!(f, ":{colno}")?;
                    } else {
                        writeln!(f, "??")?;
                    }
                }

                frame_idx += 1i32;
            }
        }

        Ok(())
    }
}

/// Fixed frame used to clean the backtrace with `RUST_BACKTRACE=1`. Note that
/// this is only inline(never) when backtraces in std are enabled, otherwise
/// it's fine to optimize away.
#[inline(never)]
pub fn __rust_begin_short_backtrace<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let result = f();

    // prevent this frame from being tail-call optimised away
    core::hint::black_box(());

    result
}

/// Fixed frame used to clean the backtrace with `RUST_BACKTRACE=1`. Note that
/// this is only inline(never) when backtraces in std are enabled, otherwise
/// it's fine to optimize away.
#[inline(never)]
pub fn __rust_end_short_backtrace<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let result = f();

    // prevent this frame from being tail-call optimised away
    core::hint::black_box(());

    result
}
