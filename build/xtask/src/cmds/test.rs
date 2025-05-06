// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::build::{Cargo, CrateToBuild};
use crate::profile::{Architecture, Profile};
use crate::tracing::OutputOptions;
use crate::util::KillOnDrop;
use crate::{Options, qemu};
use clap::{Parser, ValueHint};
use color_eyre::Help;
use color_eyre::eyre::{Context, format_err};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Debug, Parser)]
pub struct Cmd {
    /// The path to the build configuration file
    #[clap(value_hint = ValueHint::FilePath)]
    profile: PathBuf,

    /// Timeout for failing test run, in seconds.
    ///
    /// If a test doesn't run to completion before this timeout elpased, it will be
    /// treated as failed.
    #[clap(long, value_parser = parse_secs, default_value = "1200")]
    timeout_secs: Duration,

    #[clap(flatten)]
    qemu_opts: qemu::QemuOptions,
}

impl Cmd {
    pub fn run(&self, opts: &Options, output: &OutputOptions) -> crate::Result<()> {
        let profile = Profile::from_file(&self.profile)?;

        let mut cargo = Cargo::test(CrateToBuild::Kernel, &profile, opts, output)?;
        cargo.build_std(true);
        let mut cmd = cargo.into_cmd();

        let (var, val) = cargo_qemu_runner_env(&profile)?;
        cmd.env(var, val);

        cmd.args(["--", "--"]);
        cmd.args(&self.qemu_opts.qemu_args);

        let mut child = KillOnDrop(cmd.spawn()?);

        match child
            .0
            .wait_timeout(self.timeout_secs)
            .context("waiting for QEMU to complete failed")?
        {
            None => child
                .0
                .kill()
                .map_err(Into::into)
                .and_then(|_| {
                    child
                        .0
                        .wait()
                        .context("waiting for QEMU process to complete failed")
                })
                .context("killing QEMU process failed")
                .and_then(|status: ExitStatus| {
                    Err(format_err!("test QEMU process exited with {}", status))
                })
                .with_context(|| format!("tests timed out after {:?}", self.timeout_secs))
                .note("maybe the kernel hung or boot looped?"),
            Some(status) => {
                if let Some(code) = status.code() {
                    if code == 0 {
                        Ok(())
                    } else {
                        Err(format_err!("QEMU exited with status code {}", code))
                    }
                } else {
                    Err(format_err!("QEMU exited without a status code, wtf?"))
                }
            }
        }
    }
}

fn parse_secs(s: &str) -> color_eyre::Result<Duration> {
    s.parse::<u64>()
        .map(Duration::from_secs)
        .context("not a valid number of seconds")
}

pub fn cargo_qemu_runner_env(profile: &Profile) -> crate::Result<(&'static str, String)> {
    // The produced target artifact cannot be run on the host, so we proactively set the
    // runner to the
    let runner_env_var = match profile.arch {
        Architecture::Riscv64 => "CARGO_TARGET_RISCV64GC_K23_NONE_KERNEL_RUNNER",
    };

    Ok((
        runner_env_var,
        format!(
            "cargo xtask __qemu {}",
            profile.file_path.canonicalize()?.display()
        ),
    ))
}
