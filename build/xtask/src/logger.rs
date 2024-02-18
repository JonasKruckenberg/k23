use anstyle::{AnsiColor, Color, Style};
use log::{log_enabled, Level};
use std::io::Write;

pub fn init(verbosity: u8) {
    let mut builder = env_logger::Builder::from_default_env();

    builder
        .format_indent(Some(12))
        .filter(None, verbosity_level(verbosity).to_level_filter())
        .format(|f, record| {
            let style = f.default_level_style(record.level()).bold();

            if let Some(action) = record.key_values().get("action".into()) {
                let style = style.fg_color(Some(Color::Ansi(AnsiColor::Green)));

                write!(f, "{style}{:>12}{style:#} ", action)?;
            } else {
                write!(
                    f,
                    "{style}{:>12}{style:#} ",
                    prettyprint_level(record.level())
                )?;
            }

            if log_enabled!(Level::Debug) {
                let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlack)));

                write!(f, "{style}[{}]{style:#} ", record.target())?;
            }

            writeln!(f, "{}", record.args())
        })
        .init();
}

/// This maps the occurrence of `--verbose` flags to the correct log level
fn verbosity_level(num: u8) -> Level {
    match num {
        0 => Level::Info,
        1 => Level::Debug,
        2.. => Level::Trace,
    }
}

/// The default string representation for `Level` is all uppercaps which doesn't mix well with the other printed actions.
fn prettyprint_level(lvl: Level) -> &'static str {
    match lvl {
        Level::Error => "Error",
        Level::Warn => "Warn",
        Level::Info => "Info",
        Level::Debug => "Debug",
        Level::Trace => "Trace",
    }
}
