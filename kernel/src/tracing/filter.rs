// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::fmt::Formatter;
use core::str::FromStr;
use fallible_iterator::{FallibleIterator, IteratorExt};
use smallvec::SmallVec;
use tracing::level_filters::STATIC_MAX_LEVEL;
use tracing_core::metadata::ParseLevelFilterError;
use tracing_core::{Level, LevelFilter, Metadata};

#[derive(Debug)]
pub enum Error {
    UnexpectedEof,
    TooManyEqualSigns,
    InvalidLevelFilter,
}

impl From<ParseLevelFilterError> for Error {
    fn from(_: ParseLevelFilterError) -> Self {
        Self::InvalidLevelFilter
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => writeln!(f, "input must not be empty"),
            Error::TooManyEqualSigns => {
                writeln!(f, "too many '=' in filter directive, expected 0 or 1")
            }
            // Error::TooManyFieldListBegins => {
            //     writeln!(f, "too many '[{{' in filter directive, expected 0 or 1")
            // }
            // Error::MissingFieldListEnd => writeln!(f, "expected fields list to end with '}}]'"),
            Error::InvalidLevelFilter => writeln!(f, "encountered invalid level filter str"),
        }
    }
}

impl core::error::Error for Error {}

pub struct Filter {
    directives: SmallVec<[Directive; 8]>,
    max_level: LevelFilter,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            directives: SmallVec::new(),
            max_level: LevelFilter::DEBUG,
        }
    }
}

impl FromStr for Filter {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::default());
        }

        let iter = s
            .split(',')
            .filter(|s| !s.is_empty())
            .map(Directive::from_str)
            .transpose_into_fallible();

        Self::from_directives(iter)
    }
}

impl Filter {
    fn from_directives(
        mut directives: impl FallibleIterator<Item = Directive, Error = Error>,
    ) -> Result<Self, Error> {
        let mut disabled = Vec::new();
        let mut enabled = SmallVec::new();
        let mut max_level = LevelFilter::OFF;

        while let Some(directive) = directives.next()? {
            if directive.level > STATIC_MAX_LEVEL {
                disabled.push(directive);
            } else {
                if directive.level > max_level {
                    max_level = directive.level;
                }

                // insert the directive into the vec of directives, ordered by
                // specificity (length of target + number of field filters). this
                // ensures that, when finding a directive to match a span or event, we
                // search the directive set in most specific first order.
                match enabled.binary_search(&directive) {
                    Ok(i) => enabled[i] = directive,
                    Err(i) => enabled.insert(i, directive),
                }
            }
        }

        if !disabled.is_empty() {
            tracing::warn!(
                "some trace filter directives would enable traces that are disabled statically"
            );
            for directive in disabled {
                let target = if let Some(target) = &directive.target {
                    format!("the `{target}` target")
                } else {
                    "all targets".into()
                };
                let level = directive
                    .level
                    .into_level()
                    .expect("=off would not have enabled any filters");

                tracing::warn!("`{directive:?}` would enable the {level} level for {target}");
            }

            tracing::warn!("the static max level is `{STATIC_MAX_LEVEL}`");

            let help_msg = || {
                let (feature, filter) = match STATIC_MAX_LEVEL.into_level() {
                    Some(Level::TRACE) => unreachable!(
                        "if the max level is trace, no static filtering features are enabled"
                    ),
                    Some(Level::DEBUG) => ("max_level_debug", Level::TRACE),
                    Some(Level::INFO) => ("max_level_info", Level::DEBUG),
                    Some(Level::WARN) => ("max_level_warn", Level::INFO),
                    Some(Level::ERROR) => ("max_level_error", Level::WARN),
                    None => return ("max_level_off", String::new()),
                };
                (feature, format!("{filter} "))
            };
            let (feature, earlier_level) = help_msg();
            tracing::warn!(
                "to enable {earlier_level}logging, remove the `{feature}` feature from the `tracing` crate"
            );
        }

        tracing::debug!("{enabled:?} {max_level:?}");

        Ok(Self {
            directives: enabled,
            max_level,
        })
    }

    pub fn max_level(&self) -> LevelFilter {
        self.max_level
    }

    pub(super) fn enabled(&self, meta: &Metadata<'_>) -> bool {
        let level = *meta.level();
        if self.max_level < level {
            return false;
        };

        match self.directives_for(meta).next() {
            Some(d) => d.level >= level,
            None => true,
        }
    }

    fn directives_for<'a>(
        &self,
        meta: &'a Metadata<'a>,
    ) -> impl Iterator<Item = &Directive> + use<'_, 'a> {
        self.directives.iter().filter(|d| d.cares_about(meta))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Directive {
    level: LevelFilter,
    target: Option<String>,
}

impl Directive {
    fn cares_about(&self, meta: &Metadata<'_>) -> bool {
        // Does this directive have a target filter, and does it match the
        // metadata's target?
        if let Some(ref target) = self.target
            && !meta.target().starts_with(&target[..])
        {
            return false;
        }

        true
    }
}

impl FromStr for Directive {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // This method parses a filtering directive in one of the following
        // forms:
        //
        // * `foo=trace` (TARGET=LEVEL)
        // * `trace` (bare LEVEL)
        // * `foo` (bare TARGET)
        let mut split = s.split('=');
        let part0 = split.next().ok_or(Error::UnexpectedEof)?;

        // Directive includes an `=`:
        // * `foo=trace`
        if let Some(part1) = split.next() {
            if split.next().is_some() {
                return Err(Error::TooManyEqualSigns);
            }

            let mut split = part0.split("[{");
            let target = split.next().map(String::from);

            let level = part1.parse()?;
            return Ok(Self { level, target });
        }

        // Okay, the part after the `=` was empty, the directive is either a
        // bare level or a bare target.
        // * `foo`
        // * `info`
        Ok(match part0.parse::<LevelFilter>() {
            Ok(level) => Self {
                level,
                target: None,
            },
            Err(_) => Self {
                target: Some(String::from(part0)),
                level: LevelFilter::TRACE,
            },
        })
    }
}

impl PartialOrd<Self> for Directive {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Directive {
    fn cmp(&self, other: &Self) -> Ordering {
        // We attempt to order directives by how "specific" they are. This
        // ensures that we try the most specific directives first when
        // attempting to match a piece of metadata.

        // First, we compare based on whether a target is specified, and the
        // lengths of those targets if both have targets.
        let ordering = self
            .target
            .as_ref()
            .map(String::len)
            .cmp(&other.target.as_ref().map(String::len))
            .reverse();

        #[cfg(debug_assertions)]
        {
            if ordering == Ordering::Equal {
                debug_assert_eq!(
                    self.target, other.target,
                    "invariant violated: Ordering::Equal must imply a.target == b.target"
                );
            }
        }

        ordering
    }
}
