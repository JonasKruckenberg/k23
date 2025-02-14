// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use log::Record;
use sync::LazyLock;
use tracing::field;
use tracing_core::{dispatch, identify_callsite, Callsite, Collect, Event, Kind, Level, Metadata};

impl log::Log for super::Subscriber {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if let Some(level_filter) = self.max_level_hint() {
            loglevel_to_level(metadata.level()) <= level_filter
        } else {
            false
        }
    }

    fn log(&self, record: &Record) {
        dispatch::get_default(|dispatch| {
            let (cs, keys, meta) = loglevel_to_callsite(record.level());
            let cs_id = identify_callsite!(cs);

            let filter_meta = Metadata::new(
                "log record",
                record.target(),
                loglevel_to_level(record.level()),
                None,
                None,
                None,
                field::FieldSet::new(FIELD_NAMES, cs_id),
                Kind::EVENT,
            );

            if !dispatch.enabled(&filter_meta) {
                return;
            }

            let log_module = record.module_path();
            let log_file = record.file();
            let log_line = record.line();

            let module = log_module.as_ref().map(|s| s as &dyn field::Value);
            let file = log_file.as_ref().map(|s| s as &dyn field::Value);
            let line = log_line.as_ref().map(|s| s as &dyn field::Value);

            dispatch.event(&Event::new(
                meta,
                &meta.fields().value_set(&[
                    (&keys.message, Some(record.args() as &dyn field::Value)),
                    (&keys.target, Some(&record.target())),
                    (&keys.module, module),
                    (&keys.file, file),
                    (&keys.line, line),
                ]),
            ));
        });
    }

    fn flush(&self) {}
}

struct Fields {
    message: field::Field,
    target: field::Field,
    module: field::Field,
    file: field::Field,
    line: field::Field,
}

static FIELD_NAMES: &[&str] = &[
    "message",
    "log.target",
    "log.module_path",
    "log.file",
    "log.line",
];

impl Fields {
    fn new(cs: &'static dyn Callsite) -> Self {
        let fieldset = cs.metadata().fields();
        let message = fieldset.field("message").unwrap();
        let target = fieldset.field("log.target").unwrap();
        let module = fieldset.field("log.module_path").unwrap();
        let file = fieldset.field("log.file").unwrap();
        let line = fieldset.field("log.line").unwrap();
        Fields {
            message,
            target,
            module,
            file,
            line,
        }
    }
}

fn loglevel_to_level(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::ERROR,
        log::Level::Warn => Level::WARN,
        log::Level::Info => Level::INFO,
        log::Level::Debug => Level::DEBUG,
        log::Level::Trace => Level::TRACE,
    }
}

macro_rules! log_cs {
    ($level:expr, $cs:ident, $meta:ident, $ty:ident) => {
        struct $ty;
        static $cs: $ty = $ty;
        static $meta: Metadata<'static> = Metadata::new(
            "log event",
            "log",
            $level,
            ::core::option::Option::None,
            ::core::option::Option::None,
            ::core::option::Option::None,
            ::tracing_core::field::FieldSet::new(
                FIELD_NAMES,
                ::tracing_core::identify_callsite!(&$cs),
            ),
            ::tracing_core::metadata::Kind::EVENT,
        );

        impl tracing_core::callsite::Callsite for $ty {
            fn set_interest(&self, _: ::tracing_core::Interest) {}
            fn metadata(&self) -> &'static Metadata<'static> {
                &$meta
            }
        }
    };
}

log_cs!(Level::TRACE, TRACE_CS, TRACE_META, TraceCallsite);
log_cs!(Level::DEBUG, DEBUG_CS, DEBUG_META, DebugCallsite);
log_cs!(Level::INFO, INFO_CS, INFO_META, InfoCallsite);
log_cs!(Level::WARN, WARN_CS, WARN_META, WarnCallsite);
log_cs!(Level::ERROR, ERROR_CS, ERROR_META, ErrorCallsite);

static TRACE_FIELDS: LazyLock<Fields> = LazyLock::new(|| Fields::new(&TRACE_CS));
static DEBUG_FIELDS: LazyLock<Fields> = LazyLock::new(|| Fields::new(&DEBUG_CS));
static INFO_FIELDS: LazyLock<Fields> = LazyLock::new(|| Fields::new(&INFO_CS));
static WARN_FIELDS: LazyLock<Fields> = LazyLock::new(|| Fields::new(&WARN_CS));
static ERROR_FIELDS: LazyLock<Fields> = LazyLock::new(|| Fields::new(&ERROR_CS));

fn loglevel_to_callsite(
    level: log::Level,
) -> (
    &'static dyn Callsite,
    &'static Fields,
    &'static Metadata<'static>,
) {
    match level {
        log::Level::Trace => (&TRACE_CS, &*TRACE_FIELDS, &TRACE_META),
        log::Level::Debug => (&DEBUG_CS, &*DEBUG_FIELDS, &DEBUG_META),
        log::Level::Info => (&INFO_CS, &*INFO_FIELDS, &INFO_META),
        log::Level::Warn => (&WARN_CS, &*WARN_FIELDS, &WARN_META),
        log::Level::Error => (&ERROR_CS, &*ERROR_FIELDS, &ERROR_META),
    }
}
