mod logger;

use anyhow::anyhow;
use build_config::Config;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Artifact, Message, MetadataCommand};
use clap::{ArgAction, Parser};
use ed25519_dalek::Signer;
use std::ffi::OsStr;
use std::io::{IoSlice, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::{fs, process};

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
        /// Whether to build in release mode instead of debug mode
        #[clap(short, long)]
        release: bool,
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
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let xtask = Xtask::parse();

    logger::init(xtask.common.verbose);

    match xtask.cmd {
        XtaskCommand::Check { cfg, release } => {
            let cfg = Config::from_file(&cfg).unwrap();

            check_kernel(&cfg, release)?;
            check_loader(&cfg, release)?;
        }
        XtaskCommand::Run { release, cfg, .. } => {
            let cfg = Config::from_file(&cfg).unwrap();

            log::debug!("{cfg:?}");

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

            c.wait()?;
        }
        XtaskCommand::Dist { .. } => {
            todo!()
        }
    }

    Ok(())
}

fn create_kernel_image_files(kernel: &Utf8Path) -> anyhow::Result<(Utf8PathBuf, Utf8PathBuf)> {
    let signing_key = {
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;

        let mut csprng = OsRng;
        SigningKey::generate(&mut csprng)
    };

    let input = fs::read(kernel)?;

    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(&input);

    let kernel_file_path = kernel.with_extension("lz4");
    let mut kernel_file = fs::File::create(&kernel_file_path)?;
    kernel_file.write_vectored(&mut [IoSlice::new(&signature.to_bytes()), IoSlice::new(&input)])?;

    let verifying_key_path = kernel.parent().unwrap().join("verifying_key.pub");
    fs::write(&verifying_key_path, verifying_key.to_bytes())?;

    Ok((kernel_file_path, verifying_key_path))
}

fn check_loader(cfg: &Config, release: bool) -> anyhow::Result<()> {
    let target = cfg
        .loader
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    let cmd = Cargo::new_check("loader", &target, &cfg)?
        .release(release)
        // .env("RUSTFLAGS", "-Zstack-protector=all -Csoft-float=true")
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .enable_features(cfg.loader.features.clone());

    let cmd = if release {
        cmd.enable_features(["verify-image".to_string()])
    } else {
        cmd
    };

    cmd.exec()?;

    Ok(())
}

fn check_kernel(cfg: &Config, release: bool) -> anyhow::Result<()> {
    let target = cfg
        .kernel
        .target
        .as_ref()
        .unwrap_or(&cfg.target)
        .to_string();

    Cargo::new_check("kernel", &target, &cfg)?
        .release(release)
        // .env("RUSTFLAGS", "-Zstack-protector=all")
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
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

    let (kernel_image, kernel_verifying_key) = create_kernel_image_files(&kernel)?;

    log::debug!("compressed kernel image {kernel_image} public key {kernel_verifying_key}");

    let cmd = Cargo::new_build("loader", &target, &cfg)?
        .release(release)
        // .env("RUSTFLAGS", "-Zstack-protector=all")
        .env("K23_KERNEL_IMAGE", kernel_image)
        .env("K23_KERNEL_VERIFYING_KEY", kernel_verifying_key)
        .additional_args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .enable_features(cfg.loader.features.clone());

    let cmd = if release {
        cmd.enable_features(["verify-image".to_string()])
    } else {
        cmd
    };

    let bootloader = cmd.exec()?;

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
        // .env("RUSTFLAGS", "-Zstack-protector=all")
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
        let mut command = Command::new("cargo");
        command.args([
            cmd,
            "--message-format=json-render-diagnostics",
            "-p",
            crate_name,
            "--target",
            target,
        ]);
        command.env("K23_KCONFIG", cfg.config_path.as_os_str());

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

        if !self.features.is_empty() {
            self.command.args(["--features", &self.features.join(",")]);
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
        "guest_errors,int",
        "-smp",
        "1",
        "-m",
        "512M",
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
        "-nographic",
        "-serial",
        "mon:stdio",
        "-semihosting-config",
        "enable=on,userspace=on",
        "-monitor",
        "unix:qemu-monitor-socket,server,nowait",
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
