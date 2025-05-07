// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![allow(unused)]

use color_eyre::eyre::{Context, bail, eyre};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawProfile {
    arch: Architecture,
    name: String,
    #[serde(default)]
    version: u32,
    #[serde(default)]
    max_log_level: LogLevel,
    kernel: Kernel,
    loader: Loader,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub enum Architecture {
    #[serde(rename = "riscv64")]
    Riscv64,
}

#[repr(u8)]
#[derive(Clone, Copy, Default, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogLevel {
    Trace = 0,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Kernel {
    pub target: RawRustTarget,
    pub stacksize_pages: Option<u32>,
    pub max_log_level: Option<LogLevel>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub no_default_features: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Loader {
    pub target: RawRustTarget,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub no_default_features: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawRustTarget(String);

#[derive(Clone, Debug)]
pub enum RustTarget {
    Builtin(String),
    Json(PathBuf),
}

impl RawRustTarget {
    pub fn resolve(&self, profile: &Profile) -> RustTarget {
        if self.0.ends_with(".json") {
            RustTarget::Json(profile.resolve_path(&self.0).unwrap())
        } else {
            RustTarget::Builtin(self.0.clone())
        }
    }
}

impl RustTarget {
    pub fn name(&self) -> &str {
        match self {
            RustTarget::Builtin(name) => name.as_str(),
            RustTarget::Json(path) => path.file_stem().unwrap().to_str().unwrap(),
        }
    }
}

impl Display for RustTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RustTarget::Builtin(s) => write!(f, "{s}"),
            RustTarget::Json(p) => write!(f, "{}", p.display()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub arch: Architecture,
    pub name: String,
    pub version: u32,
    pub max_log_level: LogLevel,
    pub kernel: Kernel,
    pub loader: Loader,
    pub buildhash: u64,
    pub file_path: PathBuf,
}

impl Profile {
    pub fn from_file(file_path: &Path) -> crate::Result<Self> {
        let mut hasher = DefaultHasher::new();

        let doc = read_and_flatten_toml(file_path, &mut hasher, &mut BTreeSet::new())?;
        let profile_contents = doc.to_string();

        let toml: RawProfile = toml::from_str(&profile_contents)?;

        // if we had any other checks, perform them here

        Ok(Self {
            arch: toml.arch,
            name: toml.name,
            version: toml.version,
            max_log_level: toml.max_log_level,
            kernel: toml.kernel,
            loader: toml.loader,
            buildhash: hasher.finish(),
            file_path: file_path.to_path_buf(),
        })
    }

    pub fn resolve_path(&self, path: impl AsRef<Path>) -> crate::Result<PathBuf> {
        self.file_path
            .parent()
            .unwrap()
            .join(path)
            .canonicalize()
            .map_err(Into::into)
    }
}

fn read_and_flatten_toml(
    profile: &Path,
    hasher: &mut DefaultHasher,
    seen: &mut BTreeSet<PathBuf>,
) -> crate::Result<toml_edit::DocumentMut> {
    use toml_patch::merge_toml_documents;

    // Prevent diamond inheritance
    if !seen.insert(profile.to_owned()) {
        bail!(
            "{profile:?} is inherited more than once; \
             diamond dependencies are not allowed"
        );
    }
    let profile_contents =
        std::fs::read(profile).with_context(|| format!("could not read {}", profile.display()))?;

    // Accumulate the contents into the buildhash here, so that we hash both
    // the inheritance file and the target (recursively, if necessary)
    hasher.write(&profile_contents);

    let profile_contents =
        std::str::from_utf8(&profile_contents).context("failed to read manifest as UTF-8")?;

    // Additive TOML file inheritance
    let mut doc = profile_contents
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse TOML file")?;
    let Some(inherited_from) = doc.remove("inherit") else {
        // No further inheritance, so return the current document
        return Ok(doc);
    };

    use toml_edit::{Item, Value};
    let mut original = match inherited_from {
        // Single inheritance
        Item::Value(Value::String(s)) => {
            let file = profile.parent().unwrap().join(s.value());
            read_and_flatten_toml(&file, hasher, seen)
                .with_context(|| format!("Could not load {file:?}"))?
        }
        // Multiple inheritance, applied sequentially
        Item::Value(Value::Array(a)) => {
            let mut doc: Option<toml_edit::DocumentMut> = None;
            for a in a.iter() {
                if let Value::String(s) = a {
                    let file = profile.parent().unwrap().join(s.value());
                    let next: toml_edit::DocumentMut =
                        read_and_flatten_toml(&file, hasher, seen)
                            .with_context(|| format!("Could not load {file:?}"))?;
                    match doc.as_mut() {
                        Some(doc) => merge_toml_documents(doc, next)?,
                        None => doc = Some(next),
                    }
                } else {
                    bail!("could not inherit from {a}; bad type");
                }
            }
            doc.ok_or_else(|| eyre!("inherit array cannot be empty"))?
        }
        v => bail!("could not inherit from {v}; bad type"),
    };

    // Finally, apply any changes that are local in this file
    merge_toml_documents(&mut original, doc)?;
    Ok(original)
}
