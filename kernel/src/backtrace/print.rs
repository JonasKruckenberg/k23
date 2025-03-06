// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Printing of backtraces. This is adapted from the standard library.

use crate::backtrace::BacktraceStyle;
use crate::backtrace::symbolize::{Symbol, SymbolName};
use core::fmt;

const HEX_WIDTH: usize = 2 + 2 * size_of::<usize>();

/// A formatter for backtraces.
///
/// This type can be used to print a backtrace regardless of where the backtrace
/// itself comes from. If you have a `Backtrace` type then its `Debug`
/// implementation already uses this printing format.
pub struct BacktraceFmt<'a, 'b> {
    fmt: &'a mut fmt::Formatter<'b>,
    frame_index: usize,
    format: BacktraceStyle,
}
impl<'a, 'b> BacktraceFmt<'a, 'b> {
    /// Create a new `BacktraceFmt` which will write output to the provided
    /// `fmt`.
    ///
    /// The `format` argument will control the style in which the backtrace is
    /// printed, and the `print_path` argument will be used to print the
    /// `BytesOrWideString` instances of filenames. This type itself doesn't do
    /// any printing of filenames, but this callback is required to do so.
    pub fn new(fmt: &'a mut fmt::Formatter<'b>, format: BacktraceStyle) -> Self {
        BacktraceFmt {
            fmt,
            frame_index: 0,
            format,
        }
    }

    /// Adds a frame to the backtrace output.
    ///
    /// This commit returns an RAII instance of a `BacktraceFrameFmt` which can be used
    /// to actually print a frame, and on destruction it will increment the
    /// frame counter.
    pub fn frame(&mut self) -> BacktraceFrameFmt<'_, 'a, 'b> {
        BacktraceFrameFmt {
            fmt: self,
            symbol_index: 0,
        }
    }

    /// Return the inner formatter.
    ///
    /// This is used for writing custom information between frames with `write!` and `writeln!`,
    /// and won't increment the `frame_index` unlike the `frame` method.
    pub fn formatter(&mut self) -> &mut fmt::Formatter<'b> {
        self.fmt
    }
}

/// A formatter for just one frame of a backtrace.
///
/// This type is created by the `BacktraceFmt::frame` function.
pub struct BacktraceFrameFmt<'fmt, 'a, 'b> {
    fmt: &'fmt mut BacktraceFmt<'a, 'b>,
    symbol_index: usize,
}

impl BacktraceFrameFmt<'_, '_, '_> {
    /// Prints a symbol with this frame formatter.
    pub fn print_symbol(&mut self, frame_ip: usize, sym: Symbol<'_>) -> fmt::Result {
        self.print_raw_with_column(
            frame_ip,
            sym.name(),
            sym.filename(),
            sym.lineno(),
            sym.colno(),
        )
    }

    /// Print a raw (read un-symbolized) address with this frame formatter. A raw address
    /// will just show up as the address and `<unknown>` and should be used in places where we
    /// couldn't find any symbol information for an address.
    pub fn print_raw(&mut self, frame_ip: usize) -> fmt::Result {
        self.print_raw_with_column(frame_ip, None, None, None, None)
    }

    fn print_raw_with_column(
        &mut self,
        frame_ip: usize,
        symbol_name: Option<SymbolName<'_>>,
        filename: Option<&str>,
        lineno: Option<u32>,
        colno: Option<u32>,
    ) -> fmt::Result {
        self.print_raw_generic(frame_ip, symbol_name, filename, lineno, colno)?;
        self.symbol_index += 1;
        Ok(())
    }

    // #[allow(unused_mut)]
    fn print_raw_generic(
        &mut self,
        frame_ip: usize,
        symbol_name: Option<SymbolName<'_>>,
        filename: Option<&str>,
        lineno: Option<u32>,
        colno: Option<u32>,
    ) -> fmt::Result {
        // No need to print "null" frames, it basically just means that the
        // system backtrace was a bit eager to trace back super far.
        if let BacktraceStyle::Short = self.fmt.format {
            if frame_ip == 0 {
                return Ok(());
            }
        }

        // Print the index of the frame as well as the optional instruction
        // pointer of the frame. If we're beyond the first symbol of this frame
        // though we just print appropriate whitespace.
        if self.symbol_index == 0 {
            write!(self.fmt.fmt, "{:4}: ", self.fmt.frame_index)?;

            // Print the instruction pointer. If the symbol name is None we always print
            // the address. Those weird frames don't happen that often and are always the most
            // interesting in a backtrace, so we need to make sure to print at least the address so
            // we have somewhere to start investigating!
            if self.fmt.format == BacktraceStyle::Full || symbol_name.is_some() {
                write!(self.fmt.fmt, "{frame_ip:HEX_WIDTH$x?} - ")?;
            }
        } else {
            write!(self.fmt.fmt, "      ")?;
            if let BacktraceStyle::Full = self.fmt.format {
                write!(self.fmt.fmt, "{:1$}", "", HEX_WIDTH + 3)?;
            }
        }

        // Next up write out the symbol name, using the alternate formatting for
        // more information if we're a full backtrace. Here we also handle
        // symbols which don't have a name,
        match (symbol_name, &self.fmt.format) {
            (Some(name), BacktraceStyle::Short) => write!(self.fmt.fmt, "{name:#}")?,
            (Some(name), BacktraceStyle::Full) => write!(self.fmt.fmt, "{name}")?,
            (None, _) => write!(self.fmt.fmt, "<unknown>")?,
        }
        self.fmt.fmt.write_str("\n")?;

        // And last up, print out the filename/line number if they're available.
        if let (Some(file), Some(line)) = (filename, lineno) {
            self.print_fileline(file, line, colno)?;
        }

        Ok(())
    }

    fn print_fileline(&mut self, file: &str, line: u32, colno: Option<u32>) -> fmt::Result {
        // Filename/line are printed on lines under the symbol name, so print
        // some appropriate whitespace to sort of right-align ourselves.
        if let BacktraceStyle::Full = self.fmt.format {
            write!(self.fmt.fmt, "{:1$}", "", HEX_WIDTH)?;
        }
        write!(self.fmt.fmt, "             at ")?;

        // Print the filename and line number.
        write!(self.fmt.fmt, "at {file}")?;
        write!(self.fmt.fmt, ":{line}")?;

        // Add column number, if available.
        if let Some(colno) = colno {
            write!(self.fmt.fmt, ":{colno}")?;
        }

        writeln!(self.fmt.fmt)?;
        Ok(())
    }
}

impl Drop for BacktraceFrameFmt<'_, '_, '_> {
    fn drop(&mut self) {
        self.fmt.frame_index += 1;
    }
}
