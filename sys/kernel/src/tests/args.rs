// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use test::Test;

use crate::bootargs::{Flag, Parser};

pub const LIST: Flag = Flag::new("--list").with_help("print the list of tests, then exit");
pub const INCLUDE_IGNORED: Flag =
    Flag::new("--include-ignored").with_help("run ignored tests alongside the rest");
pub const IGNORED: Flag = Flag::new("--ignored").with_help("only run ignored tests");
pub const EXACT: Flag = Flag::new("--exact").with_help("treat `--test-name` as an exact match");
pub const FORMAT: Flag = Flag::new("--format")
    .with_value()
    .with_help("output format: `pretty`, `terse`, or `json`");
pub const TEST_NAME: Flag = Flag::new("--test-name")
    .with_value()
    .with_help("substring filter applied to test idents");

#[derive(Default)]
pub struct Arguments<'a> {
    pub test_name: Option<&'a str>,
    pub list: bool,
    pub include_ignored: bool,
    pub ignored: bool,
    pub exact: bool,
    pub format: FormatSetting,
}

#[derive(Default, Copy, Clone)]
pub enum FormatSetting {
    #[default]
    Pretty,
    Terse,
    Json,
}

impl<'a> Arguments<'a> {
    pub fn parse(raw: &'a str) -> Self {
        let parser = Parser::new(raw);
        Self {
            list: parser.flag(LIST.name),
            include_ignored: parser.flag(INCLUDE_IGNORED.name),
            ignored: parser.flag(IGNORED.name),
            exact: parser.flag(EXACT.name),
            format: parser
                .value(FORMAT.name)
                .and_then(|v| match v {
                    "pretty" => Some(FormatSetting::Pretty),
                    "terse" => Some(FormatSetting::Terse),
                    "json" => Some(FormatSetting::Json),
                    _ => None,
                })
                .unwrap_or_default(),
            test_name: parser.value(TEST_NAME.name),
        }
    }

    /// Returns `true` if the given test should be ignored.
    pub fn is_ignored(&self, test: &Test) -> bool {
        test.info.ignored && !self.ignored && !self.include_ignored
    }
}
