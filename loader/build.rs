// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let workspace_root = env::var_os("__K23_CARGO_RUSTC_CURRENT_DIR").map(PathBuf::from);

    println!("cargo::rerun-if-env-changed=KERNEL");
    if let (Some(workspace_root), Some(kernel)) = (workspace_root, env::var_os("KERNEL")) {
        let kernel = workspace_root.join(kernel);
        println!("cargo::rerun-if-changed={}", kernel.display());
        println!("cargo::rustc-env=KERNEL={}", kernel.display());
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    copy_linker_script(&out_dir);
    copy_constants(&out_dir);
}

fn copy_linker_script(out_dir: &Path) {
    use std::{fs::File, io::Write};

    let mut f = File::create(out_dir.join("link.ld")).unwrap();
    f.write_all(include_bytes!("./riscv64-qemu.ld")).unwrap();

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-Tlink.ld");
}

fn copy_constants(out_dir: &Path) {
    println!("cargo::rerun-if-env-changed=K23_CONSTANTS");
    let code = env::var("K23_CONSTANTS").unwrap_or_default();

    let mut f = File::create(out_dir.join("constants.rs")).unwrap();
    f.write_all(code.as_bytes()).unwrap();
}
