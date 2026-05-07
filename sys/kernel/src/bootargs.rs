// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Boot argument parsing.
//!
//! `/chosen/bootargs` is whitespace-separated; every recognised flag is
//! prefixed with `--` (clap-style). `--name value` and `--name=value` are
//! both accepted. Tokens not matched by the caller are ignored, so multiple
//! subsystems can consume the same string independently — the kernel reads
//! `--log`/`--backtrace` here; the test runner reads `--list`/`--test-name`
//! etc. via [`crate::tests::args`].

use core::str::FromStr;

use anyhow::Context;

use crate::backtrace::BacktraceStyle;
use crate::device_tree::DeviceTree;
use crate::tracing::Filter;

pub const LOG: Flag = Flag::new("--log")
    .with_value()
    .with_help("log filter, e.g. `info` or `kasync=trace,debug`");

pub const BACKTRACE: Flag = Flag::new("--backtrace")
    .with_value()
    .with_help("backtrace style on panic: `short` or `full`");

pub fn parse(devtree: &DeviceTree) -> crate::Result<Bootargs> {
    let parser = Parser::new(read_raw(devtree)?);
    Ok(Bootargs {
        log: parser.value_or_default(LOG.name).context(LOG.name)?,
        backtrace: parser
            .value_or_default(BACKTRACE.name)
            .context(BACKTRACE.name)?,
    })
}

pub fn read_raw(devtree: &DeviceTree) -> crate::Result<&str> {
    let chosen = devtree
        .find_by_path("/chosen")
        .context("missing /chosen node")?;
    Ok(chosen
        .property("bootargs")
        .map(|p| p.as_str())
        .transpose()?
        .unwrap_or(""))
}

#[derive(Default)]
pub struct Bootargs {
    pub log: Filter,
    pub backtrace: BacktraceStyle,
}

/// A bootarg flag declaration. Built with the const-builder, e.g.
/// `Flag::new("--list").with_help("...")`. Modeled on `Command` in
/// [`crate::shell`].
pub struct Flag<'a> {
    pub name: &'a str,
    pub help: &'a str,
    pub kind: FlagKind,
}

#[derive(Copy, Clone)]
pub enum FlagKind {
    /// `--name`
    Bool,
    /// `--name <value>` or `--name=<value>`
    Value,
}

impl<'a> Flag<'a> {
    #[must_use]
    pub const fn new(name: &'a str) -> Self {
        Self {
            name,
            help: "",
            kind: FlagKind::Bool,
        }
    }

    #[must_use]
    pub const fn with_help(self, help: &'a str) -> Self {
        Self { help, ..self }
    }

    #[must_use]
    pub const fn with_value(self) -> Self {
        Self {
            kind: FlagKind::Value,
            ..self
        }
    }
}

/// Lookup-only parser over a raw bootargs string.
pub struct Parser<'a>(&'a str);

impl<'a> Parser<'a> {
    pub fn new(raw: &'a str) -> Self {
        Self(raw)
    }

    /// `true` if `--name` appears as a standalone token.
    pub fn flag(&self, name: &str) -> bool {
        self.0.split_ascii_whitespace().any(|t| t == name)
    }

    /// Returns the value for `--name <value>` or `--name=<value>`, if set.
    pub fn value(&self, name: &str) -> Option<&'a str> {
        let mut toks = self.0.split_ascii_whitespace();
        while let Some(tok) = toks.next() {
            if tok == name {
                return toks.next();
            }
            if let Some(rest) = tok.strip_prefix(name)
                && let Some(v) = rest.strip_prefix('=')
            {
                return Some(v);
            }
        }
        None
    }

    /// Parses `--name <value>` via [`FromStr`]; defaults if the flag is absent.
    pub fn value_or_default<T>(&self, name: &str) -> Result<T, T::Err>
    where
        T: FromStr + Default,
    {
        self.value(name)
            .map(T::from_str)
            .transpose()
            .map(Option::unwrap_or_default)
    }
}
