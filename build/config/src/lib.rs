use anyhow::{ensure, Context};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::Formatter;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fmt, fs};

fn bootloader_default_stack_size_pages() -> usize {
    4
}
fn kernel_default_stack_size_pages() -> usize {
    32
}
fn kernel_default_trap_stack_size_pages() -> usize {
    16
}
fn kernel_default_heap_size_pages() -> usize {
    8192
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The name of the configuration, used for debugging purposes only
    pub name: String,
    /// The version of the configuration.
    pub version: Option<String>,
    /// The kernel configuration
    pub kernel: KernelConfig,
    /// The bootloader configuration
    pub loader: LoaderConfig,
    /// The virtual memory mode to use
    pub memory_mode: MemoryMode,
    /// A hash of the configuration file that was used for this build
    pub buildhash: u64,
    /// The path to the configuration file that was used for this build
    pub config_path: PathBuf,
    /// The default Rust target to build for
    pub target: Target,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawConfig {
    name: String,
    version: Option<String>,
    kernel: KernelConfig,
    bootloader: LoaderConfig,
    memory_mode: MemoryMode,
    target: Target,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct KernelConfig {
    /// The per-hart stack size in pages
    #[serde(default = "kernel_default_stack_size_pages")]
    pub stack_size_pages: usize,
    /// The per-hart trap stack size in pages
    #[serde(default = "kernel_default_trap_stack_size_pages")]
    pub trap_stack_size_pages: usize,
    /// The heap size in pages
    #[serde(default = "kernel_default_heap_size_pages")]
    pub heap_size_pages: usize,
    /// Rust features to enable
    #[serde(default)]
    pub features: Vec<String>,
    /// The verbosity level of logging output
    #[serde(default)]
    pub log_level: LogLevel,
    /// The baud rate for the kernel UART debugging output
    pub uart_baud_rate: u32,
    /// Optionally overrides the default target
    pub target: Option<Target>,
    pub linker_script: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LoaderConfig {
    /// The per-hart stack size in pages
    #[serde(default = "bootloader_default_stack_size_pages")]
    pub stack_size_pages: usize,
    /// Rust features to enable
    #[serde(default)]
    pub features: Vec<String>,
    /// The verbosity level of logging output
    #[serde(default)]
    pub log_level: LogLevel,
    /// Optionally overrides the default target
    pub target: Option<Target>,
    pub linker_script: PathBuf,
}

/// The available verbosity levels of logging output
#[repr(usize)]
#[derive(Default, Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum LogLevel {
    /// Log only very serious errors.
    Error = 1,
    /// Log only on hazardous situations.
    Warn,
    /// Log general information. This is the default.
    #[default]
    Info,
    /// Log lower priority, debug information.
    Debug,
    /// Log everything, often extremely verbose, very low priority information.
    Trace,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum MemoryMode {
    Riscv64Sv39,
    Riscv64Sv48,
    Riscv64Sv57,
}

#[derive(Debug, Clone)]
pub enum Target {
    Triple(TargetTriple),
    Path(PathBuf),
}

impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(triple) = TargetTriple::from_str(s) {
            Ok(Target::Triple(triple))
        } else {
            Ok(Target::Path(PathBuf::from(s)))
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Target::Triple(triple) => write!(
                f,
                "{}-{}-{}-{}",
                triple.arch, triple.vendor, triple.os, triple.env
            ),
            Target::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TargetTriple {
    /// The architecture of the target
    pub arch: String,
    /// The vendor of the target
    pub vendor: String,
    /// The OS of the target
    pub os: String,
    /// The environment of the target
    pub env: String,
}

impl FromStr for TargetTriple {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut iter = s.splitn(4, '-');

        let arch = iter
            .next()
            .context("missing architecture in target triple")?;
        let vendor = iter.next().context("missing vendor in target triple")?;
        let os = iter.next().context("missing OS in target triple")?;
        let env = iter
            .next()
            .context("missing environment in target triple")?;

        ensure!(matches!(arch, "riscv64gc"), "unsupported architecture");
        ensure!(matches!(vendor, "unknown"), "unsupported vendor");
        ensure!(matches!(os, "none"), "unsupported OS");
        ensure!(matches!(env, "elf"), "unsupported environment");

        Ok(Self {
            arch: arch.to_string(),
            vendor: vendor.to_string(),
            os: os.to_string(),
            env: env.to_string(),
        })
    }
}

impl<'de> serde::Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Target;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("expected string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Target::from_str(v).map_err(|_| serde::de::Error::custom("failed to parse target"))
            }
        }

        let out = deserializer.deserialize_str(Visitor)?;

        Ok(out)
    }
}

impl Serialize for Target {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&self.to_string())
    }
}

impl Config {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        Self::from_file_with_hasher(path, DefaultHasher::default())
    }

    fn from_file_with_hasher(path: &Path, mut hasher: DefaultHasher) -> anyhow::Result<Self> {
        let str = fs::read_to_string(path).context("failed to read configuration file")?;
        hasher.write(str.as_bytes());

        let mut raw: RawConfig = toml::from_str(&str).context("failed to parse configuration")?;
        let dir = path.parent().unwrap();

        raw.bootloader.linker_script = dir.join(raw.bootloader.linker_script).canonicalize()?;
        raw.kernel.linker_script = dir.join(raw.kernel.linker_script).canonicalize()?;

        Ok(Self {
            name: raw.name,
            version: raw.version,
            memory_mode: raw.memory_mode,
            kernel: raw.kernel,
            loader: raw.bootloader,
            buildhash: hasher.finish(),
            config_path: path.to_path_buf().canonicalize()?,
            target: raw.target,
        })
    }
}
