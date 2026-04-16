// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use ktest::{Test, TestInfo};

use super::args::FormatSetting;
use super::{Conclusion, Outcome};

pub struct Printer {
    format: FormatSetting,
}

impl Printer {
    pub fn new(format: FormatSetting) -> Self {
        Self { format }
    }

    pub(crate) fn print_title(&self, num_tests: u64) {
        match self.format {
            FormatSetting::Pretty | FormatSetting::Terse => {
                let plural_s = if num_tests == 1 { "" } else { "s" };

                tracing::info!("\nrunning {num_tests} test{plural_s}");
            }
            FormatSetting::Json => tracing::info!(
                r#"{{ "type": "suite", "event": "started", "test_count": {num_tests} }}"#,
            ),
        }
    }

    pub(crate) fn print_test(&self, info: &TestInfo) {
        let TestInfo { ident, .. } = info;
        match self.format {
            FormatSetting::Pretty => {
                tracing::info!("test {ident} ... ",);
            }
            FormatSetting::Terse => {
                // In terse mode, nothing is printed before the job. Only
                // `print_single_outcome` prints one character.
            }
            FormatSetting::Json => {
                tracing::info!(r#"{{ "type": "test", "event": "started", "name": "{ident}" }}"#,)
            }
        }
    }

    pub(crate) fn print_single_outcome(&self, info: &TestInfo, outcome: &Outcome) {
        let TestInfo { ident, .. } = info;
        match self.format {
            FormatSetting::Pretty => {
                self.print_outcome_pretty(outcome);
            }
            FormatSetting::Terse => {
                let c = match outcome {
                    Outcome::Passed => '.',
                    Outcome::Failed { .. } => 'F',
                    Outcome::Ignored => 'i',
                };

                tracing::info!("{c}");
            }
            FormatSetting::Json => {
                tracing::info!(
                    r#"{{ "type": "test", "name": "{ident}", "event": "{}" }}"#,
                    match outcome {
                        Outcome::Passed => "ok",
                        Outcome::Failed(_) => "failed",
                        Outcome::Ignored => "ignored",
                    }
                );
            }
        }
    }

    fn print_outcome_pretty(&self, outcome: &Outcome) {
        match outcome {
            Outcome::Passed => tracing::info!("ok"),
            Outcome::Failed(_) => {
                tracing::info!("FAILED");
            }
            Outcome::Ignored => tracing::info!("ignored"),
        }
    }

    pub(crate) fn print_list<'a>(&self, tests: impl Iterator<Item = &'a Test>, ignored: bool) {
        for test in tests {
            // libtest prints out:
            // * all tests without `--ignored`
            // * just the ignored tests with `--ignored`
            if ignored && !test.info.ignored {
                continue;
            }

            tracing::info!("{}: test", test.info.ident);
        }
    }

    pub(crate) fn print_summary(&self, conclusion: &Conclusion) {
        match self.format {
            FormatSetting::Pretty | FormatSetting::Terse => {
                let outcome = if conclusion.has_failed() {
                    Outcome::Failed(Box::new(()))
                } else {
                    Outcome::Passed
                };

                tracing::info!("test result: ");
                self.print_outcome_pretty(&outcome);
                tracing::info!(
                    "{} passed; {} failed; {} ignored; {} measured; \
                        {} filtered out",
                    conclusion.num_passed.load(Ordering::Acquire),
                    conclusion.num_failed.load(Ordering::Acquire),
                    conclusion.num_ignored.load(Ordering::Acquire),
                    conclusion.num_measured.load(Ordering::Acquire),
                    conclusion.num_filtered_out.load(Ordering::Acquire),
                );
            }
            FormatSetting::Json => {
                tracing::info!(
                    concat!(
                        r#"{{ "type": "suite", "event": "{}", "passed": {}, "failed": {},"#,
                        r#" "ignored": {}, "measured": {}, "filtered_out": {} }}"#,
                    ),
                    if conclusion.has_failed() {
                        "failed"
                    } else {
                        "ok"
                    },
                    conclusion.num_passed.load(Ordering::Acquire),
                    conclusion.num_failed.load(Ordering::Acquire),
                    conclusion.num_ignored.load(Ordering::Acquire),
                    conclusion.num_measured.load(Ordering::Acquire),
                    conclusion.num_filtered_out.load(Ordering::Acquire),
                )
            }
        }
    }
}
