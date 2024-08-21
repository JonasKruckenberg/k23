use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};

static LINKER: &[u8] = include_bytes!("./loader-riscv64-qemu.ld");

fn main() {
    let workspace_root = Path::new(env!("CARGO_RUSTC_CURRENT_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let ld_path = out_dir.join("linker.x");
    fs::write(&ld_path, LINKER).unwrap();
    println!("cargo::rustc-link-arg=-T{}", ld_path.display());
    println!("cargo::rerun-if-env-changed={}", ld_path.display());
    println!("cargo::rerun-if-env-changed=K23_VERIFYING_KEY_PATH");
    println!("cargo::rerun-if-env-changed=K23_PAYLOAD_PATH");

    let verifying_key = include_from_env(workspace_root, env::var_os("K23_VERIFYING_KEY_PATH"));
    let signature = include_from_env(workspace_root, env::var_os("K23_SIGNATURE_PATH"));
    let payload = include_from_env(workspace_root, env::var_os("K23_PAYLOAD_PATH"));
    let payload_size = if let Some(s) = env::var_os("K23_PAYLOAD_SIZE") {
        s.into_string().unwrap()
    } else {
        "0".into()
    };

    fs::write(
        out_dir.join("gen.rs"),
        format!(
            r#"
    pub const VERIFYING_KEY: Option<&[u8; ::ed25519_dalek::PUBLIC_KEY_LENGTH]> = {verifying_key};
    pub const SIGNATURE: Option<&[u8; ::ed25519_dalek::Signature::BYTE_SIZE]> = {signature};
    pub static PAYLOAD: Option<&[u8]> = {payload};
    pub const PAYLOAD_SIZE: usize = {payload_size};
    "#,
        ),
    )
    .unwrap();
}

fn include_from_env(workspace_root: &Path, var: Option<OsString>) -> String {
    if let Some(path) = var {
        let path = workspace_root.join(path);

        println!("cargo::rerun-if-changed={}", path.display());
        format!(r#"Some(include_bytes!("{}"))"#, path.display())
    } else {
        "None".to_string()
    }
}
