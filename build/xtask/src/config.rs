use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

fn kernel_default_stack_size_pages() -> usize {
    16
}
fn bootloader_default_stack_size_pages() -> usize {
    4
}

#[derive(Clone, Debug, Serialize)]
pub struct Config {
    /// The name of the configuration, used for debugging purposes only
    pub name: String,
    /// The version of the configuration.
    pub version: Option<String>,
    /// The target triple for the configuration
    pub target: TargetTriple,
    /// The kernel configuration
    pub kernel: KernelConfig,
    /// The bootloader configuration
    pub bootloader: BootloaderConfig,
    /// The virtual memory mode to use
    pub memory_mode: MemoryMode,
    /// The baud rate for the kernel UART debugging output
    pub uart_baud_rate: u32,
    /// A hash of the configuration file that was used for this build
    pub buildhash: u64,
    /// The path to the configuration file that was used for this build
    pub config_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawConfig {
    name: String,
    version: Option<String>,
    target: String,
    kernel: KernelConfig,
    bootloader: BootloaderConfig,
    memory_mode: MemoryMode,
    uart_baud_rate: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct KernelConfig {
    /// The per-hart stack size in pages
    #[serde(default = "kernel_default_stack_size_pages")]
    pub stack_size_pages: usize,
    /// Rust features to enable
    #[serde(default)]
    pub features: Vec<String>,
    /// The verbosity level of logging output
    #[serde(default)]
    pub log_level: LogLevel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BootloaderConfig {
    /// The per-hart stack size in pages
    #[serde(default = "bootloader_default_stack_size_pages")]
    pub stack_size_pages: usize,
    /// Rust features to enable
    #[serde(default)]
    pub features: Vec<String>,
    /// The verbosity level of logging output
    #[serde(default)]
    pub log_level: LogLevel,
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

impl LogLevel {
    pub fn to_usize(&self) -> usize {
        match self {
            LogLevel::Error => 1,
            LogLevel::Warn => 2,
            LogLevel::Info => 3,
            LogLevel::Debug => 4,
            LogLevel::Trace => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum MemoryMode {
    Riscv64Sv39,
    Riscv64Sv48,
    Riscv64Sv57,
}

#[derive(Clone, Debug, Serialize)]
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

impl TargetTriple {
    pub fn from_str(target_triple: &str) -> anyhow::Result<Self> {
        let mut iter = target_triple.splitn(4, '-');

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

    pub fn to_string(&self) -> String {
        format!("{}-{}-{}-{}", self.arch, self.vendor, self.os, self.env)
    }
}

impl Config {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        Self::from_file_with_hasher(path, DefaultHasher::default())
    }

    fn from_file_with_hasher(path: &Path, mut hasher: DefaultHasher) -> anyhow::Result<Self> {
        let str = fs::read_to_string(path).context("failed to read configuration file")?;
        hasher.write(str.as_bytes());

        let raw: RawConfig = toml::from_str(&str).context("failed to parse configuration")?;

        Ok(Self {
            name: raw.name,
            version: raw.version,
            target: TargetTriple::from_str(&raw.target)?,
            memory_mode: raw.memory_mode,
            uart_baud_rate: raw.uart_baud_rate,
            kernel: raw.kernel,
            bootloader: raw.bootloader,
            buildhash: hasher.finish(),
            config_path: path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        let cfg = Config::from_file(Path::new("../../configs/riscv64-virt.toml")).unwrap();

        assert_eq!(cfg.name, "riscv64-virt");
        assert_eq!(cfg.version, Some("0.1.0".to_string()));
        assert_eq!(cfg.target.arch, "riscv64gc");
        assert_eq!(cfg.target.vendor, "unknown");
        assert_eq!(cfg.target.os, "none");
        assert_eq!(cfg.target.env, "elf");
        assert_eq!(cfg.buildhash, 4364868941823984348);
        assert_eq!(
            cfg.config_path,
            PathBuf::from("../../configs/riscv64-virt.toml")
        );
        assert_eq!(cfg.kernel.stack_size_pages, 4);
    }
}
