// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{fs, thread};

use clap::{Parser, ValueHint};

use crate::profile::Profile;
use crate::tracing::OutputOptions;
use crate::{Options, build, qemu};

#[derive(Debug, Parser)]
pub struct Cmd {
    /// The path to the build configuration file
    #[clap(value_hint = ValueHint::FilePath)]
    profile: PathBuf,
    /// The TCP port to listen for debug connections on.
    #[clap(long, default_value = "1234")]
    gdb_port: u16,
    /// Extra arguments passed to QEMU.
    #[clap(raw = true, conflicts_with = "norun")]
    qemu_args: Vec<String>,
    /// Do not start a new k23 instance; just run `lldb`
    #[clap(long, short, conflicts_with = "gdb_port", conflicts_with = "qemu_args")]
    norun: bool,
}

impl Cmd {
    pub fn run(&self, opts: &Options, output: &OutputOptions) -> crate::Result<()> {
        let profile = Profile::from_file(&self.profile)?;

        let target_dir = opts
            .target_dir
            .clone()
            .unwrap_or(PathBuf::from("target"))
            .canonicalize()?;

        let (kernel, loader) = if !self.norun {
            let qemu_opts = qemu::QemuOptions {
                wait_for_debugger: true,
                gdb_port: self.gdb_port,
                qemu_args: self.qemu_args.clone(),
            };

            let kernel = build::build_kernel(opts, output, &profile)?;
            let loader = build::build_loader(opts, output, &profile, &kernel)?;

            let mut qemu = qemu::spawn(&qemu_opts, profile, &loader, false, &[])?;
            thread::spawn(move || qemu.0.wait().unwrap().exit_ok().unwrap());

            (kernel, loader)
        } else {
            let kernel = target_dir
                .join(profile.kernel.target.resolve(&profile).name())
                .join("debug")
                .join("kernel");

            let loader = target_dir
                .join(profile.loader.target.resolve(&profile).name())
                .join("debug")
                .join("loader");

            (kernel, loader)
        };

        let lldb_script = target_dir.join("lldb_script.txt");
        fs::write(
            &lldb_script,
            format!(
                r#"
        target create {loader}
        target modules add {kernel}
        target modules load --file {kernel} -s 0xffffffc000000000
        gdb-remote localhost:{gdb_port}
                    "#,
                loader = loader.display(),
                kernel = kernel.display(),
                gdb_port = self.gdb_port,
            ),
        )?;

        Command::new("rust-lldb")
            .args(["-s", lldb_script.to_str().unwrap()])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()?;

        Ok(())
    }
}
