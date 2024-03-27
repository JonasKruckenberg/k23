mod logger;

use anyhow::anyhow;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Artifact, Message, MetadataCommand};
use clap::{ArgAction, Parser};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

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
    Check {
        // /// Path to the image configuration file, in TOML.
        // cfg: PathBuf,
    },
    Gdb {
        // /// Path to the image configuration file, in TOML.
        // cfg: PathBuf,
        /// Don't start QEMU in the background
        #[clap(long, default_value = "true")]
        run: bool,
    },
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
        XtaskCommand::Check { .. } => {
            check_kernel()?;
            check_loader()?;
        }
        XtaskCommand::Gdb { run, .. } => {
            let kernel = build_kernel(false)?;
            let loader = build_loader(false, &kernel)?;

            let maybe_child = if run {
                let c = start_qemu("qemu-system-riscv64", loader.as_str(), true)?;
                Some(c)
            } else {
                None
            };

            Command::new("rust-lldb")
                .args([loader.as_str(), "-o", "gdb-remote localhost:1234"])
                .stdout(Stdio::inherit())
                .stdin(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()?;

            if let Some(mut c) = maybe_child {
                c.kill()?;
            }
        }
        XtaskCommand::Run { release, .. } => {
            let kernel = build_kernel(release)?;
            let loader = build_loader(release, &kernel)?;

            log::debug!("{loader}");
            log::debug!("{kernel}");

            let mut c = start_qemu("qemu-system-riscv64", loader.as_str(), true)?;
            c.wait()?;
        }
        XtaskCommand::Dist { .. } => {
            todo!()
        }
    }

    Ok(())
}

fn check_loader() -> anyhow::Result<()> {
    Cargo::new_check("loader", "riscv64imac-unknown-none-elf")
        .env("RUSTFLAGS", "-Zstack-protector=all -Csoft-float=true")
        .env("K23_KERNEL_ARTIFACT", "")
        .exec()?;

    Ok(())
}

fn check_kernel() -> anyhow::Result<()> {
    Cargo::new_check("kernel", "riscv64gc-unknown-none-elf")
        .env("RUSTFLAGS", "-Zstack-protector=all")
        .exec()?;

    Ok(())
}

fn build_loader(release: bool, kernel: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let bootloader = Cargo::new_build("loader", "riscv64gc-unknown-none-elf")
        .release(release)
        .env("RUSTFLAGS", "-Zstack-protector=all")
        .env("K23_KERNEL_ARTIFACT", kernel)
        .env(
            "K23_KERNEL_ARTIFACT_LEN",
            fs::metadata(kernel).unwrap().len().to_string(),
        )
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .exec()?;

    let executable = bootloader
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

fn build_kernel(release: bool) -> anyhow::Result<Utf8PathBuf> {
    let kernel = Cargo::new_build("kernel", "riscv64gc-unknown-none-elf")
        .release(release)
        .env("RUSTFLAGS", "-Zstack-protector=all")
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
        .exec()?;

    let executable = kernel
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

struct Cargo<'a> {
    crate_name: &'a str,
    release: bool,
    features: Vec<String>,
    command: Command,
}

impl<'a> Cargo<'a> {
    pub fn new_build(crate_name: &'a str, target: &str) -> Self {
        Self::new("build", crate_name, target)
    }

    pub fn new_check(crate_name: &'a str, target: &str) -> Self {
        Self::new("check", crate_name, target)
    }

    fn new(cmd: &'a str, crate_name: &'a str, target: &str) -> Self {
        let mut command = Command::new("cargo");
        command.args([
            cmd,
            "--message-format=json-render-diagnostics",
            "-p",
            crate_name,
            "--target",
            target,
        ]);

        Self {
            crate_name,
            release: false,
            features: vec![],
            command,
        }
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
        self.command.env(key, val);
        self
    }

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

    pub fn exec(mut self) -> anyhow::Result<Artifact> {
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
                        log::info!("Successfully compiled `{}`", self.crate_name);
                    } else {
                        anyhow::bail!("could not compile `{}`", self.crate_name);
                    }
                }
                _ => (), // Unknown message
            }
        }

        command.wait().expect("Couldn't get cargo's exit status");

        artifact.ok_or(anyhow!("failed to retrieve artifact from command"))
    }
}

fn start_qemu(runner: &str, kernel: &str, debug: bool) -> anyhow::Result<Child> {
    let mut cmd = Command::new(runner);
    cmd.args([
        "-bios",
        "default",
        "-kernel",
        kernel,
        "-machine",
        "virt",
        "-cpu",
        "rv64",
        "-d",
        "guest_errors,unimp,int",
        "-smp",
        "1",
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
    ]);

    if debug {
        cmd.args(["-s", "-S"]);
        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
    } else {
        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit());
    }

    Ok(cmd.spawn()?)
}
