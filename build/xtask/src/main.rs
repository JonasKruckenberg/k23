mod config;
mod logger;

use anyhow::anyhow;
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{Artifact, Message, MetadataCommand};
use clap::{ArgAction, Parser};
use config::Config;
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

            let (qemu_bin, cpu) = match cfg.target.arch.as_str() {
                "riscv64gc" => ("qemu-system-riscv64", "rv64"),
                _ => unimplemented!("Unsupported target architecture"),
            };

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
    let stack_size_pages = cfg.bootloader.stack_size_pages.to_string();
    let log_level = cfg.bootloader.log_level.to_usize().to_string();
    let uart_baud_rate = cfg.uart_baud_rate.to_string();
    let memory_mode = ron::ser::to_string(&cfg.memory_mode)?;

    let env_vars = vec![
        ("K23_KCONFIG_STACK_SIZE_PAGES", stack_size_pages.as_str()),
        ("K23_KCONFIG_LOG_LEVEL", log_level.as_str()),
        ("K23_KCONFIG_UART_BAUD_RATE", uart_baud_rate.as_str()),
        ("K23_KCONFIG_MEMORY_MODE", memory_mode.as_str()),
    ];

    let features: Vec<_> = cfg.bootloader.features.iter().map(|f| f.as_str()).collect();
    let kernel = build_crate(
        "bootloader",
        &cfg.target.to_string(),
        env_vars,
        &features,
        release,
    )?;
    let executable = kernel
        .executable
        .ok_or(anyhow!("failed to retrieve bootloader artifact"))?;

    Ok(executable)
}

fn build_kernel(cfg: &Config, release: bool) -> anyhow::Result<Utf8PathBuf> {
    let stack_size_pages = cfg.kernel.stack_size_pages.to_string();
    let log_level = cfg.kernel.log_level.to_usize().to_string();
    let uart_baud_rate = cfg.uart_baud_rate.to_string();
    let memory_mode = ron::ser::to_string(&cfg.memory_mode)?;

    let env_vars = vec![
        ("K23_KCONFIG_STACK_SIZE_PAGES", stack_size_pages.as_str()),
        ("K23_KCONFIG_LOG_LEVEL", log_level.as_str()),
        ("K23_KCONFIG_UART_BAUD_RATE", uart_baud_rate.as_str()),
        ("K23_KCONFIG_MEMORY_MODE", memory_mode.as_str()),
    ];

    let features: Vec<_> = cfg.kernel.features.iter().map(|f| f.as_str()).collect();
    let kernel = build_crate(
        "kernel",
        &cfg.target.to_string(),
        env_vars,
        &features,
        release,
    )?;
    let executable = kernel
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

fn build_crate(
    crate_name: &str,
    target: &str,
    env_vars: Vec<(&str, &str)>,
    features: &[&str],
    release: bool,
) -> anyhow::Result<Artifact> {
    let mut command = Command::new("cargo");
    command.args(&[
        "build",
        "--message-format=json-render-diagnostics",
        "-p",
        crate_name,
        "--target",
        target,
        "--features",
        &features.join(","),
    ]);
    command.envs(env_vars);

    if release {
        command.arg("--release");
    }

    let mut command = command.stdout(Stdio::piped()).spawn().unwrap();
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
                    log::info!("Successfully compiled `{crate_name}`");
                } else {
                    anyhow::bail!("could not compile `{crate_name}`");
                }
            }
            _ => (), // Unknown message
        }
    }

    command.wait().expect("Couldn't get cargo's exit status");

    artifact.ok_or(anyhow!("failed to retrieve artifact from command"))
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
