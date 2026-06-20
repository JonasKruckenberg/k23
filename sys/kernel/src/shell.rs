// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Basic kernel shell for debugging purposes, taken from
//! <https://github.com/hawkw/mycelium/blob/main/src/shell.rs> (MIT)

const S: &str = r#"
   __    ___  ____
  / /__ |_  ||_  /
 /  '_// __/_/_ <
/_/\_\/____/____/
"#;

use alloc::string::String;
use core::fmt;
use core::fmt::Write;

use kasync::executor::Executor;
use loader_api::BootInfo;
use spin::{Barrier, OnceLock};
use uart_16550::Receiver;

use crate::irq;
use crate::state::global;

static COMMANDS: &[Command] = &[PANIC, FAULT, VERSION, SHUTDOWN];

pub fn init(boot_info: &BootInfo, rx: Receiver, sched: &'static Executor, num_cpus: usize) {
    // The `Barrier` below is here so that the maybe verbose startup logging is
    // out of the way before dropping the user into the kernel shell. If we don't
    // wait for the last CPU to have finished initializing it will mess up the shell output.
    static SYNC: OnceLock<Barrier> = OnceLock::new();
    let barrier = SYNC.get_or_init(|| Barrier::new(num_cpus));

    if barrier.wait().is_leader() {
        tracing::info!("{S}");
        tracing::info!("type `help` to list available commands");

        let irq_num = boot_info.uart.unwrap().irq_num;
        sched
            .try_spawn(async move {
                let mut line = String::new();
                loop {
                    let res = irq::next_event(irq_num).await;
                    assert!(res.is_ok());

                    let byte = rx.recv();
                    let ch = byte as char;

                    // Echo shares the console TX lock with the log writer so output
                    // can't interleave; the guard is dropped before the next await.
                    let tx = crate::tracing::console_tx();

                    let mut newline = false;
                    match ch {
                        // Emit a full CRLF on Enter rather than echoing the raw
                        // CR/LF, so the cursor returns to column 0 on terminals
                        // that don't translate newlines themselves (e.g. UTM).
                        '\n' | '\r' => {
                            newline = true;
                            tx.send(b'\r');
                            tx.send(b'\n');
                        }
                        // DEL: `Sender::send` expands this into the
                        // backspace-space-backspace erase sequence.
                        '\u{007F}' => {
                            tx.send(byte);
                            line.pop();
                        }
                        ch => {
                            tx.send(byte);
                            line.push(ch);
                        }
                    }

                    if newline {
                        eval(&line);
                        line.clear();
                    }
                }
            })
            .unwrap();
    }
}

pub fn eval(line: &str) {
    if line == "help" {
        tracing::info!(target: "shell", "available commands:");
        print_help("", COMMANDS);
        tracing::info!(target: "shell", "");
        return;
    }

    match handle_command(Context::new(line), COMMANDS) {
        Ok(_) => {}
        Err(error) => tracing::error!(target: "shell", "error: {error}"),
    }
}

const PANIC: Command = Command::new("panic")
    .with_usage("<MESSAGE>")
    .with_help("cause a kernel panic with the given message. use with caution.")
    .with_fn(|line| {
        panic!("{}", line.current);
    });

const FAULT: Command = Command::new("fault")
    .with_help("cause a CPU fault (null pointer dereference). use with caution.")
    .with_fn(|_| {
        // Safety: This actually *is* unsafe and *is* causing problematic behaviour, but that is exactly what
        // we want here!
        unsafe {
            core::ptr::dangling::<u8>().read_volatile();
        }
        Ok(())
    });

const VERSION: Command = Command::new("version")
    .with_help("print verbose build and version info.")
    .with_fn(|_| {
        tracing::info!("k23 v1.0.0"); // FIXME pipe through correct version!
        //tracing::info!("k23 v{}", env!("CARGO_PKG_VERSION"));

        // TODO reimplement this with vergen later
        // tracing::info!(build.version = %concat!(
        //     env!("CARGO_PKG_VERSION"),
        //     "-",
        //     env!("VERGEN_GIT_BRANCH"),
        //     ".",
        //     env!("VERGEN_GIT_SHA")
        // ));
        // tracing::info!(build.timestamp = %env!("VERGEN_BUILD_TIMESTAMP"));
        // tracing::info!(build.opt_level = %env!("VERGEN_CARGO_OPT_LEVEL"));
        // tracing::info!(build.target = %env!("VERGEN_CARGO_TARGET_TRIPLE"));
        // tracing::info!(commit.sha = %env!("VERGEN_GIT_SHA"));
        // tracing::info!(commit.branch = %env!("VERGEN_GIT_BRANCH"));
        // tracing::info!(commit.date = %env!("VERGEN_GIT_COMMIT_TIMESTAMP"));
        // tracing::info!(rustc.version = %env!("VERGEN_RUSTC_SEMVER"));
        // tracing::info!(rustc.channel = %env!("VERGEN_RUSTC_CHANNEL"));

        Ok(())
    });

const SHUTDOWN: Command = Command::new("shutdown")
    .with_help("exit the kernel and shutdown the machine.")
    .with_fn(|_| {
        tracing::info!("Bye, Bye!");

        global().executor.close();

        Ok(())
    });

#[derive(Debug)]
pub struct Command<'cmd> {
    name: &'cmd str,
    help: &'cmd str,
    usage: &'cmd str,
    run: fn(Context<'_>) -> CmdResult<'_>,
}

pub type CmdResult<'a> = Result<(), Error<'a>>;

#[derive(Debug)]
pub struct Error<'a> {
    line: &'a str,
    kind: ErrorKind<'a>,
}

#[derive(Debug)]
enum ErrorKind<'a> {
    UnknownCommand(&'a [Command<'a>]),
    Other(&'static str),
}

#[derive(Copy, Clone)]
pub struct Context<'cmd> {
    line: &'cmd str,
    current: &'cmd str,
}

fn print_help(parent_cmd: &str, commands: &[Command]) {
    let parent_cmd_pad = if parent_cmd.is_empty() { "" } else { " " };
    for command in commands {
        tracing::info!(target: "shell", "  {parent_cmd}{parent_cmd_pad}{command}");
    }
    tracing::info!(target: "shell", "  {parent_cmd}{parent_cmd_pad}help --- prints this help message");
}

fn handle_command<'cmd>(ctx: Context<'cmd>, commands: &'cmd [Command]) -> CmdResult<'cmd> {
    let chunk = ctx.current.trim();
    for cmd in commands {
        if let Some(current) = chunk.strip_prefix(cmd.name) {
            let current = current.trim();

            return panic_unwind::catch_unwind(|| cmd.run(Context { current, ..ctx })).unwrap_or({
                Err(Error {
                    line: cmd.name,
                    kind: ErrorKind::Other("command failed"),
                })
            });
        }
    }

    Err(ctx.unknown_command(commands))
}

// === impl Command ===

impl<'cmd> Command<'cmd> {
    #[must_use]
    pub const fn new(name: &'cmd str) -> Self {
        #[cold]
        fn invalid_command(_ctx: Context<'_>) -> CmdResult<'_> {
            panic!("command is missing run function, this is a bug");
        }

        Self {
            name,
            help: "",
            usage: "",
            run: invalid_command,
        }
    }

    #[must_use]
    pub const fn with_help(self, help: &'cmd str) -> Self {
        Self { help, ..self }
    }

    #[must_use]
    pub const fn with_usage(self, usage: &'cmd str) -> Self {
        Self { usage, ..self }
    }

    #[must_use]
    pub const fn with_fn(self, run: fn(Context<'_>) -> CmdResult<'_>) -> Self {
        Self { run, ..self }
    }

    pub fn run<'ctx>(&'cmd self, ctx: Context<'ctx>) -> CmdResult<'ctx>
    where
        'cmd: 'ctx,
    {
        let current = ctx.current.trim();

        if current == "help" {
            let name = ctx.line.strip_suffix(" help").unwrap_or("<???BUG???>");
            tracing::info!(target: "shell", "{name}");

            return Ok(());
        }

        (self.run)(ctx)
    }
}

impl fmt::Display for Command<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            run: _func,
            name,
            help,
            usage,
        } = self;

        write!(
            f,
            "{name}{usage_pad}{usage} --- {help}",
            usage_pad = if !usage.is_empty() { " " } else { "" },
        )
    }
}

// === impl Error ===

impl fmt::Display for Error<'_> {
    fn fmt(&self, mut f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn command_names<'cmd>(
            cmds: &'cmd [Command<'cmd>],
        ) -> impl Iterator<Item = &'cmd str> + 'cmd {
            cmds.iter()
                .map(|Command { name, .. }| *name)
                .chain(core::iter::once("help"))
        }

        let Self { line, kind } = self;
        match kind {
            ErrorKind::UnknownCommand(commands) => {
                write!(f, "unknown command {line:?}, expected one of: [")?;
                comma_delimited(&mut f, command_names(commands))?;
                f.write_char(']')?;
            }
            ErrorKind::Other(msg) => write!(f, "could not execute {line:?}: {msg}")?,
        }

        Ok(())
    }
}

impl core::error::Error for Error<'_> {}

fn comma_delimited<F: fmt::Display>(
    mut writer: impl Write,
    values: impl IntoIterator<Item = F>,
) -> fmt::Result {
    let mut values = values.into_iter();
    if let Some(value) = values.next() {
        write!(writer, "{value}")?;
        for value in values {
            write!(writer, ", {value}")?;
        }
    }

    Ok(())
}

// === impl Context ===

impl<'cmd> Context<'cmd> {
    pub const fn new(line: &'cmd str) -> Self {
        Self {
            line,
            current: line,
        }
    }

    fn unknown_command(&self, commands: &'cmd [Command]) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::UnknownCommand(commands),
        }
    }
}
