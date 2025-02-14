// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;

pub trait SetColor {
    fn set_fg_color(&mut self, color: Color);
    fn fg_color(&self) -> Color;

    /// Sets bold text.
    ///
    /// This may brighten a text color if bold text is not supported.
    fn set_bold(&mut self, bold: bool);

    fn with_bold(&mut self) -> WithBold<'_, Self>
    where
        Self: fmt::Write + Sized,
    {
        self.set_bold(true);
        WithBold { writer: self }
    }

    #[must_use]
    fn with_fg_color(&mut self, color: Color) -> WithFgColor<'_, Self>
    where
        Self: fmt::Write + Sized,
    {
        let prev_color = self.fg_color();
        self.set_fg_color(color);
        WithFgColor {
            writer: self,
            prev_color,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum Color {
    Black = 0,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Default,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

#[derive(Debug, Eq, PartialEq)]
pub struct WithFgColor<'writer, W>
where
    W: fmt::Write + SetColor,
{
    writer: &'writer mut W,
    prev_color: Color,
}

#[derive(Debug, Eq, PartialEq)]
pub struct WithBold<'writer, W>
where
    W: fmt::Write + SetColor,
{
    writer: &'writer mut W,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct AnsiEscapes<W> {
    writer: W,
    current_fg: Color,
}

impl<W: SetColor> SetColor for &'_ mut W {
    #[inline]
    fn set_fg_color(&mut self, color: Color) {
        W::set_fg_color(self, color);
    }

    #[inline]
    fn fg_color(&self) -> Color {
        W::fg_color(self)
    }

    #[inline]
    fn set_bold(&mut self, bold: bool) {
        W::set_bold(self, bold);
    }
}

// === impl WithFgColor ===

impl<W> fmt::Write for WithFgColor<'_, W>
where
    W: fmt::Write + SetColor,
{
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.writer.write_str(s)
    }

    #[inline]
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.writer.write_char(c)
    }

    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        self.writer.write_fmt(args)
    }
}

impl<W> Drop for WithFgColor<'_, W>
where
    W: fmt::Write + SetColor,
{
    fn drop(&mut self) {
        self.writer.set_fg_color(self.prev_color);
    }
}

// === impl WithBold ===

impl<W> fmt::Write for WithBold<'_, W>
where
    W: fmt::Write + SetColor,
{
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.writer.write_str(s)
    }

    #[inline]
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.writer.write_char(c)
    }

    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        self.writer.write_fmt(args)
    }
}

impl<W> Drop for WithBold<'_, W>
where
    W: fmt::Write + SetColor,
{
    fn drop(&mut self) {
        self.writer.set_bold(false);
    }
}

// === impl AnsiEscapes ===

impl<W> AnsiEscapes<W> {
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            current_fg: Color::Default,
        }
    }
}

impl<W> fmt::Write for AnsiEscapes<W>
where
    W: fmt::Write,
{
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.writer.write_str(s)
    }

    #[inline]
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.writer.write_char(c)
    }

    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        self.writer.write_fmt(args)
    }
}

impl<W> AnsiEscapes<W> {
    const ANSI_FG_COLOR_TABLE: [&'static str; 17] = [
        "30", // black
        "31", // red
        "32", // green
        "33", // yellow
        "34", // blue
        "35", // magenta
        "36", // cyan
        "37", // white
        "39", // default
        "90", // bright black
        "91", // bright red
        "92", // bright green
        "93", // bright yellow
        "94", // bright blue
        "95", // bright magenta
        "96", // bright cyan
        "97", // bright white
    ];

    fn fg_code(&self) -> &'static str {
        Self::ANSI_FG_COLOR_TABLE[self.current_fg as usize]
    }
}

impl<W: fmt::Write> SetColor for AnsiEscapes<W> {
    fn set_fg_color(&mut self, color: Color) {
        self.current_fg = color;
        let _ = write!(self.writer, "\x1b[{}m", self.fg_code());
    }

    fn fg_color(&self) -> Color {
        self.current_fg
    }

    fn set_bold(&mut self, bold: bool) {
        let _ = if bold {
            self.writer.write_str("\x1b[1m")
        } else {
            self.writer.write_str("\x1b[22m")
        };
    }
}
