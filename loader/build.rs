use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};
use toml::Table;

fn main() {
    let workspace_root = Path::new(env!("CARGO_RUSTC_CURRENT_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let maybe_config = parse_kconfig(workspace_root);

    // process the configuration
    if let Some(config) = maybe_config {
        let loader_config = get_table(&config, "loader").unwrap();

        let linker_script_in = workspace_root.join(
            get_str(loader_config, "linker-script")
                .expect("config is missing `loader.linker-script`"),
        );
        let linker_script_out = out_dir.join("linker.x");
        fs::copy(linker_script_in, &linker_script_out).unwrap();

        println!("cargo::rustc-link-arg=-T{}", linker_script_out.display());
        println!(
            "cargo::rerun-if-env-changed={}",
            linker_script_out.display()
        );
    }

    // handle the kernel inclusion
    // let verifying_key = include_from_env(workspace_root, env::var_os("K23_VERIFYING_KEY_PATH"));
    // let signature = include_from_env(workspace_root, env::var_os("K23_SIGNATURE_PATH"));
    let kernel = include_from_env(workspace_root, env::var_os("K23_KERNEL_PATH"));

    println!("cargo::rerun-if-env-changed=K23_VERIFYING_KEY_PATH");
    println!("cargo::rerun-if-env-changed=K23_KERNEL_PATH");

    //pub const VERIFYING_KEY: Option<&[u8; ::ed25519_dalek::PUBLIC_KEY_LENGTH]> = {verifying_key};
    //pub const SIGNATURE: Option<&[u8; ::ed25519_dalek::Signature::BYTE_SIZE]> = {signature};
    fs::write(
        out_dir.join("kernel.rs"),
        format!(
            r#"
    pub static KERNEL: &[u8] = {kernel};
    "#,
        ),
    )
    .unwrap();
}

fn parse_kconfig(workspace_root: &Path) -> Option<Table> {
    let path = env::var_os("K23_CONFIG")?;
    println!("cargo::rerun-if-env-changed=K23_CONFIG");

    Some(toml::from_str(&fs::read_to_string(workspace_root.join(path)).unwrap()).unwrap())
}

fn get_table<'a>(table: &'a Table, key: &str) -> Option<&'a Table> {
    table.get(key).and_then(|v| v.as_table())
}

fn get_str<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(|v| v.as_str())
}

fn include_from_env(workspace_root: &Path, var: Option<OsString>) -> String {
    if let Some(path) = var {
        let path = workspace_root.join(path);

        println!("cargo::rerun-if-changed={}", path.display());
        format!(r#"include_bytes!("{}")"#, path.display())
    } else {
        "&[]".to_string()
    }
}
