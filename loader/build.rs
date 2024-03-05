use std::path::PathBuf;
use std::{env, fs};

const LINKER: &[u8] = include_bytes!("riscv64-virt.ld");

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let ld = out_dir.join("linker.ld");
    fs::write(&ld, LINKER).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LOG");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
}
