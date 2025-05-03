extern crate core;

mod build;
mod profile;
mod qemu;

use crate::profile::Profile;
use clap::{Parser, ValueHint};
use color_eyre::eyre::Result;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Parser)]
struct Options {
    #[clap(subcommand)]
    pub subcommand: SubCommand,

    #[clap(long, short)]
    pub verbose: bool,

    #[clap(long)]
    pub release: bool,

    /// Overrides the directory in which to build the output image.
    #[clap(short, long, env = "OUT_DIR", value_hint = ValueHint::DirPath, global = true)]
    pub out_dir: Option<PathBuf>,

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

#[derive(Debug, Parser)]
enum SubCommand {
    Build {
        /// The path to the build configuration file
        #[clap(value_hint = ValueHint::FilePath)]
        profile: PathBuf,
    },
    Dist {
        /// The path to the build configuration file
        #[clap(value_hint = ValueHint::FilePath)]
        profile: PathBuf,
    },
    /// Builds a bootable disk image and runs it in QEMU (implied `build`).
    Qemu {
        /// The path to the build configuration file
        #[clap(value_hint = ValueHint::FilePath)]
        profile: PathBuf,
        #[clap(flatten)]
        qemu: qemu::QemuOptions,
    },
    Lldb {
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
    },
}

fn main() -> Result<()> {
    let opts = Options::parse();

    match opts.subcommand {
        SubCommand::Build { ref profile } => {
            let profile = Profile::from_file(&profile)?;
            let kernel = build::build_kernel(&opts, &profile)?;
            let _image = build::build_loader(&opts, &profile, &kernel)?;
        }
        SubCommand::Dist { ref profile } => {
            let profile = Profile::from_file(&profile)?;
            let kernel = build::build_kernel(&opts, &profile)?;
            let _image = build::build_loader(&opts, &profile, &kernel)?;

            // TODO do something with it now

            todo!()
        }
        SubCommand::Qemu {
            ref profile,
            ref qemu,
        } => {
            let profile = Profile::from_file(&profile)?;
            let kernel = build::build_kernel(&opts, &profile)?;
            let image = build::build_loader(&opts, &profile, &kernel)?;
            qemu::run(qemu, profile, &image, true, false)?;
        }
        SubCommand::Lldb {
            ref profile,
            gdb_port,
            ref qemu_args,
            norun,
        } => {
            let profile = Profile::from_file(&profile)?;

            let target_dir = opts
                .target_dir
                .clone()
                .unwrap_or(PathBuf::from("target"))
                .canonicalize()?;

            let (kernel, loader) = if !norun {
                let qemu_opts = qemu::QemuOptions {
                    wait_for_debugger: true,
                    gdb_port,
                    qemu_args: qemu_args.clone(),
                };

                let kernel = build::build_kernel(&opts, &profile)?;
                let loader = build::build_loader(&opts, &profile, &kernel)?;

                qemu::run(&qemu_opts, profile, &loader, false, true)?;

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
                    kernel = kernel.display()
                ),
            )?;

            Command::new("rust-lldb")
                .args(["-s", lldb_script.to_str().unwrap()])
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()?;
        }
    }

    Ok(())
}
