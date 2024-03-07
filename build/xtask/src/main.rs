mod logger;

use anyhow::anyhow;
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{Artifact, Message, MetadataCommand};
use clap::{ArgAction, Parser};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Helper for passing VERSION to opt.
/// If `CARGO_VERSION_INFO` is set, use it, otherwise use `CARGO_PKG_VERSION`.
fn version() -> &'static str {
    option_env!("CARGO_VERSION_INFO").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Parser)]
#[clap(version = version())]
struct Xtask {
    #[clap(subcommand)]
    cmd: XtaskCommand,
    #[clap(flatten)]
    common: Common,
}

#[derive(Debug, Parser)]
struct Common {
    /// Enables verbose logging
    #[clap(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
}

#[derive(Debug, Parser)]
enum XtaskCommand {
    /// Builds and runs the image in QEMU
    Run {
        // /// Path to the image configuration file, in TOML.
        // cfg: PathBuf,
        /// Whether to build in release mode instead of debug mode
        #[clap(short, long)]
        release: bool,
    },
    /// Builds the image
    Dist {
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
        /// Whether to build in debug mode instead of release mode
        #[clap(short, long)]
        debug: bool,
    },
}

fn main() {
    if let Err(err) = run() {
        log::error!("{:?}", err);
    }
}

fn run() -> anyhow::Result<()> {
    let xtask = Xtask::parse();

    logger::init(xtask.common.verbose);

    match xtask.cmd {
        XtaskCommand::Run { release, .. } => {
            // let loader = build_loader(release)?;
            let kernel = build_kernel(release)?;

            // log::debug!("{loader}");
            log::debug!("{kernel}");

            Command::new("qemu-system-riscv64")
                .args([
                    "-bios",
                    "default",
                    "-kernel",
                    kernel.as_str(),
                    "-machine",
                    "virt",
                    "-cpu",
                    "rv64",
                    "-d",
                    "guest_errors,unimp",
                    "-smp",
                    "8",
                    "-m",
                    "128M",
                    "-nographic",
                    "-serial",
                    "mon:stdio",
                    "-device",
                    "virtio-rng-device",
                    "-device",
                    "virtio-gpu-device",
                    "-device",
                    "virtio-net-device",
                    "-device",
                    "virtio-tablet-device",
                    "-device",
                    "virtio-keyboard-device",
                ])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .stdin(Stdio::inherit())
                .output()
                .unwrap();
        }
        XtaskCommand::Dist { .. } => {
            todo!()
        }
    }

    Ok(())
}

// fn build_loader(release: bool) -> anyhow::Result<Utf8PathBuf> {
//     let bootloader = Builder::new("loader", "riscv64imac-unknown-none-elf")
//         .release(release)
//         .env("RUSTFLAGS", "-Csoft-float")
//         .additional_args([
//             "-Z",
//             "build-std=core,alloc",
//             "-Z",
//             "build-std-features=compiler-builtins-mem",
//         ])
//         .build()?;
//
//     let executable = bootloader
//         .executable
//         .ok_or(anyhow!("failed to retrieve kernel artifact"))?;
//
//     Ok(executable)
// }

fn build_kernel(release: bool) -> anyhow::Result<Utf8PathBuf> {
    let kernel = Builder::new("kernel", "riscv64gc-unknown-none-elf")
        .release(release)
        // .env(
        //     "RUSTFLAGS",
        //     "-Cforce-unwind-tables=true -Zstack-protector=strong",
        // )
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .build()?;

    let executable = kernel
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

struct Builder<'a> {
    name: &'a str,
    release: bool,
    features: Vec<String>,
    command: Command,
}

impl<'a> Builder<'a> {
    pub fn new(name: &'a str, target: &str) -> Self {
        let mut command = Command::new("cargo");
        command.args([
            "build",
            "--message-format=json-render-diagnostics",
            "-p",
            name,
            "--target",
            target,
        ]);

        Self {
            name,
            release: false,
            features: vec![],
            command,
        }
    }

    // pub fn env(mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
    //     self.command.env(key, val);
    //     self
    // }

    pub fn additional_args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
    {
        self.command.args(args);
        self
    }

    pub fn release(mut self, release: bool) -> Self {
        self.release = release;
        self
    }

    pub fn build(mut self) -> anyhow::Result<Artifact> {
        self.command.args(["--features", &self.features.join(",")]);
        if self.release {
            self.command.arg("--release");
        }

        log::debug!("command {:?}", self.command);

        let mut command = self.command.stdout(Stdio::piped()).spawn().unwrap();
        let metadata = MetadataCommand::new().exec().unwrap();

        let mut artifact: Option<Artifact> = None;
        let workspace_packages = metadata.workspace_packages();

        let reader = std::io::BufReader::new(command.stdout.take().unwrap());
        for message in Message::parse_stream(reader) {
            match message.unwrap() {
                Message::CompilerMessage(msg) => {
                    log::info!("{:?}", msg);
                }
                Message::CompilerArtifact(art) => {
                    artifact = Some(art);
                }
                Message::BuildScriptExecuted(script) => {
                    if let Ok(idx) = workspace_packages
                        .binary_search_by(|candidate| candidate.id.cmp(&script.package_id))
                    {
                        let package = workspace_packages[idx];
                        log::info!("Successfully compiled `{}` build script", package.name);
                    }
                }
                Message::BuildFinished(finished) => {
                    if finished.success {
                        log::info!("Successfully compiled `{}`", self.name);
                    } else {
                        anyhow::bail!("could not compile `{}`", self.name);
                    }
                }
                _ => (), // Unknown message
            }
        }
        log::debug!("here");

        command.wait().expect("Couldn't get cargo's exit status");

        artifact.ok_or(anyhow!("failed to retrieve artifact from command"))
    }
}
