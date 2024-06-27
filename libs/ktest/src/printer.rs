use crate::args::FormatSetting;
use crate::assert::Failed;
use crate::{Conclusion, Outcome, Test, TestInfo};
use core::fmt;

pub struct Printer<'a> {
    out: &'a mut dyn fmt::Write,
    format: FormatSetting,
}

impl<'a> Printer<'a> {
    pub fn new(out: &'a mut dyn fmt::Write, format: FormatSetting) -> Self {
        Self { out, format }
    }

    pub(crate) fn print_title(&mut self, num_tests: u64) {
        match self.format {
            FormatSetting::Pretty | FormatSetting::Terse => {
                let plural_s = if num_tests == 1 { "" } else { "s" };

                writeln!(self.out).unwrap();
                writeln!(self.out, "running {} test{}", num_tests, plural_s).unwrap();
            }
            FormatSetting::Json => writeln!(
                self.out,
                r#"{{ "type": "suite", "event": "started", "test_count": {} }}"#,
                num_tests
            )
            .unwrap(),
        }
    }

    pub(crate) fn print_test(&mut self, info: &TestInfo) {
        let TestInfo { module, name, .. } = info;
        match self.format {
            FormatSetting::Pretty => {
                write!(self.out, "test {module}::{name} ... ",).unwrap();
            }
            FormatSetting::Terse => {
                // In terse mode, nothing is printed before the job. Only
                // `print_single_outcome` prints one character.
            }
            FormatSetting::Json => {
                writeln!(
                    self.out,
                    r#"{{ "type": "test", "event": "started", "name": "{module}::{name}" }}"#,
                )
                .unwrap();
            }
        }
    }

    pub(crate) fn print_single_outcome(&mut self, info: &TestInfo, outcome: &Outcome) {
        let TestInfo { module, name, .. } = info;
        match self.format {
            FormatSetting::Pretty => {
                self.print_outcome_pretty(outcome);
                writeln!(self.out).unwrap();
            }
            FormatSetting::Terse => {
                let c = match outcome {
                    Outcome::Passed => '.',
                    Outcome::Failed { .. } => 'F',
                    Outcome::Ignored => 'i',
                };

                write!(self.out, "{}", c).unwrap();
            }
            FormatSetting::Json => {
                writeln!(
                    self.out,
                    r#"{{ "type": "test", "name": "{module}::{name}", "event": "{}" }}"#,
                    match outcome {
                        Outcome::Passed => "ok",
                        Outcome::Failed(_) => "failed",
                        Outcome::Ignored => "ignored",
                    }
                )
                .unwrap();
            }
        }
    }

    fn print_outcome_pretty(&mut self, outcome: &Outcome) {
        let s = match outcome {
            Outcome::Passed => "ok",
            Outcome::Failed { .. } => "FAILED",
            Outcome::Ignored => "ignored",
        };

        write!(self.out, "{}", s).unwrap();
    }

    pub(crate) fn print_list(&mut self, tests: &[Test], ignored: bool) {
        for test in tests {
            // libtest prints out:
            // * all tests without `--ignored`
            // * just the ignored tests with `--ignored`
            if ignored && !test.info.ignored {
                continue;
            }

            writeln!(self.out, "{}::{}: test", test.info.module, test.info.name,).unwrap();
        }
    }

    pub(crate) fn print_summary(&mut self, conclusion: &Conclusion) {
        match self.format {
            FormatSetting::Pretty | FormatSetting::Terse => {
                let outcome = if conclusion.has_failed() {
                    Outcome::Failed(Failed::default())
                } else {
                    Outcome::Passed
                };

                writeln!(self.out).unwrap();
                write!(self.out, "test result: ").unwrap();
                self.print_outcome_pretty(&outcome);
                writeln!(
                    self.out,
                    ". {} passed; {} failed; {} ignored; {} measured; \
                        {} filtered out",
                    conclusion.num_passed,
                    conclusion.num_failed,
                    conclusion.num_ignored,
                    conclusion.num_measured,
                    conclusion.num_filtered_out,
                )
                .unwrap();
                writeln!(self.out).unwrap();
            }
            FormatSetting::Json => {
                writeln!(
                    self.out,
                    concat!(
                        r#"{{ "type": "suite", "event": "{}", "passed": {}, "failed": {},"#,
                        r#" "ignored": {}, "measured": {}, "filtered_out": {} }}"#,
                    ),
                    if conclusion.num_failed > 0 {
                        "failed"
                    } else {
                        "ok"
                    },
                    conclusion.num_passed,
                    conclusion.num_failed,
                    conclusion.num_ignored,
                    conclusion.num_measured,
                    conclusion.num_filtered_out,
                )
                .unwrap();
            }
        }
    }
}
