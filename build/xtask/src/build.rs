// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::path::{Path, PathBuf};
use std::process::Command;

use color_eyre::eyre::{Context, bail};
use tracing_core::LevelFilter;

use crate::Options;
use crate::configuration::{Configuration, LogLevel, RustTarget};
use crate::tracing::{ColorMode, OutputOptions};
use crate::util::KillOnDrop;

const DEFAULT_KERNEL_STACK_SIZE_PAGES: u32 = 256;

pub fn build_kernel(
    opts: &Options,
    output: &OutputOptions,
    configuration: &Configuration,
) -> crate::Result<PathBuf> {
    let (mut cargo, output) = Cargo::build(CrateToBuild::Kernel, configuration, opts, output)?;
    cargo.build_std(true);
    let mut cmd = cargo.into_cmd();

    let stacksize_pages = configuration
        .kernel
        .stacksize_pages
        .unwrap_or(DEFAULT_KERNEL_STACK_SIZE_PAGES);

    let max_log_level = match configuration
        .kernel
        .max_log_level
        .unwrap_or(configuration.max_log_level)
    {
        LogLevel::Trace => "::tracing::Level::TRACE",
        LogLevel::Debug => "::tracing::Level::DEBUG",
        LogLevel::Info => "::tracing::Level::INFO",
        LogLevel::Warn => "::tracing::Level::WARN",
        LogLevel::Error => "::tracing::Level::ERROR",
    };

    cmd.env(
        "K23_CONSTANTS",
        format!(
            r#"
    pub const STACK_SIZE_PAGES: u32 = {stacksize_pages};
    pub const MAX_LOG_LEVEL: ::tracing::Level = {max_log_level};
    "#
        ),
    );

    tracing::debug!("{cmd:?}");

    let mut child = KillOnDrop(cmd.spawn()?);
    child.0.wait()?.exit_ok()?;

    Ok(output)
}

pub fn build_loader(
    opts: &Options,
    output: &OutputOptions,
    configuration: &Configuration,
    kernel: &Path,
) -> crate::Result<PathBuf> {
    let (mut cargo, output) = Cargo::build(CrateToBuild::Loader, configuration, opts, output)?;
    cargo.build_std(true);
    let mut cmd = cargo.into_cmd();

    let max_log_level = match configuration
        .kernel
        .max_log_level
        .unwrap_or(configuration.max_log_level)
    {
        LogLevel::Trace => "::log::Level::Trace",
        LogLevel::Debug => "::log::Level::Debug",
        LogLevel::Info => "::log::Level::Info",
        LogLevel::Warn => "::log::Level::Warn",
        LogLevel::Error => "::log::Level::Error",
    };

    cmd.env(
        "K23_CONSTANTS",
        format!(
            r#"
    pub const MAX_LOG_LEVEL: ::log::Level = {max_log_level};
    "#
        ),
    );
    cmd.env("KERNEL", kernel);

    tracing::debug!("{cmd:?}");
    let mut child = KillOnDrop(cmd.spawn()?);
    child.0.wait()?.exit_ok()?;

    Ok(output)
}

#[derive(Debug, Copy, Clone)]
pub enum CrateToBuild {
    Kernel,
    Loader,
}

impl CrateToBuild {
    fn as_str(&self) -> &'static str {
        match self {
            CrateToBuild::Kernel => "kernel",
            CrateToBuild::Loader => "loader",
        }
    }
}

pub struct Cargo<'a> {
    cmd: &'a str,
    cargo_path: &'a Path,
    target_dir: PathBuf,
    verbosity: u8,
    release: bool,
    build_std: bool,
    color_mode: ColorMode,
    configuration: &'a Configuration,
    no_default_features: bool,
    features: Vec<String>,
    rust_target: RustTarget,
    krate: CrateToBuild,
}

impl<'a> Cargo<'a> {
    fn new(
        cmd: &'a str,
        krate: CrateToBuild,
        opts: &'a Options,
        output: &OutputOptions,
        configuration: &'a Configuration,
    ) -> Self {
        let verbosity = output.log.default_level().map_or(0, |lvl| match lvl {
            LevelFilter::TRACE => 2,
            LevelFilter::DEBUG => 1,
            _ => 0,
        });

        let kernel_target = configuration.kernel.target.resolve(&configuration);
        let loader_target = configuration.loader.target.resolve(&configuration);

        let (no_default_features, features, rust_target) = match krate {
            CrateToBuild::Kernel => (
                configuration.kernel.no_default_features,
                configuration.kernel.features.clone(),
                kernel_target.clone(),
            ),
            CrateToBuild::Loader => (
                configuration.loader.no_default_features,
                configuration.loader.features.clone(),
                loader_target.clone(),
            ),
        };

        let target_dir = opts
            .target_dir
            .clone()
            .unwrap_or(PathBuf::from("target"))
            .canonicalize()
            .unwrap();

        Self {
            cmd,
            cargo_path: &opts.cargo_path,
            target_dir,
            verbosity,
            release: opts.release,
            build_std: false,
            color_mode: output.color,
            configuration,
            no_default_features,
            features,
            rust_target,
            krate,
        }
    }

    // pub fn check(
    //     krate: CrateToBuild,
    //     configuration: &'a Configuration,
    //     opts: &'a Options,
    //     output: &OutputOptions,
    // ) -> crate::Result<Self> {
    //     let this = Self::new("check", krate, opts, output, configuration);
    //
    //     this.maybe_clean()?;
    //
    //     Ok(this)
    // }
    //
    // pub fn clippy(
    //     krate: CrateToBuild,
    //     configuration: &'a Configuration,
    //     opts: &'a Options,
    //     output: &OutputOptions,
    // ) -> crate::Result<Self> {
    //     let this = Self::new("clippy", krate, opts, output, configuration);
    //
    //     this.maybe_clean()?;
    //
    //     Ok(this)
    // }

    pub fn build(
        krate: CrateToBuild,
        configuration: &'a Configuration,
        opts: &'a Options,
        output: &OutputOptions,
    ) -> crate::Result<(Self, PathBuf)> {
        let this = Self::new("build", krate, opts, output, configuration);

        this.maybe_clean()?;

        let output = this
            .target_dir
            .join(this.rust_target.name())
            .join(if opts.release { "release" } else { "debug" })
            .join(krate.as_str());

        Ok((this, output))
    }

    pub fn test(
        krate: CrateToBuild,
        configuration: &'a Configuration,
        opts: &'a Options,
        output: &OutputOptions,
    ) -> crate::Result<Self> {
        let this = Self::new("test", krate, opts, output, configuration);

        this.maybe_clean()?;

        Ok(this)
    }

    pub fn build_std(&mut self, build_std: bool) -> &mut Self {
        self.build_std = build_std;
        self
    }

    pub fn into_cmd(self) -> Command {
        let mut cmd = Command::new(&self.cargo_path);
        cmd.args([
            self.cmd,
            "-p",
            self.krate.as_str(),
            "--target",
            &self.rust_target.to_string(),
        ]);

        cmd.env("CARGO_TARGET_DIR", self.target_dir);
        cmd.env("CARGO_TERM_COLOR", self.color_mode.as_str());

        cmd.env("K23_CONFIG_PATH", &self.configuration.file_path);

        if self.release {
            cmd.arg("--release");
        }

        if self.no_default_features {
            cmd.arg("--no-default-features");
        }

        // pass on the number of `--verbose` flags we received
        if self.verbosity > 0 {
            cmd.arg(format!("-{}", str::repeat("v", self.verbosity as usize)));
        }

        if !self.features.is_empty() {
            cmd.arg("--features");
            cmd.arg(self.features.join(","));
        }

        if self.build_std {
            cmd.args([
                "-Z",
                "build-std=core,alloc",
                "-Z",
                "build-std-features=compiler-builtins-mem",
            ]);
        }

        cmd
    }

    fn maybe_clean(&self) -> crate::Result<()> {
        let buildstamp_file = self.target_dir.join("buildstamp");

        let rebuild = match std::fs::read(&buildstamp_file) {
            Ok(contents) => {
                if let Ok(contents) = std::str::from_utf8(&contents) {
                    if let Ok(cmp) = u64::from_str_radix(contents, 16) {
                        self.configuration.buildhash != cmp
                    } else {
                        tracing::warn!("buildstamp file contents unknown; re-building.");
                        true
                    }
                } else {
                    tracing::warn!("buildstamp file contents corrupt; re-building.");
                    true
                }
            }
            Err(_) => {
                tracing::debug!("no buildstamp file found; re-building.");
                true
            }
        };
        // if we need to rebuild, we should clean everything before we start building
        if rebuild {
            tracing::debug!("configuration.toml has changed; rebuilding...");

            let kernel_target = self
                .configuration
                .kernel
                .target
                .resolve(&self.configuration);
            cargo_clean(&["kernel"], &kernel_target.to_string())?;

            let loader_target = self
                .configuration
                .loader
                .target
                .resolve(&self.configuration);
            cargo_clean(&["loader"], &loader_target.to_string())?;
        }

        // now that we're clean, update our buildstamp file; any failure to build
        // from here on need not trigger a clean
        std::fs::write(
            &buildstamp_file,
            format!("{:x}", self.configuration.buildhash),
        )?;

        Ok(())
    }
}

fn cargo_clean(names: &[&str], target: &str) -> crate::Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("clean");
    println!("cleaning {:?}", names);
    for name in names {
        cmd.arg("-p").arg(name);
    }
    cmd.arg("--release").arg("--target").arg(target);

    let status = cmd
        .status()
        .context(format!("failed to cargo clean ({:?})", cmd))?;

    if !status.success() {
        bail!("command failed, see output for details");
    }

    Ok(())
}
