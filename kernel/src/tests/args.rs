// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use ktest::Test;

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
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(str: &'a str) -> Self {
        Self::parse(str.split_ascii_whitespace())
    }

    pub fn parse(mut iter: impl Iterator<Item = &'a str>) -> Self {
        let mut out = Self::default();

        while let Some(str) = iter.next() {
            match str {
                "--list" => out.list = true,
                "--include-ignored" => out.include_ignored = true,
                "--ignored" => out.ignored = true,
                "--exact" => out.exact = true,
                "--format" => match iter.next().unwrap() {
                    "pretty" => out.format = FormatSetting::Pretty,
                    "terse" => out.format = FormatSetting::Terse,
                    "json" => out.format = FormatSetting::Json,
                    _ => {}
                },
                _ => out.test_name = Some(str),
            }
        }

        out
    }

    /// Returns `true` if the given test should be ignored.
    pub fn is_ignored(&self, test: &Test) -> bool {
        test.info.ignored && !self.ignored && !self.include_ignored
    }
}
