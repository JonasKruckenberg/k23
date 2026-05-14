// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use anyhow::bail;
use test::Test;

use crate::bootargs::Flag;

pub const LIST: Flag = Flag::new_bool("--list").with_help("print the list of tests, then exit");
pub const INCLUDE_IGNORED: Flag =
    Flag::new_bool("--include-ignored").with_help("run ignored tests alongside the rest");
pub const IGNORED: Flag = Flag::new_bool("--ignored").with_help("only run ignored tests");
pub const EXACT: Flag =
    Flag::new_bool("--exact").with_help("treat `--test-name` as an exact match");
pub const FORMAT: Flag =
    Flag::new_string("--format").with_help("output format: `pretty`, `terse`, or `json`");
pub const FILTER: Flag =
    Flag::new_string("--filter").with_help("substring filter applied to test idents");

#[derive(Default, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "its fiiiine")]
pub struct Arguments<'a> {
    pub test_name: Option<&'a str>,
    pub list: bool,
    pub include_ignored: bool,
    pub ignored: bool,
    pub exact: bool,
    pub format: FormatSetting,
}

#[derive(Debug, Default, Copy, Clone)]
pub enum FormatSetting {
    #[default]
    Pretty,
    Terse,
    Json,
}

impl<'a> Arguments<'a> {
    pub fn parse(raw: &'a str) -> crate::Result<Self> {
        let mut args = Self::default();
        let mut tokens = raw.split_ascii_whitespace();

        while let Some(tok) = tokens.next() {
            if LIST.consume(tok, &mut tokens).is_some() {
                args.list = true;
            } else if INCLUDE_IGNORED.consume(tok, &mut tokens).is_some() {
                args.include_ignored = true;
            } else if IGNORED.consume(tok, &mut tokens).is_some() {
                args.ignored = true;
            } else if EXACT.consume(tok, &mut tokens).is_some() {
                args.exact = true;
            } else if let Some(v) = FORMAT.consume(tok, &mut tokens) {
                args.format = match v {
                    "pretty" => FormatSetting::Pretty,
                    "terse" => FormatSetting::Terse,
                    "json" => FormatSetting::Json,
                    fmt => bail!(
                        "invalid output format \"{fmt}\". Expected one of \"pretty\", \"terse\", or \"json\"."
                    ),
                };
            } else if let Some(v) = FILTER.consume(tok, &mut tokens) {
                args.test_name = Some(v);
            }
        }

        Ok(args)
    }

    /// Returns `true` if the given test should be ignored.
    pub fn is_ignored(&self, test: &Test) -> bool {
        test.info.ignored && !self.ignored && !self.include_ignored
    }
}
