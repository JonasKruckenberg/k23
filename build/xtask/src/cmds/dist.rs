// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::Options;
use crate::profile::Profile;
use crate::tracing::OutputOptions;
use clap::{Parser, ValueHint};
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct Cmd {
    /// The path to the build configuration file
    #[clap(value_hint = ValueHint::FilePath)]
    profile: PathBuf,

    /// Overrides the directory in which to build the output image.
    #[clap(short, long, env = "OUT_DIR", value_hint = ValueHint::DirPath, global = true)]
    out_dir: Option<PathBuf>,
}

impl Cmd {
    pub fn run(&self, opts: &Options, output: &OutputOptions) -> crate::Result<()> {
        let profile = Profile::from_file(&self.profile)?;

        let kernel = crate::build::build_kernel(&opts, output, &profile)?;
        let _image = crate::build::build_loader(&opts, output, &profile, &kernel)?;

        Ok(())
    }
}
