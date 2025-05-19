// Copyright 2025 Jonas Kruckenberg
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

use crate::device_tree::DeviceTree;
use crate::mem::{Mmap, PhysicalAddress, with_kernel_aspace};
use crate::runtime::Runtime;
use crate::{arch, irq};
use alloc::string::{String, ToString};
use core::fmt;
use core::fmt::Write;
use core::range::Range;
use core::str::FromStr;
use fallible_iterator::FallibleIterator;
use spin::{Barrier, OnceLock};

static COMMANDS: &[Command] = &[PANIC, FAULT, VERSION, SHUTDOWN];

pub fn init(
    devtree: &'static DeviceTree,
    rt: &'static Runtime,
    num_cpus: usize,
) -> crate::Result<()> {
    static SYNC: OnceLock<Barrier> = OnceLock::new();
    let barrier = SYNC.get_or_init(|| Barrier::new(num_cpus));

    if barrier.wait().is_leader() {
        tracing::info!("{S}");
        tracing::info!("type `help` to list available commands");

        rt.try_spawn(async move {
            let (mut uart, _mmap, irq_num) = init_uart(devtree);

            let mut line = String::new();
            loop {
                let res = irq::next_event(irq_num).await;
                assert!(res.is_ok());
                let mut newline = false;

                let ch = uart.recv() as char;
                uart.write_char(ch).unwrap();
                match ch {
                    '\n' | '\r' => {
                        newline = true;
                        uart.write_str("\n\r").unwrap();
                    }
                    '\u{007F}' => {
                        line.pop();
                    }
                    ch => line.push(ch),
                }

                if newline {
                    eval(&line);
                    line.clear();
                }
            }
        })?;
    }

    Ok(())
}

fn init_uart(devtree: &DeviceTree) -> (uart_16550::SerialPort, Mmap, u32) {
    let s = devtree.find_by_path("/soc/serial").unwrap();
    assert!(s.is_compatible(["ns16550a"]));

    let clock_freq = s.property("clock-frequency").unwrap().as_u32().unwrap();
    let mut regs = s.regs().unwrap();
    let reg = regs.next().unwrap().unwrap();
    assert!(regs.next().unwrap().is_none());
    let irq_num = s.property("interrupts").unwrap().as_u32().unwrap();

    let mmap = with_kernel_aspace(|aspace| {
        // FIXME: this is gross, we're using the PhysicalAddress as an alignment utility :/
        let size = PhysicalAddress::new(reg.size.unwrap())
            .checked_align_up(arch::PAGE_SIZE)
            .unwrap()
            .get();

        let range_phys = {
            let start = PhysicalAddress::new(reg.starting_address);
            Range::from(start..start.checked_add(size).unwrap())
        };

        Mmap::new_phys(
            aspace.clone(),
            range_phys,
            size,
            arch::PAGE_SIZE,
            Some("UART-16550".to_string()),
        )
        .unwrap()
    });

    // Safety: info comes from device tree
    let uart = unsafe { uart_16550::SerialPort::new(mmap.range().start.get(), clock_freq, 115200) };

    (uart, mmap, irq_num)
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
        unsafe {
            #[expect(clippy::zero_ptr, reason = "we actually want to cause a fault here")]
            (0x0 as *const u8).read_volatile();
        }
        Ok(())
    });

const VERSION: Command = Command::new("version")
    .with_help("print verbose build and version info.")
    .with_fn(|_| {
        tracing::info!("k23 v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!(build.version = %concat!(
            env!("CARGO_PKG_VERSION"),
            "-",
            env!("VERGEN_GIT_BRANCH"),
            ".",
            env!("VERGEN_GIT_SHA")
        ));
        tracing::info!(build.timestamp = %env!("VERGEN_BUILD_TIMESTAMP"));
        tracing::info!(build.opt_level = %env!("VERGEN_CARGO_OPT_LEVEL"));
        tracing::info!(build.target = %env!("VERGEN_CARGO_TARGET_TRIPLE"));
        tracing::info!(commit.sha = %env!("VERGEN_GIT_SHA"));
        tracing::info!(commit.branch = %env!("VERGEN_GIT_BRANCH"));
        tracing::info!(commit.date = %env!("VERGEN_GIT_COMMIT_TIMESTAMP"));
        tracing::info!(rustc.version = %env!("VERGEN_RUSTC_SEMVER"));
        tracing::info!(rustc.channel = %env!("VERGEN_RUSTC_CHANNEL"));

        Ok(())
    });

const SHUTDOWN: Command = Command::new("shutdown")
    .with_help("exit the kernel and shutdown the machine.")
    .with_fn(|_| {
        tracing::info!("Bye, Bye!");

        todo!("scheduler().shutdown()");
        // scheduler().shutdown();

        // Ok(())
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
    InvalidArguments {
        help: &'a str,
        arg: &'a str,
        flag: Option<&'a str>,
    },
    FlagRequired {
        flags: &'a [&'a str],
    },
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
        fn invalid_command(_ctx: Context<'_>) -> CmdResult {
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

        fn fmt_flag_names(f: &mut fmt::Formatter<'_>, flags: &[&str]) -> fmt::Result {
            let mut names = flags.iter();
            if let Some(name) = names.next() {
                f.write_str(name)?;
                for name in names {
                    write!(f, "|{name}")?;
                }
            }
            Ok(())
        }

        let Self { line, kind } = self;
        match kind {
            ErrorKind::UnknownCommand(commands) => {
                write!(f, "unknown command {line:?}, expected one of: [")?;
                comma_delimited(&mut f, command_names(commands))?;
                f.write_char(']')?;
            }
            ErrorKind::InvalidArguments { help, arg, flag } => {
                f.write_str("invalid argument")?;
                if let Some(flag) = flag {
                    write!(f, " {flag}")?;
                }
                write!(f, " {arg:?}: {help}")?;
            }
            ErrorKind::FlagRequired { flags } => {
                write!(f, "the '{line}' command requires the ")?;
                fmt_flag_names(f, flags)?;
                write!(f, " flag")?;
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

    pub fn command(&self) -> &'cmd str {
        self.current.trim()
    }

    fn unknown_command(&self, commands: &'cmd [Command]) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::UnknownCommand(commands),
        }
    }

    pub fn invalid_argument(&self, help: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::InvalidArguments {
                arg: self.current,
                flag: None,
                help,
            },
        }
    }

    pub fn invalid_argument_named(&self, name: &'static str, help: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::InvalidArguments {
                arg: self.current,
                flag: Some(name),
                help,
            },
        }
    }

    pub fn other_error(&self, msg: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::Other(msg),
        }
    }

    pub fn parse_bool_flag(&mut self, flag: &str) -> bool {
        if let Some(rest) = self.command().trim().strip_prefix(flag) {
            self.current = rest.trim();
            true
        } else {
            false
        }
    }

    pub fn parse_optional_u32_hex_or_dec(
        &mut self,
        name: &'static str,
    ) -> Result<Option<u32>, Error<'cmd>> {
        let (chunk, rest) = match self.command().split_once(" ") {
            Some((chunk, rest)) => (chunk.trim(), rest),
            None => (self.command(), ""),
        };

        if chunk.is_empty() {
            return Ok(None);
        }

        let val = if let Some(hex_num) = chunk.strip_prefix("0x") {
            u32::from_str_radix(hex_num.trim(), 16).map_err(|_| Error {
                line: self.line,
                kind: ErrorKind::InvalidArguments {
                    arg: chunk,
                    flag: Some(name),
                    help: "expected a 32-bit hex number",
                },
            })?
        } else {
            u32::from_str(chunk).map_err(|_| Error {
                line: self.line,
                kind: ErrorKind::InvalidArguments {
                    arg: chunk,
                    flag: Some(name),
                    help: "expected a 32-bit decimal number",
                },
            })?
        };

        self.current = rest;
        Ok(Some(val))
    }

    pub fn parse_u32_hex_or_dec(&mut self, name: &'static str) -> Result<u32, Error<'cmd>> {
        self.parse_optional_u32_hex_or_dec(name).and_then(|val| {
            val.ok_or_else(|| self.invalid_argument_named(name, "expected a number"))
        })
    }

    pub fn parse_optional_flag<T>(
        &mut self,
        names: &'static [&'static str],
    ) -> Result<Option<T>, Error<'cmd>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        for name in names {
            if let Some(rest) = self.command().strip_prefix(name) {
                let (chunk, rest) = match rest.trim().split_once(" ") {
                    Some((chunk, rest)) => (chunk.trim(), rest),
                    None => (rest, ""),
                };

                if chunk.is_empty() {
                    return Err(Error {
                        line: self.line,
                        kind: ErrorKind::InvalidArguments {
                            arg: chunk,
                            flag: Some(name),
                            help: "expected a value",
                        },
                    });
                }

                match chunk.parse() {
                    Ok(val) => {
                        self.current = rest;
                        return Ok(Some(val));
                    }
                    Err(e) => {
                        tracing::warn!(target: "shell", "invalid value {chunk:?} for flag {name}: {e}");
                        return Err(Error {
                            line: self.line,
                            kind: ErrorKind::InvalidArguments {
                                arg: chunk,
                                flag: Some(name),
                                help: "invalid value",
                            },
                        });
                    }
                }
            }
        }

        Ok(None)
    }

    pub fn parse_required_flag<T>(
        &mut self,
        names: &'static [&'static str],
    ) -> Result<T, Error<'cmd>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        self.parse_optional_flag(names).and_then(|val| {
            val.ok_or(Error {
                line: self.line,
                kind: ErrorKind::FlagRequired { flags: names },
            })
        })
    }
}
