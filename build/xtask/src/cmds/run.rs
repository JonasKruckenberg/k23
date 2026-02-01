// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::path::PathBuf;

use clap::{Parser, ValueHint};

use crate::configuration::Configuration;
use crate::tracing::OutputOptions;
use crate::{Options, qemu};

#[derive(Debug, Parser)]
pub struct Cmd {
    /// The path to the build configuration file
    #[clap(value_hint = ValueHint::FilePath)]
    configuration: PathBuf,
    #[clap(flatten)]
    qemu_opts: qemu::QemuOptions,
}

impl Cmd {
    pub fn run(&self, opts: &Options, output: &OutputOptions) -> crate::Result<()> {
        let configuration = Configuration::from_file(&self.configuration)?;

        let kernel = crate::build::build_kernel(opts, output, &configuration)?;
        let image = crate::build::build_loader(opts, output, &configuration, &kernel)?;

        let mut child = qemu::spawn(&self.qemu_opts, configuration, &image, true, &[])?;

        child.0.wait()?;

        Ok(())
    }
}
