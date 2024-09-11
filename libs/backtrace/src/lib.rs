#![no_std]

mod symbolize;

use core::fmt;
use core::fmt::Formatter;
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

impl<'a, 'data> fmt::Display for Backtrace<'a, 'data> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "stack backtrace:")?;
        let mut frame_idx = 0;

        let mut frames = self.frames.clone();

        while let Some(frame) = frames.next().unwrap() {
            let mut syms = self
                .symbolize_ctx
                .resolve_unsynchronized(frame.region_start())
                .unwrap();

            while let Some(sym) = syms.next().unwrap() {
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

                frame_idx += 1;
            }
        }

        writeln!(
            f,
            "note: Some details are omitted, \
             run with `RUST_BACKTRACE=full` for a verbose backtrace."
        )?;

        Ok(())
    }
}
