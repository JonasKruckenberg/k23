// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::{Cell, RefCell};
use cpu_local::cpu_local;

mod color;
mod log;
mod registry;
mod writer;

use crate::time::Instant;
use crate::tracing::writer::{IndentKind, MakeWriter, Semihosting, Writer};
use crate::CPUID;
pub use ::tracing::*;
use color::{Color, SetColor};
use core::fmt;
use core::fmt::Write;
use registry::Registry;
use sync::{LazyLock, OnceLock};
use tracing::field;
use tracing_core::span::{Attributes, Current, Id, Record};
use tracing_core::{Collect, Dispatch, Event, Interest, Level, LevelFilter, Metadata};

static SUBSCRIBER: OnceLock<Subscriber> = OnceLock::new();

cpu_local! {
    static OUTPUT_INDENT: Cell<usize> = Cell::new(0);
    static TIME_BASE: RefCell<Option<Instant>> = RefCell::new(None);
}

#[expect(tail_expr_drop_order, reason = "")]
pub fn init(level_filter: LevelFilter) {
    let subscriber = SUBSCRIBER.get_or_init(|| Subscriber {
        level_filter,
        output: Output::new(Semihosting::new()),
        registry: LazyLock::new(Registry::default),
    });

    ::log::set_max_level(match level_filter {
        LevelFilter::OFF => ::log::LevelFilter::Off,
        LevelFilter::TRACE => ::log::LevelFilter::Trace,
        LevelFilter::DEBUG => ::log::LevelFilter::Debug,
        LevelFilter::INFO => ::log::LevelFilter::Info,
        LevelFilter::WARN => ::log::LevelFilter::Warn,
        LevelFilter::ERROR => ::log::LevelFilter::Error,
    });
    ::log::set_logger(subscriber).unwrap();
}

pub fn init_late() {
    let subscriber = SUBSCRIBER.get().unwrap();
    let dispatch = Dispatch::from_static(subscriber);
    tracing::dispatch::set_global_default(dispatch).unwrap();
}

pub fn per_cpu_init_late(time_base: Instant) {
    TIME_BASE.set(Some(time_base));
}

struct Subscriber {
    level_filter: LevelFilter,
    output: Output<Semihosting>,
    registry: LazyLock<Registry>,
}

impl Collect for Subscriber {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(self.level_filter)
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let id = self.registry.new_span(attrs);
        let meta = attrs.metadata();

        let Some(mut writer) = self.output.writer(meta) else {
            return id;
        };

        let _ = write_level(&mut writer, *meta.level());
        let _ = write_cpu(&mut writer);
        let _ = write_timestamp(&mut writer);
        let _ = writer.indent(IndentKind::NewSpan);
        let _ = writer.with_bold().write_str(meta.name());
        let _ = writer.with_fg_color(Color::BrightBlack).write_str(": ");

        // ensure the span's fields are nicely indented if they wrap by
        // "entering" and then "exiting"`````findent`
        // the span.
        self.output.enter();
        attrs.record(&mut Visitor::new(writer.max_line_len, &mut writer));
        self.output.exit();

        id
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {
        // TODO
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {
        // TODO
    }

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();

        let Some(mut writer) = self.output.writer(meta) else {
            return;
        };

        let _ = write_level(&mut writer, *meta.level());
        let _ = write_cpu(&mut writer);
        let _ = write_timestamp(&mut writer);
        let _ = writer.indent(IndentKind::Event);
        let _ = write!(
            writer.with_fg_color(Color::BrightBlack),
            "{}: ",
            meta.target()
        );
        event.record(&mut Visitor::new(writer.max_line_len, &mut writer));
    }

    fn enter(&self, id: &Id) {
        self.registry.enter(id);
        self.output.enter();
    }

    fn exit(&self, id: &Id) {
        self.registry.exit(id);
        self.output.exit();
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.registry.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.registry.try_close(id)
    }

    fn current_span(&self) -> Current {
        self.registry.current_span()
    }
}

struct Output<W> {
    make_writer: W,
    max_line_len: usize,
}

impl<W> Output<W> {
    fn new<'a>(make_writer: W) -> Self
    where
        W: MakeWriter<'a>,
    {
        Self {
            max_line_len: make_writer.line_len() - 16,
            make_writer,
        }
    }

    #[inline]
    fn enabled<'a>(&'a self, metadata: &Metadata<'_>) -> bool
    where
        W: MakeWriter<'a>,
    {
        self.make_writer.enabled(metadata)
    }

    #[inline]
    fn enter(&self) {
        OUTPUT_INDENT.set(OUTPUT_INDENT.get() + 1);
    }

    #[inline]
    fn exit(&self) {
        let prev = OUTPUT_INDENT.replace(OUTPUT_INDENT.get() - 1);
        debug_assert!(prev > 0);
    }

    #[expect(tail_expr_drop_order, reason = "")]
    fn writer<'a>(&'a self, meta: &Metadata<'_>) -> Option<Writer<W::Writer>>
    where
        W: MakeWriter<'a>,
    {
        let writer = self.make_writer.make_writer_for(meta)?;

        Some(Writer {
            writer,
            current_line: 0,
            max_line_len: self.max_line_len,
            indent: OUTPUT_INDENT.get(),
        })
    }
}

#[inline]
fn write_level<W>(w: &mut W, level: Level) -> fmt::Result
where
    W: Write + SetColor,
{
    w.write_char('[')?;
    match level {
        Level::TRACE => w.with_fg_color(Color::Cyan).write_str("TRACE"),
        Level::DEBUG => w.with_fg_color(Color::Blue).write_str("DEBUG"),
        Level::INFO => w.with_fg_color(Color::Green).write_str("INFO "),
        Level::WARN => w.with_fg_color(Color::Yellow).write_str("WARN "),
        Level::ERROR => {
            w.set_bold(true);
            let res = w.with_fg_color(Color::Red).write_str("ERROR");
            w.set_bold(false);
            res
        }
    }?;
    w.write_char(']')
}

#[inline]
fn write_cpu<W>(w: &mut W) -> fmt::Result
where
    W: Write,
{
    w.write_fmt(format_args!("[CPU {}]", CPUID.get()))
}

#[inline]
fn write_timestamp<W>(w: &mut W) -> fmt::Result
where
    W: Write + SetColor,
{
    let time_base = TIME_BASE
        .try_with_borrow(|time_base| *time_base)
        .unwrap_or_default();

    w.write_char('[')?;
    if let Some(time_base) = time_base {
        let elapsed = time_base.elapsed();
        write!(
            w.with_fg_color(Color::BrightBlack),
            "{:>6}.{:06}",
            elapsed.as_secs(),
            elapsed.subsec_micros()
        )?;
    } else {
        write!(w.with_fg_color(Color::BrightBlack), "     ?.??????")?;
    }
    w.write_char(']')?;
    Ok(())
}

struct Visitor<'writer, W> {
    writer: &'writer mut W,
    seen: bool,
    newline: bool,
    comma: bool,
    max_line_len: usize,
}

impl<'writer, W> Visitor<'writer, W>
where
    W: Write,
    &'writer mut W: SetColor,
{
    fn new(max_line_len: usize, writer: &'writer mut W) -> Self {
        Self {
            writer,
            seen: false,
            comma: false,
            newline: false,
            max_line_len,
        }
    }

    fn record_inner(&mut self, field: &field::Field, val: &dyn fmt::Debug) {
        // XXX(eliza): sad and gross hack
        struct HasWrittenNewline<'a, W> {
            writer: &'a mut W,
            has_written_newline: bool,
            has_written_punct: bool,
        }

        impl<W: Write> Write for HasWrittenNewline<'_, W> {
            #[inline]
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.has_written_punct = s.ends_with(|ch: char| ch.is_ascii_punctuation());
                if s.contains('\n') {
                    self.has_written_newline = true;
                }
                self.writer.write_str(s)
            }
        }

        impl<W: Write> SetColor for HasWrittenNewline<'_, W>
        where
            W: SetColor,
        {
            fn fg_color(&self) -> Color {
                self.writer.fg_color()
            }

            fn set_fg_color(&mut self, color: Color) {
                self.writer.set_fg_color(color);
            }

            fn set_bold(&mut self, bold: bool) {
                self.writer.set_bold(bold);
            }
        }

        let mut writer = HasWrittenNewline {
            writer: &mut self.writer,
            has_written_newline: false,
            has_written_punct: false,
        };
        let nl = if self.newline { '\n' } else { ' ' };

        if field.name() == "message" {
            if self.seen {
                let _ = write!(writer.with_bold(), "{nl}{val:?}");
            } else {
                let _ = write!(writer.with_bold(), "{val:?}");
                self.comma = !writer.has_written_punct;
            }
            self.seen = true;
            return;
        }

        if self.comma {
            let _ = writer.with_fg_color(Color::BrightBlack).write_char(',');
        }

        if self.seen {
            let _ = writer.write_char(nl);
        }

        if !self.comma {
            self.seen = true;
            self.comma = true;
        }

        // pretty-print the name with dots in the punctuation color
        let mut name_pieces = field.name().split('.');
        if let Some(piece) = name_pieces.next() {
            let _ = writer.write_str(piece);
            for piece in name_pieces {
                let _ = writer.with_fg_color(Color::BrightBlack).write_char('.');
                let _ = writer.write_str(piece);
            }
        }

        let _ = writer.with_fg_color(Color::BrightBlack).write_char('=');
        let _ = write!(writer, "{val:?}");
        self.newline |= writer.has_written_newline;
    }
}

impl<'writer, W> field::Visit for Visitor<'writer, W>
where
    W: Write,
    &'writer mut W: SetColor,
{
    #[inline]
    fn record_u64(&mut self, field: &field::Field, val: u64) {
        self.record_inner(field, &val);
    }

    #[inline]
    fn record_i64(&mut self, field: &field::Field, val: i64) {
        self.record_inner(field, &val);
    }

    #[inline]
    fn record_bool(&mut self, field: &field::Field, val: bool) {
        self.record_inner(field, &val);
    }

    #[inline]
    fn record_str(&mut self, field: &field::Field, val: &str) {
        if val.len() >= self.max_line_len {
            self.newline = true;
        }
        self.record_inner(field, &val);
    }

    fn record_debug(&mut self, field: &field::Field, val: &dyn fmt::Debug) {
        self.record_inner(field, val);
    }
}
