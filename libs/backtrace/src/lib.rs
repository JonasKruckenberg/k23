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
    #[inline]
    pub fn capture(ctx: &'a SymbolizeContext<'data>) -> Result<Self, unwind2::Error> {
        Self::new_inner(ctx, FrameIter::new())
    }

    /// Constructs a backtrace from the provided register context, returning an owned representation.
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
    #[inline]
    pub fn from_registers(
        ctx: &'a SymbolizeContext<'data>,
        regs: unwind2::Registers,
        ip: usize,
    ) -> Result<Self, unwind2::Error> {
        let iter = FrameIter::from_registers(regs, ip);
        Self::new_inner(ctx, iter)
    }

    fn new_inner(
        ctx: &'a SymbolizeContext<'data>,
        mut iter: FrameIter,
    ) -> Result<Self, unwind2::Error> {
        let mut frames = ArrayVec::new();
        let mut frames_omitted: usize = 0;

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

        let mut print = false;
        let mut omitted_count: usize = 0;
        let mut first_omit = true;

        for ip in &self.frames {
            let mut syms = self
                .symbolize_ctx
                .resolve_unsynchronized(*ip as u64)
                .unwrap();

            while let Some(sym) = syms.next().unwrap() {
                // if print_fmt == PrintFmt::Short {
                if let Some(sym) = sym.name().map(|s| s.as_raw_str()) {
                    if sym.contains("__rust_end_short_backtrace") {
                        print = true;
                        continue;
                    }
                    if print && sym.contains("__rust_begin_short_backtrace") {
                        print = false;
                        continue;
                    }
                    if !print {
                        omitted_count += 1;
                    }
                }
                // }

                if print {
                    if omitted_count > 0 {
                        // debug_assert!(print_fmt == PrintFmt::Short);
                        // only print the message between the middle of frames
                        if !first_omit {
                            let _ = writeln!(
                                f,
                                "      [... omitted {} frame{} ...]",
                                omitted_count,
                                if omitted_count > 1 { "s" } else { "" }
                            );
                        }
                        first_omit = false;
                        omitted_count = 0;
                    }

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
                }

                frame_idx += 1i32;
            }
        }

        // writeln!(
        //     f,
        //     "note: Some details are omitted, \
        //      run with `RUST_BACKTRACE=full` for a verbose backtrace."
        // )?;

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
