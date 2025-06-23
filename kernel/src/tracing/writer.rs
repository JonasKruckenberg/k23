// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::tracing::color::{AnsiEscapes, Color, SetColor};
use core::cell::UnsafeCell;
use core::fmt::{Arguments, Write};
use core::{cmp, fmt};
use spin::{ReentrantMutex, ReentrantMutexGuard};
use tracing_core::Metadata;

pub trait MakeWriter<'a> {
    type Writer: fmt::Write;
    fn make_writer(&'a self) -> Self::Writer;

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        let _ = meta;
        true
    }

    #[inline]
    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Option<Self::Writer> {
        if self.enabled(meta) {
            return Some(self.make_writer());
        }

        None
    }

    fn line_len(&self) -> usize {
        120
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum IndentKind {
    Event,
    NewSpan,
    Indent,
}

pub struct Writer<W: Write> {
    pub(super) writer: W,
    pub(super) indent: usize,
    pub(super) max_line_len: usize,
    pub(super) current_line: usize,
}

impl<W: Write> Writer<W> {
    pub fn indent(&mut self, kind: IndentKind) -> fmt::Result {
        self.write_indent(" ")?;

        for i in 1..=self.indent {
            let indent_str = match (i, kind) {
                (i, IndentKind::Event) if i == self.indent => "├",
                _ => "│",
            };
            self.write_indent(indent_str)?;
        }

        if kind == IndentKind::NewSpan {
            self.write_indent("┌")?;
        }

        Ok(())
    }

    fn write_indent(&mut self, chars: &'static str) -> fmt::Result {
        self.writer.write_str(chars)?;
        self.current_line += chars.len();
        Ok(())
    }

    fn write_newline(&mut self) -> fmt::Result {
        // including width of the 16-character timestamp bit
        self.writer.write_str("                             ")?;
        self.current_line = 3;
        self.indent(IndentKind::Indent)
    }

    pub fn finish(&mut self) -> fmt::Result {
        self.current_line = 0;
        self.writer.write_char('\n')
    }
}

impl<W> Write for Writer<W>
where
    W: Write,
{
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let lines = s.split_inclusive('\n');
        for line in lines {
            let mut line = line;
            while self.current_line + line.len() >= self.max_line_len {
                let offset = if let Some(last_ws) = line[..self.max_line_len - self.current_line]
                    .chars()
                    .rev()
                    .position(|c| c.is_whitespace())
                {
                    // found a nice whitespace to break on!
                    self.writer.write_str(&line[..last_ws])?;
                    last_ws
                } else {
                    let offset = cmp::min(line.len(), self.max_line_len);
                    self.writer.write_str(&line[..offset])?;
                    offset
                };

                self.writer.write_char('\n')?;
                self.write_newline()?;
                self.writer.write_char(' ')?;
                self.current_line += 1;
                line = &line[offset..];
            }

            self.writer.write_str(line)?;
            if line.ends_with('\n') {
                self.write_newline()?;
                self.writer.write_char(' ')?;
            }
            self.current_line += line.len();
        }

        Ok(())
    }

    fn write_char(&mut self, ch: char) -> fmt::Result {
        self.writer.write_char(ch)?;
        if ch == '\n' {
            self.write_newline()
        } else {
            Ok(())
        }
    }
}

impl<W> SetColor for Writer<W>
where
    W: Write + SetColor,
{
    fn set_fg_color(&mut self, color: Color) {
        self.writer.set_fg_color(color);
    }

    fn fg_color(&self) -> Color {
        self.writer.fg_color()
    }

    fn set_bold(&mut self, bold: bool) {
        self.writer.set_bold(bold);
    }
}

impl<W: Write> Drop for Writer<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

// Architecture-specific debug output implementation
cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        type DebugStream = riscv::hio::HostStream;

        fn new_debug_stream() -> DebugStream {
            riscv::hio::HostStream::new_stdout()
        }
    } else {
        compile_error!("Unsupported architecture for debug output");
    }
}

pub struct Semihosting(ReentrantMutex<UnsafeCell<DebugStream>>);
pub struct SemihostingWriter<'a>(ReentrantMutexGuard<'a, UnsafeCell<DebugStream>>);

impl Semihosting {
    pub fn new() -> Self {
        Self(ReentrantMutex::new(UnsafeCell::new(new_debug_stream())))
    }
}

impl<'a> MakeWriter<'a> for Semihosting {
    type Writer = AnsiEscapes<SemihostingWriter<'a>>;

    fn make_writer(&'a self) -> Self::Writer {
        AnsiEscapes::new(SemihostingWriter(self.0.lock()))
    }
}

impl Write for SemihostingWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Safety: Racy access to the HostStream is safe, at worst this produces interleaved debug output
        let this = unsafe { &mut *self.0.get() };
        this.write_str(s)
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        // Safety: Racy access to the HostStream is safe, at worst this produces interleaved debug output
        let this = unsafe { &mut *self.0.get() };
        this.write_char(c)
    }

    fn write_fmt(&mut self, args: Arguments<'_>) -> fmt::Result {
        // Safety: Racy access to the HostStream is safe, at worst this produces interleaved debug output
        let this = unsafe { &mut *self.0.get() };
        this.write_fmt(args)
    }
}
