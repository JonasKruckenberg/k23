// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod color;
mod filter;
mod log;
mod registry;
mod writer;

use crate::state::try_global;
use crate::tracing::writer::{MakeWriter, Semihosting};
pub use ::tracing::*;
use color::{Color, SetColor};
use core::cell::{Cell, OnceCell};
use core::fmt;
use core::fmt::Write;
use cpu_local::cpu_local;
pub use filter::Filter;
use registry::Registry;
use spin::OnceLock;
use tracing::field;
use tracing_core::span::{Attributes, Current, Id, Record};
use tracing_core::{Collect, Dispatch, Event, Interest, Level, LevelFilter, Metadata};

static SUBSCRIBER: OnceLock<Subscriber> = OnceLock::new();

cpu_local! {
    /// Per-cpu indentation representing the span depth we're currently in
    static OUTPUT_INDENT: Cell<usize> = Cell::new(0);
    static CPUID: OnceCell<usize> = OnceCell::new();
}

pub fn per_cpu_init_early(cpuid: usize) {
    CPUID.get_or_init(|| cpuid);
}

/// Perform early initialization of the tracing subsystem. This will enable printing of `log` and `span`
/// events, but no spans yet.
///
/// This should be called as early in the boot process as possible.
pub fn init_early() {
    let subscriber = SUBSCRIBER.get_or_init(|| Subscriber {
        // level_filter,
        output: Output::new(Semihosting::new()),
        lateinit: OnceLock::new(),
    });
    ::log::set_logger(subscriber).unwrap();

    let subscriber = SUBSCRIBER.get().unwrap();
    let dispatch = Dispatch::from_static(subscriber);
    dispatch::set_global_default(dispatch).unwrap();
}

/// Fully initialize the subsystem, after this point tracing [`Span`]s will be processed as well.
pub fn init(filter: Filter) {
    let subscriber = SUBSCRIBER
        .get()
        .expect("tracing::init must be called after tracing::init_early");

    ::log::set_max_level(match filter.max_level() {
        LevelFilter::OFF => ::log::LevelFilter::Off,
        LevelFilter::TRACE => ::log::LevelFilter::Trace,
        LevelFilter::DEBUG => ::log::LevelFilter::Debug,
        LevelFilter::INFO => ::log::LevelFilter::Info,
        LevelFilter::WARN => ::log::LevelFilter::Warn,
        LevelFilter::ERROR => ::log::LevelFilter::Error,
    });

    subscriber
        .lateinit
        .get_or_init(|| (Registry::default(), filter));
}

struct Subscriber {
    // level_filter: LevelFilter,
    output: Output<Semihosting>,
    lateinit: OnceLock<(Registry, Filter)>,
}

impl Collect for Subscriber {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if self.enabled(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        if let Some((_, filter)) = self.lateinit.get() {
            filter.enabled(meta)
        } else {
            true
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        if let Some((_, filter)) = self.lateinit.get() {
            Some(filter.max_level())
        } else {
            None
        }
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let Some((registry, _)) = self.lateinit.get() else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
            return Id::from_u64(0xDEAD);
        };

        let id = registry.new_span(attrs);
        let meta = attrs.metadata();

        let Some(mut writer) = self.output.writer(meta) else {
            return id;
        };

        let _ = write_level(&mut writer, *meta.level());
        let _ = write_cpu(&mut writer);
        let _ = write_timestamp(&mut writer);

        // let _ = writer.indent(IndentKind::NewSpan);
        let _ = write!(
            writer.with_fg_color(Color::BrightBlack),
            "{}: ",
            meta.target()
        );
        let _ = writer.with_bold().write_str(meta.name());
        let _ = writer.with_fg_color(Color::BrightBlack).write_str(": ");

        // ensure the span's fields are nicely indented if they wrap by
        // "entering" and then "exiting"`````findent`
        // the span.
        self.output.enter();
        attrs.record(&mut Visitor::new(180, &mut writer));
        self.output.exit();

        let _ = writer.write_char('\n');

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
        // let _ = writer.indent(IndentKind::Event);
        let _ = write!(
            writer.with_fg_color(Color::BrightBlack),
            "{}: ",
            meta.target()
        );
        event.record(&mut Visitor::new(180, &mut writer));
        let _ = writer.write_char('\n');
    }

    fn enter(&self, id: &Id) {
        self.output.enter();
        if let Some((registry, _)) = self.lateinit.get() {
            registry.enter(id);
        } else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
        }
    }

    fn exit(&self, id: &Id) {
        self.output.exit();
        if let Some((registry, _)) = self.lateinit.get() {
            registry.exit(id);
        } else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
        }
    }

    fn clone_span(&self, id: &Id) -> Id {
        if let Some((registry, _)) = self.lateinit.get() {
            registry.clone_span(id)
        } else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
            Id::from_u64(0xDEAD)
        }
    }

    fn try_close(&self, id: Id) -> bool {
        if let Some((registry, _)) = self.lateinit.get() {
            registry.try_close(id)
        } else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
            false
        }
    }

    fn current_span(&self) -> Current {
        if let Some((registry, _)) = self.lateinit.get() {
            registry.current_span()
        } else {
            tracing::warn!("tracing spans be only be tracked *after* tracing::init is called");
            Current::none()
        }
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

    fn writer<'a>(&'a self, meta: &Metadata<'_>) -> Option<W::Writer>
    where
        W: MakeWriter<'a>,
    {
        self.make_writer.make_writer_for(meta)

        // Some(Writer {
        //     writer,
        //     current_line: 0,
        //     max_line_len: self.max_line_len,
        //     indent: OUTPUT_INDENT.get(),
        // })
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
    if let Some(cpuid) = CPUID.get() {
        w.write_fmt(format_args!("[CPU {cpuid}]"))
    } else {
        w.write_fmt(format_args!("[CPU ??]"))
    }
}

#[inline]
fn write_timestamp<W>(w: &mut W) -> fmt::Result
where
    W: Write + SetColor,
{
    w.write_char('[')?;

    if let Some(global) = try_global() {
        let elapsed = global.time_origin.elapsed(&global.timer);
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
