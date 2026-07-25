// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use tracing_core::Metadata;
use uart_16550::Sender;

use crate::tracing::color::{AnsiEscapes, Color, SetColor};

pub trait MakeWriter {
    type Writer<'a>: fmt::Write
    where
        Self: 'a;

    /// Calls `f` with a writer for this sink, serialising against other writers
    /// for the duration of the call.
    fn with_writer<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Self::Writer<'_>) -> R;

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        let _ = meta;
        true
    }

    /// Like [`MakeWriter::with_writer`], but returns `None` without running `f`
    /// if this sink is not enabled for `meta`.
    #[inline]
    fn with_writer_for<F, R>(&self, meta: &Metadata<'_>, f: F) -> Option<R>
    where
        F: FnOnce(Self::Writer<'_>) -> R,
    {
        if self.enabled(meta) {
            return Some(self.with_writer(f));
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

/// The console output sink: the UART transmit half plus a claim flag that keeps
/// whole log lines from interleaving.
pub struct Uart {
    tx: Sender,
    /// Set while some writer owns the console, so concurrent writers don't
    /// interleave mid-line.
    ///
    /// Deliberately *not* a lock over `tx`: [`Sender::send`] is safe to call
    /// concurrently (it only touches atomics and volatile MMIO), so this flag
    /// buys output legibility, not soundness. A writer that loses the race
    /// therefore writes anyway rather than waiting — see [`Uart::with_tx`].
    busy: AtomicBool,
}

pub struct UartWriter<'a>(&'a Sender);

/// Hands the console back when dropped, including on unwind.
struct LineClaim<'a>(&'a AtomicBool);

impl Drop for LineClaim<'_> {
    #[inline]
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Uart {
    pub fn new(tx: Sender) -> Self {
        Self {
            tx,
            busy: AtomicBool::new(false),
        }
    }

    /// Calls `f` with the raw transmit half, claiming the console for the
    /// duration of the call so log lines and raw writes don't interleave.
    ///
    /// The claim is best effort: if another writer already holds it, `f` runs
    /// anyway. Waiting instead would hang the hart whenever a panic is thrown
    /// from inside a log line, because `#[panic_handler]` logs and so re-enters
    /// this function while the claim is still held further up the same stack.
    /// The cost of not waiting is garbled output under contention, which beats
    /// a deadlock in an already-fatal situation.
    pub fn with_tx<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Sender) -> R,
    {
        let _claim = self
            .busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
            .then(|| LineClaim(&self.busy));

        f(&self.tx)
    }
}

impl MakeWriter for Uart {
    type Writer<'a> = AnsiEscapes<UartWriter<'a>>;

    fn with_writer<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Self::Writer<'_>) -> R,
    {
        self.with_tx(|tx| f(AnsiEscapes::new(UartWriter(tx))))
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
