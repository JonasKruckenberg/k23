use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    println!("cargo::rerun-if-env-changed=K23_CONSTANTS");
    let code = env::var("K23_CONSTANTS").unwrap_or_default();

    let mut f = File::create(out_dir.join("constants.rs")).unwrap();
    f.write_all(code.as_bytes()).unwrap();
}
