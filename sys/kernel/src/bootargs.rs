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
//! `--log`/`--backtrace`/`--heap-max` here; the test runner reads
//! `--list`/`--test-name` etc. via [`crate::tests::args`].
//!
//! Parsing walks tokens left-to-right and dispatches each one to a matching
//! [`Flag`] via [`Flag::consume`], mirroring the dispatch loop in
//! [`crate::shell`].

use anyhow::{Context, anyhow};

use crate::backtrace::BacktraceStyle;
use crate::device_tree::DeviceTree;
use crate::tracing::Filter;

pub const LOG: Flag =
    Flag::new_string("--log").with_help("log filter, e.g. `info` or `kasync=trace,debug`");

pub const BACKTRACE: Flag =
    Flag::new_string("--backtrace").with_help("backtrace style on panic: `short` or `full`");

pub const HEAP_MAX: Flag =
    Flag::new_bytes("--heap-max").with_help("hard cap on the kernel heap, e.g. `512M` or `2G`");

pub fn parse(devtree: &DeviceTree) -> crate::Result<Bootargs> {
    let mut bootargs = Bootargs::default();
    let mut tokens = read_raw(devtree)?.split_ascii_whitespace();

    while let Some(tok) = tokens.next() {
        if let Some(v) = LOG.consume(tok, &mut tokens) {
            bootargs.log = v.parse().context(LOG.name)?;
        } else if let Some(v) = BACKTRACE.consume(tok, &mut tokens) {
            bootargs.backtrace = v.parse().context(BACKTRACE.name)?;
        } else if let Some(v) = HEAP_MAX.consume(tok, &mut tokens) {
            bootargs.heap_max = Some(parse_size(v).context(HEAP_MAX.name)?);
        }
    }

    Ok(bootargs)
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
    /// Hard cap on the kernel heap, in bytes. `None` means use the built-in default.
    pub heap_max: Option<usize>,
}

/// A bootarg flag declaration. Built with the const-builder, e.g.
/// `Flag::new("--list").with_help("...")`.
pub struct Flag {
    name: &'static str,
    help: &'static str,
    kind: FlagKind,
}

/// Shape of a flag's value, used to render help text via [`FlagKind::hint`].
#[derive(Copy, Clone)]
pub enum FlagKind {
    /// `--name`
    Bool,
    /// `--name <bytes>[K|M|G|T]` — IEC binary units; parsed via [`parse_size`].
    Bytes,
    /// `--name <value>` — opaque string, parsed by the consumer via [`FromStr`].
    ///
    /// [`FromStr`]: core::str::FromStr
    String,
}

impl FlagKind {
    /// Help-text hint for the value, e.g. `"<bytes>[K|M|G|T]"`. Empty for `Bool`.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Bool => "",
            Self::Bytes => "<bytes>[K|M|G|T]",
            Self::String => "<value>",
        }
    }
}

impl Flag {
    #[must_use]
    const fn new(name: &'static str, kind: FlagKind) -> Self {
        Self {
            name,
            help: "",
            kind,
        }
    }

    #[must_use]
    pub const fn new_bool(name: &'static str) -> Self {
        Self::new(name, FlagKind::Bool)
    }

    #[must_use]
    pub const fn new_string(name: &'static str) -> Self {
        Self::new(name, FlagKind::String)
    }

    #[must_use]
    pub const fn new_bytes(name: &'static str) -> Self {
        Self::new(name, FlagKind::Bytes)
    }

    #[must_use]
    pub const fn with_help(self, help: &'static str) -> Self {
        Self { help, ..self }
    }

    /// If `tok` matches this flag (`--name=value` or starting the `--name <value>` pair),
    /// return the value, advancing `rest` to consume the value when it lives in the next
    /// token. Returns `None` if the flag doesn't match.
    pub fn consume<'a, I: Iterator<Item = &'a str>>(
        &self,
        tok: &'a str,
        rest: &mut I,
    ) -> Option<&'a str> {
        let after = tok.strip_prefix(self.name)?;

        if after.is_empty() {
            match self.kind {
                // a bool flag should not be followed by more input
                FlagKind::Bool => Some(""),
                _ => rest.next(),
            }
        } else {
            match self.kind {
                // a bool flag should not be followed by more input
                FlagKind::Bool => None,
                _ => after.strip_prefix('='),
            }
        }
    }
}

/// Parse a byte count with an optional `K`/`M`/`G`/`T` (or lowercase) suffix in IEC binary
/// units (1K = 1024, 1M = 1024², etc.). A bare integer is treated as bytes; whitespace around
/// the suffix is not permitted.
fn parse_size(s: &str) -> Result<usize, anyhow::Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("empty size"));
    }

    let (digits, multiplier): (&str, usize) = match s.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&s[..s.len() - 1], 1024),
        Some(b'm' | b'M') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        Some(b't' | b'T') => (&s[..s.len() - 1], 1024 * 1024 * 1024 * 1024),
        _ => (s, 1),
    };

    let n: usize = digits
        .parse()
        .map_err(|_| anyhow!("not a valid integer: {digits:?}"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("size overflow"))
}
