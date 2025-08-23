// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Build tooling for k23
//!
//! # Prior art
//!
//! - [Inoculate] - Build tooling for the mycelium OS project
//! - [Hubris xtask] - Build tooling for the oxide computer hubris kernel
//! - [Redox OS configs] - RedoxOS uses toml-based build configurations as well
//!
//! [Inoculate]: https://github.com/hawkw/mycelium/tree/main/inoculate
//! [Hubris xtask]: https://github.com/oxidecomputer/hubris/tree/master/build/xtask
//! [Redox OS configs]: https://gitlab.redox-os.org/redox-os/redox/-/tree/master/config?ref_type=heads

#![feature(exit_status_error)]

mod build;
mod cmds;
mod profile;
mod qemu;
mod tracing;
mod util;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueHint};
use color_eyre::eyre::Result;
#[derive(Debug, Parser)]
struct Xtask {
    #[clap(subcommand)]
    pub subcommand: SubCommand,
    #[clap(flatten)]
    pub options: Options,
    #[clap(flatten)]
    pub output: tracing::OutputOptions,
}

#[derive(Debug, Parser)]
struct Options {
    /// Build kernel & loader in release mode, with optimizations enabled
    #[clap(long, global = true)]
    pub release: bool,

    /// Overrides the target directory for the kernel build.
    #[clap(
        short,
        long,
        env = "CARGO_TARGET_DIR",
        value_hint = ValueHint::DirPath,
        global = true
    )]
    pub target_dir: Option<PathBuf>,

    /// Overrides the path to the `cargo` executable.
    ///
    /// By default, this is read from the `CARGO` environment variable.
    #[clap(
        long = "cargo",
        env = "CARGO",
        default_value = "cargo",
        value_hint = ValueHint::ExecutablePath,
        global = true
    )]
    pub cargo_path: PathBuf,
}

#[derive(Debug, Subcommand)]
enum SubCommand {
    Build(cmds::build::Cmd),
    Dist(cmds::dist::Cmd),
    /// Builds a bootable disk image and runs it on target.
    ///
    /// Note that for now, the only supported target is QEMU.
    Run(cmds::run::Cmd),
    /// Builds a bootable disk image with tests and runs it on target, collecting the results.
    ///
    /// Note that for now, the only supported target is QEMU.
    Test(cmds::test::Cmd),
    Lldb(cmds::lldb::Cmd),
    #[clap(name = "__qemu", hide = true)]
    Qemu(cmds::qemu::Cmd),
}

fn main() -> Result<()> {
    let xtask = Xtask::parse();

    xtask.output.init_tracing_subscriber()?;

    match xtask.subcommand {
        SubCommand::Build(cmd) => cmd.run(&xtask.options, &xtask.output),
        SubCommand::Dist(cmd) => cmd.run(&xtask.options, &xtask.output),
        SubCommand::Run(cmd) => cmd.run(&xtask.options, &xtask.output),
        SubCommand::Test(cmd) => cmd.run(&xtask.options, &xtask.output),
        SubCommand::Lldb(cmd) => cmd.run(&xtask.options, &xtask.output),
        SubCommand::Qemu(cmd) => cmd.run(&xtask.options, &xtask.output),
    }
}
