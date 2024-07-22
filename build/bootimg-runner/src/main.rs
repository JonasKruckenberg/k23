use clap::{ArgAction, Parser, ValueHint};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use object::{Architecture, Object};
use std::fs::File;
use std::io::{IoSlice, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::{env, fs};

#[derive(Debug, clap::Parser)]
struct Options {
    #[clap(value_hint = ValueHint::FilePath)]
    pub payload: PathBuf,
    // /// Path to the image configuration file, in TOML.
    // #[clap(value_hint = ValueHint::FilePath)]
    // pub buildcfg_path: PathBuf,
    #[clap(
        long = "cargo",
        env = "CARGO",
        default_value = "cargo",
        value_hint = ValueHint::ExecutablePath,
        global = true
    )]
    pub cargo_path: PathBuf,
    #[clap(
        long = "qemu",
        env = "QEMU",
        value_hint = ValueHint::ExecutablePath,
        global = true
    )]
    pub qemu_path: Option<PathBuf>,
    /// Enables verbose logging
    #[clap(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
    /// Whether to build in release mode instead of debug mode
    #[clap(short, long)]
    release: bool,
    #[clap(short, long, alias = "debug", alias = "dbg", alias = "gdb")]
    wait_for_debugger: bool,
}

fn main() {
    let opts = Options::parse();
    println!("{opts:?}");

    let payload = fs::read(&opts.payload).unwrap();
    let obj =
        object::File::parse(payload.as_slice()).expect("failed to parse compilation artifact");
    let target = Target::from_elf(&obj);

    let builder = Builder::new_from_elf(opts.cargo_path, target, opts.release);

    let (verifying_key, signing_key) = generate_keypair();
    let payload = builder.compress_and_sign(&payload, signing_key);

    let loader = builder.build_loader(verifying_key, &payload);

    println!("{loader:?} {payload:?}");

    run_in_qemu(
        opts.qemu_path.as_deref(),
        target,
        &loader,
        opts.wait_for_debugger,
    )
    .unwrap();
}

fn generate_keypair() -> (VerifyingKey, SigningKey) {
    let signing_key = {
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;

        let mut csprng = OsRng;
        SigningKey::generate(&mut csprng)
    };

    (signing_key.verifying_key(), signing_key)
}

#[derive(Copy, Clone)]
pub enum Target {
    Riscv64,
    Riscv32,
}

impl Target {
    pub fn from_elf(elf_file: &object::File) -> Target {
        match elf_file.architecture() {
            Architecture::Riscv32 => Self::Riscv64,
            Architecture::Riscv64 => Self::Riscv64,
            arch => panic!("unsupported architecture {arch:?}"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Riscv64 => "riscv64gc-unknown-none-elf",
            Target::Riscv32 => "riscv32imac-unknown-none-elf",
        }
    }
}

pub struct Builder {
    cargo: PathBuf,
    target: Target,
    out_dir: PathBuf,
    release: bool,
}

impl Builder {
    pub fn new_from_elf(cargo: PathBuf, target: Target, release: bool) -> Self {
        let workspace_dir = {
            let qemu_run_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
            qemu_run_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
        };

        let out_dir = workspace_dir
            .join("target")
            .join(target.as_str())
            .join(if release { "release" } else { "debug" });

        Self {
            target,
            out_dir,
            cargo,
            release,
        }
    }

    pub fn build_loader(&self, verifying_key: VerifyingKey, payload_path: &Path) -> PathBuf {
        let verifying_key_path = self.out_dir.join("verifying_key.bin");
        fs::write(&verifying_key_path, verifying_key.as_bytes()).unwrap();

        let mut cmd = Command::new(&self.cargo);
        cmd.args([
            "build",
            "-p",
            "loader",
            "--target",
            self.target.as_str(),
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .envs([
            ("K23_VERIFYING_KEY_PATH", verifying_key_path.as_path()),
            ("K23_PAYLOAD_PATH", payload_path),
        ]);

        if self.release {
            cmd.arg("--release");
        }

        let out = cmd.output().unwrap();

        assert!(
            out.status.success(),
            "failed to build loader {}",
            core::str::from_utf8(&out.stderr).unwrap()
        );

        self.out_dir.join("loader")
    }

    pub fn compress_and_sign(&self, input: &[u8], signing_key: SigningKey) -> PathBuf {
        let compressed = lz4_flex::compress_prepend_size(&input);

        let signature = signing_key.sign(&compressed);

        let out_path = self.out_dir.join("payload.bin");
        let mut file = File::create(&out_path).unwrap();

        file.write_vectored(&[
            IoSlice::new(&signature.to_bytes()),
            IoSlice::new(&compressed),
        ])
        .unwrap();

        out_path
    }
}

fn run_in_qemu(
    qemu_path: Option<&Path>,
    target: Target,
    bootimg_path: &Path,
    wait_for_debugger: bool,
) -> Option<i32> {
    let runner = qemu_path.map_or_else(
        || match target {
            Target::Riscv64 => "qemu-system-riscv64",
            Target::Riscv32 => "qemu-system-riscv32",
        },
        |path| path.to_str().unwrap(),
    );

    let mut child = KillOnDrop({
        let mut cmd = Command::new(runner);
        cmd.args([
            "-kernel",
            bootimg_path.to_str().unwrap(),
            "-machine",
            "virt",
            "-cpu",
            "rv64",
            "-smp",
            "1",
            "-m",
            "512M",
            "-d",
            "guest_errors,int",
            "-nographic",
            "-monitor",
            "none",
            "-semihosting-config",
            "enable=on,target=native",
            "-monitor",
            "unix:qemu-monitor-socket,server,nowait",
        ]);

        if wait_for_debugger {
            cmd.args(["-s", "-S"]);
        }

        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Error running qemu-system-riscv64; perhaps you haven't installed it yet?")
    });

    child.0.wait().unwrap().code()
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}
