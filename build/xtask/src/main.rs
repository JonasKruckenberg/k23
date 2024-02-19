mod config;
mod logger;

use crate::config::Target;
use anyhow::anyhow;
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{Artifact, Message, MetadataCommand};
use clap::{ArgAction, Parser};
use config::Config;
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
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
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
        XtaskCommand::Run { cfg, release } => {
            let cfg = Config::from_file(&cfg).unwrap();

            let bootloader = build_bootloader(&cfg, release)?;
            let kernel = build_kernel(&cfg, release)?;

            let (qemu_bin, cpu) = if cfg.target.to_string().contains("riscv64gc") {
                ("qemu-system-riscv64", "rv64")
            } else {
                unimplemented!("Unsupported target architecture");
            };

            log::debug!("{bootloader}");
            log::debug!("{kernel}");

            Command::new(qemu_bin)
                .args(&[
                    "-bios",
                    "default",
                    "-kernel",
                    bootloader.as_str(),
                    // bootloader.executable.unwrap().as_str(),
                    // "-device",
                    // &format!("loader,addr=0x80400000,file={}", kernel.as_str()),
                    "-machine",
                    "virt",
                    "-cpu",
                    cpu,
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
                ])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .stdin(Stdio::inherit())
                .output()
                .unwrap();
        }
        XtaskCommand::Dist { cfg, debug } => {
            let cfg = Config::from_file(&cfg).unwrap();

            let kernel = build_kernel(&cfg, !debug)?;

            log::info!(action = "Packaging"; "Extracting elf artifact {kernel}");

            Command::new("riscv64-elf-objcopy")
                .args([kernel.as_str(), "-O", "binary", "kernel.bin"])
                .output()
                .unwrap();
        }
    }

    Ok(())
}

fn build_bootloader(cfg: &Config, release: bool) -> anyhow::Result<Utf8PathBuf> {
    // fall back to the root target
    let target = cfg.bootloader.target.clone().unwrap_or(cfg.target.clone());

    log::debug!("building for target {:?}", target);

    let bootloader = Builder::new("bootloader", target)
        .enable_features(cfg.bootloader.features.as_slice())
        .release(release)
        .env(
            "K23_KCONFIG_STACK_SIZE_PAGES",
            cfg.bootloader.stack_size_pages.to_string(),
        )
        .env(
            "K23_KCONFIG_LOG_LEVEL",
            cfg.bootloader.log_level.to_usize().to_string(),
        )
        .env("K23_KCONFIG_UART_BAUD_RATE", cfg.uart_baud_rate.to_string())
        .env(
            "K23_KCONFIG_MEMORY_MODE",
            ron::ser::to_string(&cfg.memory_mode)?,
        )
        .env("RUSTFLAGS", "-Csoft-float")
        .additional_args(&[
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .build()?;

    let executable = bootloader
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

fn build_kernel(cfg: &Config, release: bool) -> anyhow::Result<Utf8PathBuf> {
    // fall back to the root target
    let target = cfg.kernel.target.clone().unwrap_or(cfg.target.clone());

    let kernel = Builder::new("kernel", target)
        .enable_features(cfg.kernel.features.as_slice())
        .release(release)
        .env(
            "K23_KCONFIG_STACK_SIZE_PAGES",
            cfg.kernel.stack_size_pages.to_string(),
        )
        .env(
            "K23_KCONFIG_LOG_LEVEL",
            cfg.kernel.log_level.to_usize().to_string(),
        )
        .env("K23_KCONFIG_UART_BAUD_RATE", cfg.uart_baud_rate.to_string())
        .env(
            "K23_KCONFIG_MEMORY_MODE",
            ron::ser::to_string(&cfg.memory_mode)?,
        )
        .env("RUSTFLAGS", "-Cforce-unwind-tables=true")
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
    pub fn new(name: &'a str, target: Target) -> Self {
        let target = target.to_string();

        let mut command = Command::new("cargo");
        command.args(&[
            "build",
            "--message-format=json-render-diagnostics",
            "-p",
            name,
            "--target",
            &target,
        ]);

        Self {
            name,
            release: false,
            features: vec![],
            command,
        }
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Self {
        self.command.env(key, val);
        self
    }

    // pub fn envs<I, K, V>(mut self, vars: I) -> Self
    // where
    //     I: IntoIterator<Item = (K, V)>,
    //     K: AsRef<OsStr>,
    //     V: AsRef<OsStr>,
    // {
    //     self.command.envs(vars);
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

    pub fn enable_features<I, F>(mut self, features: I) -> Self
    where
        I: IntoIterator<Item = F>,
        F: AsRef<str>,
    {
        for f in features {
            self.features.push(f.as_ref().to_string());
        }
        self
    }

    pub fn release(mut self, release: bool) -> Self {
        self.release = release;
        self
    }

    pub fn build(mut self) -> anyhow::Result<Artifact> {
        self.command.args(&["--features", &self.features.join(",")]);
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

// fn parse_elf(executable: &Path) -> anyhow::Result<(u64, Vec<u8>, BTreeMap<String, u64>)> {
//     let data = fs::read(executable)?;
//     let elf: ElfFile<elf::FileHeader64<Endianness>> = ElfFile::parse(&*data).unwrap();
//
//     let symbols = elf.symbols();
//
//     let mut out = BTreeMap::new();
//     for symbol in symbols {
//         let name = symbol.name().unwrap();
//
//         out.insert(name.to_string(), symbol.address());
//     }
//
//     Ok((elf.entry(), elf.data().to_vec(), out))
// }
