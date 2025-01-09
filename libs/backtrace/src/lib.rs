// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]

mod symbolize;

use core::fmt;
use core::fmt::Formatter;
use fallible_iterator::FallibleIterator;
use unwind2::FramesIter;

pub use crate::symbolize::SymbolizeContext;

#[derive(Clone)]
pub struct Backtrace<'a, 'data> {
    symbolize_ctx: &'a SymbolizeContext<'data>,
    frames: FramesIter,
}

impl<'a, 'data> Backtrace<'a, 'data> {
    pub fn capture(ctx: &'a SymbolizeContext<'data>) -> Self {
        let frames = FramesIter::new();

        Self {
            symbolize_ctx: ctx,
            frames,
        }
    }
}

// FIXME: This *will* dealock rn, since we can't log from within this impl
// it will lead to a deadlock since we already hold the stdouts lock
impl fmt::Display for Backtrace<'_, '_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "stack backtrace:")?;
        let mut frame_idx = 0;

        let mut frames = self.frames.clone();
        let mut print = false;
        let mut omitted_count: usize = 0;
        let mut first_omit = true;

        while let Some(frame) = frames.next().unwrap() {
            let mut syms = self
                .symbolize_ctx
                .resolve_unsynchronized(frame.region_start())
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

                    write!(
                        f,
                        "{frame_idx}: {address:#x}    -",
                        address = frame.region_start()
                    )?;
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

                frame_idx += 1;
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
