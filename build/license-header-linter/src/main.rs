// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// Adapted from RediSearch's license_header_linter:
// https://github.com/RediSearch/RediSearch/tree/master/src/redisearch_rs/tools/license_header_linter

/*
 * Copyright (c) 2006-2026, Redis Ltd.
 * All rights reserved.
 *
 * Licensed under your choice of the Redis Source Available License 2.0
 * (RSALv2); or (b) the Server Side Public License v1 (SSPLv1); or (c) the
 * GNU Affero General Public License v3 (AGPLv3).
*/

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{fs, io};

use clap::Parser;

const LICENSE_HEADER: &str = "\
// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
";

const EXCLUDED_DIR_NAMES: &[&str] = &["target", "buck-out", "result", "third-party"];
const EXCLUDED_PATHS: &[&str] = &["lib/range-tree", "lib/sharded-slab", "lib/wast"];

#[derive(Parser)]
struct Args {
    /// Prepend the header to files missing it instead of failing.
    #[arg(long)]
    fix: bool,

    /// Directory to scan.
    #[arg(default_value = ".")]
    root: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let mut bad = Vec::new();
    visit_dir(&args.root, &args.root, args.fix, &mut bad);

    if bad.is_empty() {
        return ExitCode::SUCCESS;
    }
    let mut stderr = io::stderr().lock();
    let _ = writeln!(
        stderr,
        "license header missing or malformed in {} file(s):",
        bad.len()
    );
    for path in &bad {
        let _ = writeln!(stderr, "  {}", path.display());
    }
    let _ = writeln!(
        stderr,
        "run `just fix-license-headers` to prepend the canonical header"
    );
    ExitCode::FAILURE
}

fn visit_dir(root: &Path, dir: &Path, fix: bool, bad: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("failed to read directory") {
        let path = entry.expect("failed to read directory entry").path();
        if path.is_dir() {
            if !is_excluded(root, &path) {
                visit_dir(root, &path, fix, bad);
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            check_file(&path, fix, bad);
        }
    }
}

fn is_excluded(root: &Path, dir: &Path) -> bool {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.starts_with('.') || EXCLUDED_DIR_NAMES.contains(&name) {
        return true;
    }
    dir.strip_prefix(root).is_ok_and(|rel| {
        EXCLUDED_PATHS
            .iter()
            .any(|excluded| rel == Path::new(excluded))
    })
}

fn check_file(path: &Path, fix: bool, bad: &mut Vec<PathBuf>) {
    let content = fs::read_to_string(path).unwrap_or_default();
    if content.starts_with(LICENSE_HEADER) {
        return;
    }
    if fix {
        fs::write(path, format!("{LICENSE_HEADER}\n{content}")).expect("failed to write file");
        let _ = writeln!(io::stderr().lock(), "fixed {}", path.display());
    } else {
        bad.push(path.to_path_buf());
    }
}
