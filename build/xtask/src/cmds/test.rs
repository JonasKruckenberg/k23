// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::profile::Profile;
use crate::tracing::OutputOptions;
use crate::{Options, qemu};
use clap::{Parser, ValueHint};
use color_eyre::eyre::Context;
use std::path::PathBuf;
use std::time::Duration;

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
    timeout_secs: Option<Duration>,

    #[clap(flatten)]
    qemu_opts: qemu::QemuOptions,
}

fn parse_secs(s: &str) -> color_eyre::Result<Duration> {
    s.parse::<u64>()
        .map(Duration::from_secs)
        .context("not a valid number of seconds")
}

impl Cmd {
    pub fn run(&self, opts: &Options, output: &OutputOptions) -> crate::Result<()> {
        let profile = Profile::from_file(&self.profile)?;

        let kernel = crate::build::build_kernel(&opts, output, &profile, true)?;
        let image = crate::build::build_loader(&opts, output, &profile, &kernel, true)?;

        let mut child = qemu::spawn(&self.qemu_opts, profile, &image, true, &[])?;

        child.0.wait()?;

        todo!()
    }
}
