// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let workspace_root = env::var_os("__K23_CARGO_RUSTC_CURRENT_DIR").map(PathBuf::from);

    println!("cargo::rerun-if-env-changed=KERNEL");
    if let (Some(workspace_root), Some(kernel)) = (workspace_root, env::var_os("KERNEL")) {
        let kernel = workspace_root.join(kernel);
        println!("cargo::rerun-if-changed={}", kernel.display());
        println!("cargo::rustc-env=KERNEL={}", kernel.display());
    }

    copy_linker_script();
}

fn copy_linker_script() {
    use std::fs::File;
    use std::io::Write;

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir);
    let mut f = File::create(dest_path.join("link.ld")).unwrap();
    f.write_all(include_bytes!("./riscv64-qemu.ld")).unwrap();

    println!("cargo:rustc-link-search={}", dest_path.display());
    println!("cargo:rustc-link-arg=-Tlink.ld");
}
