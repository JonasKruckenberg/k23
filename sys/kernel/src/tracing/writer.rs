// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::fmt::Write;

use spin::{ReentrantMutex, ReentrantMutexGuard};
use tracing_core::Metadata;
use uart_16550::Sender;

use crate::tracing::color::{AnsiEscapes, Color, SetColor};

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
}

/// Wraps the console writer to translate bare line feeds into CRLF, and to
/// terminate each log line with a trailing CRLF on drop.
///
/// Emitting the carriage return ourselves keeps output readable on terminals
/// that don't post-process it (e.g. UTM's console). QEMU's serial backend
/// happens to hide a missing CR by leaving the host tty's `ONLCR` translation
/// on, so relying on the terminal is not portable.
pub struct Writer<W: Write> {
    pub(super) writer: W,
}

impl<W: Write> Write for Writer<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut rest = s;
        while let Some(lf) = rest.find('\n') {
            self.writer.write_str(&rest[..lf])?;
            self.writer.write_str("\r\n")?;
            rest = &rest[lf + 1..];
        }
        self.writer.write_str(rest)
    }

    fn write_char(&mut self, ch: char) -> fmt::Result {
        if ch == '\n' {
            self.writer.write_str("\r\n")
        } else {
            self.writer.write_char(ch)
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
        // Terminate the log line; a bare LF would "staircase" on raw terminals.
        let _ = self.writer.write_str("\r\n");
    }
}

/// The console output sink: the UART transmit half behind a reentrant lock so
/// nested logging on one CPU can't deadlock. `None` when there is no console.
pub struct Uart(pub(crate) ReentrantMutex<Sender>);
pub struct UartWriter<'a>(ReentrantMutexGuard<'a, Sender>);

impl Uart {
    pub fn new(tx: Sender) -> Self {
        Self(ReentrantMutex::new(tx))
    }
}

impl<'a> MakeWriter<'a> for Uart {
    type Writer = AnsiEscapes<UartWriter<'a>>;

    fn make_writer(&'a self) -> Self::Writer {
        AnsiEscapes::new(UartWriter(self.0.lock()))
    }
}

impl Write for UartWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.0.send(byte);
        }

        Ok(())
    }
}
