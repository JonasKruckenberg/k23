use crate::Options;
use crate::profile::{LogLevel, Profile, RustTarget};
use crate::tracing::{ColorMode, OutputOptions};
use color_eyre::eyre::{Context, bail};
use std::io::{IsTerminal, stderr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing_core::LevelFilter;

const DEFAULT_KERNEL_STACK_SIZE_PAGES: u32 = 256;

pub fn build_kernel(
    opts: &Options,
    output: &OutputOptions,
    profile: &Profile,
    include_tests: bool,
) -> crate::Result<PathBuf> {
    let cargo_verbosity = output.log.default_level().map_or(0, |lvl| match lvl {
        LevelFilter::TRACE => 2,
        LevelFilter::DEBUG => 1,
        _ => 0,
    });

    let mut build = BuildOptions::new(opts, output, &profile, BuildTarget::Kernel)
        .build_std(true)
        .verbose(cargo_verbosity)
        .release(opts.release)
        .include_tests(include_tests)
        .finish();

    let stacksize_pages = profile
        .kernel
        .stacksize_pages
        .unwrap_or(DEFAULT_KERNEL_STACK_SIZE_PAGES);

    let max_log_level = match profile
        .kernel
        .max_log_level
        .unwrap_or(profile.max_log_level)
    {
        LogLevel::Trace => "::tracing::Level::TRACE",
        LogLevel::Debug => "::tracing::Level::DEBUG",
        LogLevel::Info => "::tracing::Level::INFO",
        LogLevel::Warn => "::tracing::Level::WARN",
        LogLevel::Error => "::tracing::Level::ERROR",
    };

    build.cmd.env(
        "K23_CONSTANTS",
        format!(
            r#"
    pub const STACK_SIZE_PAGES: u32 = {stacksize_pages};
    pub const MAX_LOG_LEVEL: ::tracing::Level = {max_log_level};
    "#
        ),
    );

    build.run()?;
    Ok(build.output)
}

pub fn build_loader(
    opts: &Options,
    output: &OutputOptions,
    profile: &Profile,
    kernel: &Path,
    include_tests: bool,
) -> crate::Result<PathBuf> {
    let cargo_verbosity = output.log.default_level().map_or(0, |lvl| match lvl {
        LevelFilter::TRACE => 2,
        LevelFilter::DEBUG => 1,
        _ => 0,
    });

    let mut build = BuildOptions::new(opts, output, &profile, BuildTarget::Loader)
        .build_std(true)
        .verbose(cargo_verbosity)
        .release(opts.release)
        .include_tests(include_tests)
        .finish();

    let max_log_level = match profile
        .kernel
        .max_log_level
        .unwrap_or(profile.max_log_level)
    {
        LogLevel::Trace => "::log::Level::Trace",
        LogLevel::Debug => "::log::Level::Debug",
        LogLevel::Info => "::log::Level::Info",
        LogLevel::Warn => "::log::Level::Warn",
        LogLevel::Error => "::log::Level::Error",
    };

    build.cmd.env(
        "K23_CONSTANTS",
        format!(
            r#"
    pub const MAX_LOG_LEVEL: ::log::Level = {max_log_level};
    "#
        ),
    );
    build.cmd.env("KERNEL", kernel);

    build.run()?;
    Ok(build.output)
}

pub enum BuildTarget {
    Kernel,
    Loader,
}
pub struct BuildOptions<'a> {
    profile: &'a Profile,
    cargo_path: &'a Path,
    target_dir: Option<&'a Path>,
    verbosity: u8,
    release: bool,
    build_std: bool,
    include_tests: bool,
    no_default_features: bool,
    features: Vec<String>,
    chosen_target: RustTarget,
    kernel_target: RustTarget,
    loader_target: RustTarget,
    crate_name: &'static str,
    color_mode: ColorMode,
}

impl<'a> BuildOptions<'a> {
    pub fn new(
        opts: &'a Options,
        output: &OutputOptions,
        profile: &'a Profile,
        target: BuildTarget,
    ) -> Self {
        let kernel_target = profile.kernel.target.resolve(&profile);
        let loader_target = profile.loader.target.resolve(&profile);

        let (no_default_features, features, chosen_target, crate_name) = match target {
            BuildTarget::Kernel => (
                profile.kernel.no_default_features,
                profile.kernel.features.clone(),
                kernel_target.clone(),
                "kernel",
            ),
            BuildTarget::Loader => (
                profile.loader.no_default_features,
                profile.loader.features.clone(),
                loader_target.clone(),
                "loader",
            ),
        };

        Self {
            profile,
            cargo_path: &opts.cargo_path,
            target_dir: opts.target_dir.as_deref(),
            verbosity: 0,
            release: false,
            build_std: false,
            include_tests: false,
            no_default_features,
            features,
            crate_name,
            chosen_target,
            kernel_target,
            loader_target,
            color_mode: output.color,
        }
    }

    pub fn release(mut self, release: bool) -> Self {
        self.release = release;
        self
    }

    pub fn verbose(mut self, verbose: u8) -> Self {
        self.verbosity = verbose;
        self
    }

    pub fn build_std(mut self, build_std: bool) -> Self {
        self.build_std = build_std;
        self
    }

    pub fn include_tests(mut self, include_tests: bool) -> Self {
        self.include_tests = include_tests;
        self
    }

    pub fn finish(self) -> Build<'a> {
        let mut cmd = Command::new(&self.cargo_path);
        cmd.args([
            "build",
            "-p",
            self.crate_name,
            "--target",
            &self.chosen_target.to_string(),
        ]);

        cmd.env("CARGO_TERM_COLOR", self.color_mode.as_str());

        if self.release {
            cmd.arg("--release");
        }

        if self.include_tests {
            cmd.arg("--tests");
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

        cmd.env("K23_CONFIG_PATH", &self.profile.file_path);

        // We're capturing stderr (for diagnosis), so `cargo` won't automatically
        // turn on color.  If *we* are a TTY, then force it on.
        if stderr().is_terminal() {
            cmd.arg("--color");
            cmd.arg("always");
        }

        let target_dir = self
            .target_dir
            .clone()
            .unwrap_or(Path::new("target"))
            .canonicalize()
            .unwrap();

        let output = target_dir
            .join(self.chosen_target.name())
            .join("debug")
            .join(self.crate_name);

        if let Some(target_dir) = self.target_dir {
            cmd.env("CARGO_TARGET_DIR", target_dir);
        }

        Build {
            profile: self.profile,
            cmd,
            output,
            target_dir,
            kernel_target: self.kernel_target,
            loader_target: self.loader_target,
        }
    }
}

pub struct Build<'a> {
    profile: &'a Profile,
    cmd: Command,
    output: PathBuf,
    target_dir: PathBuf,
    kernel_target: RustTarget,
    loader_target: RustTarget,
}

impl Build<'_> {
    fn check_rebuild(&self) -> crate::Result<()> {
        let buildstamp_file = self.target_dir.join("buildstamp");

        let rebuild = match std::fs::read(&buildstamp_file) {
            Ok(contents) => {
                if let Ok(contents) = std::str::from_utf8(&contents) {
                    if let Ok(cmp) = u64::from_str_radix(contents, 16) {
                        self.profile.buildhash != cmp
                    } else {
                        println!("buildstamp file contents unknown; re-building.");
                        true
                    }
                } else {
                    println!("buildstamp file contents corrupt; re-building.");
                    true
                }
            }
            Err(_) => {
                println!("no buildstamp file found; re-building.");
                true
            }
        };
        // if we need to rebuild, we should clean everything before we start building
        if rebuild {
            println!("profile.toml has changed; rebuilding...");
            cargo_clean(&["kernel"], &self.kernel_target.to_string())?;
            cargo_clean(&["loader"], &self.loader_target.to_string())?;
        }

        // now that we're clean, update our buildstamp file; any failure to build
        // from here on need not trigger a clean
        std::fs::write(&buildstamp_file, format!("{:x}", self.profile.buildhash))?;

        Ok(())
    }

    fn run(&mut self) -> crate::Result<()> {
        self.check_rebuild()?;

        tracing::debug!("{:?}", self.cmd);

        let out = self
            .cmd
            .stderr(Stdio::inherit())
            .stdout(Stdio::inherit())
            .spawn()?
            .wait()?;

        if !out.success() {
            bail!("build failed");
        }

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
