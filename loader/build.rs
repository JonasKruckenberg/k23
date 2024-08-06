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

    let verifying_key = if let Some(verifying_key_path) = env::var_os("K23_VERIFYING_KEY_PATH") {
        let verifying_key_path = workspace_root.join(verifying_key_path);

        println!("cargo::rerun-if-changed={}", verifying_key_path.display());

        format!(r#"include_bytes!("{}")"#, verifying_key_path.display())
    } else {
        "&[0; ::ed25519_dalek::PUBLIC_KEY_LENGTH]".to_string()
    };

    let payload = if let Some(payload_path) = env::var_os("K23_PAYLOAD_PATH") {
        let payload_path = workspace_root.join(payload_path);
        let len = fs::metadata(&payload_path).unwrap().len();

        println!("cargo::rerun-if-changed={}", payload_path.display());

        format!(
            r#"&[u8; {len}] = include_bytes!("{}")"#,
            payload_path.display()
        )
    } else {
        "&[u8; 0] = &[]".to_string()
    };

    fs::write(
        out_dir.join("gen.rs"),
        format!(
            r#"
    pub const VERIFYING_KEY: &[u8; ::ed25519_dalek::PUBLIC_KEY_LENGTH] = {verifying_key};
    pub const PAYLOAD: {payload};
    "#,
        ),
    )
    .unwrap();
}
