mod logger;

use anyhow::anyhow;
use build_config::Config;
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
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
    },
    /// Builds and runs the image in QEMU
    Run {
        /// Whether to build in release mode instead of debug mode
        #[clap(short, long)]
        release: bool,
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
    },
    /// Builds the image
    Dist {
        /// Whether to build in debug mode instead of release mode
        #[clap(short, long)]
        debug: bool,
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
    },
    /// Builds and runs the image in QEMU
    Dbg {
        /// Whether to build in release mode instead of debug mode
        #[clap(short, long)]
        release: bool,
        /// Path to the image configuration file, in TOML.
        cfg: PathBuf,
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
        XtaskCommand::Check { cfg } => {
            let cfg = Config::from_file(&cfg).unwrap();

            check_kernel(&cfg)?;
            check_loader(&cfg)?;
        }
        XtaskCommand::Run { release, cfg, .. } => {
            let cfg = Config::from_file(&cfg).unwrap();

            let kernel = build_kernel(&cfg, release)?;
            let loader = build_loader(&cfg, release, &kernel)?;

            log::debug!("{loader}");
            log::debug!("{kernel}");

            let mut c = start_qemu("qemu-system-riscv64", loader.as_str(), false)?;
            c.wait()?;
        }
        XtaskCommand::Dbg { release, cfg, .. } => {
            let cfg = Config::from_file(&cfg).unwrap();

            let kernel = build_kernel(&cfg, release)?;
            let loader = build_loader(&cfg, release, &kernel)?;

            log::debug!("{loader}");
            log::debug!("{kernel}");

            let mut c = start_qemu("qemu-system-riscv64", loader.as_str(), true)?;

            // Command::new("rust-lldb")
            //     .arg(loader)
            //     .stdin(Stdio::inherit())
            //     .stdout(Stdio::inherit())
            //     .stderr(Stdio::inherit())
            //     .output()?;

            c.wait()?;
        }
        XtaskCommand::Dist { .. } => {
            todo!()
        }
    }

    Ok(())
}

fn compress_kernel(kernel: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let out_path = kernel.with_extension("lz4");

    let input = fs::read(kernel)?;
    let output = lz4_flex::block::compress_prepend_size(&input);
    fs::write(&out_path, output)?;

    Ok(out_path)
}

fn check_loader(cfg: &Config) -> anyhow::Result<()> {
    let target = cfg
        .loader
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    Cargo::new_check("loader", &target, &cfg)?
        .env("RUSTFLAGS", "-Zstack-protector=all -Csoft-float=true")
        .env("K23_KERNEL_ARTIFACT", "main.rs") // use main.rs as a fake file during checking
        .enable_features(cfg.loader.features.clone())
        .exec()?;

    Ok(())
}

fn check_kernel(cfg: &Config) -> anyhow::Result<()> {
    let target = cfg
        .kernel
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    Cargo::new_check("kernel", &target, &cfg)?
        .env("RUSTFLAGS", "-Zstack-protector=all")
        .enable_features(cfg.kernel.features.clone())
        .exec()?;

    Ok(())
}

fn build_loader(cfg: &Config, release: bool, kernel: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
    let target = cfg
        .loader
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    let compressed_kernel = compress_kernel(&kernel)?;

    log::debug!("compressed kernel artifact {compressed_kernel}");

    let bootloader = Cargo::new_build("loader", &target, &cfg)?
        .release(release)
        .env("RUSTFLAGS", "-Zstack-protector=all")
        .env("K23_KERNEL_ARTIFACT", compressed_kernel)
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .enable_features(cfg.loader.features.clone())
        .exec()?;

    let executable = bootloader
        .executable
        .ok_or(anyhow!("failed to retrieve kernel artifact"))?;

    Ok(executable)
}

fn build_kernel(cfg: &Config, release: bool) -> anyhow::Result<Utf8PathBuf> {
    let target = cfg
        .kernel
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    let kernel = Cargo::new_build("kernel", &target, &cfg)?
        .release(release)
        .env("RUSTFLAGS", "-Zstack-protector=all")
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .enable_features(cfg.kernel.features.clone())
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
    pub fn new_build(crate_name: &'a str, target: &str, cfg: &Config) -> anyhow::Result<Self> {
        Self::new("build", crate_name, target, cfg)
    }

    pub fn new_check(crate_name: &'a str, target: &str, cfg: &Config) -> anyhow::Result<Self> {
        Self::new("check", crate_name, target, cfg)
    }

    fn new(cmd: &'a str, crate_name: &'a str, target: &str, cfg: &Config) -> anyhow::Result<Self> {
        // let cfg_ron = ron::to_string(&cfg)?;

        let cfg_ron = ron::ser::to_string_pretty(&cfg, ron::ser::PrettyConfig::new())?;

        let mut command = Command::new("cargo");
        command.args([
            cmd,
            "--message-format=json-render-diagnostics",
            "-p",
            crate_name,
            "--target",
            target,
        ]);
        command.env("K23_KCONFIG", cfg_ron);

        Ok(Self {
            crate_name,
            release: false,
            features: vec![],
            command,
        })
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

    pub fn enable_features<I>(mut self, features: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        self.features.extend(features);
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

        self.command.args(["--features", &self.features.join(",")]);

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
        "-display",
        "none",
        // "-nographic",
        // "-serial",
        // "mon:stdio",
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
        "-chardev",
        "stdio,id=stdio0",
        "-semihosting-config",
        "enable=on,userspace=on,chardev=stdio0",
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
