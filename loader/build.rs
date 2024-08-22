use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};
use toml::Table;

fn main() {
    let workspace_root = Path::new(env!("CARGO_RUSTC_CURRENT_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let maybe_config = parse_kconfig(workspace_root);
    maybe_config
        .as_ref()
        .map(KConfig::from_table)
        .unwrap_or_default()
        .into_file(&out_dir.join("kconfig.rs"));

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

    // handle the payload inclusion
    // let verifying_key = include_from_env(workspace_root, env::var_os("K23_VERIFYING_KEY_PATH"));
    // let signature = include_from_env(workspace_root, env::var_os("K23_SIGNATURE_PATH"));
    let payload = include_from_env(workspace_root, env::var_os("K23_PAYLOAD_PATH"));

    println!("cargo::rerun-if-env-changed=K23_VERIFYING_KEY_PATH");
    println!("cargo::rerun-if-env-changed=K23_PAYLOAD_PATH");

    //pub const VERIFYING_KEY: Option<&[u8; ::ed25519_dalek::PUBLIC_KEY_LENGTH]> = {verifying_key};
    //pub const SIGNATURE: Option<&[u8; ::ed25519_dalek::Signature::BYTE_SIZE]> = {signature};
    fs::write(
        out_dir.join("payload.rs"),
        format!(
            r#"
    pub static PAYLOAD: Option<&[u8]> = {payload};
    "#,
        ),
    )
    .unwrap();
}

struct KConfig<'a> {
    stack_size_pages: u64,
    log_level: &'a str,
    memory_mode: &'a str,
}

impl<'a> KConfig<'a> {
    fn from_table(table: &'a Table) -> Self {
        let loader_config = get_table(table, "loader").expect("config is missing `loader`");

        Self {
            stack_size_pages: get_uint(loader_config, "stack-size-pages")
                .expect("config is missing `loader.stack-size-pages`"),
            log_level: get_str(loader_config, "log-level").unwrap_or_else(|| {
                get_str(table, "log-level").expect("config is missing `loader.log-level`")
            }),
            memory_mode: get_str(table, "memory-mode").expect("config is missing `memory-mode`"),
        }
    }

    fn into_file(self, out_path: &Path) {
        let Self {
            stack_size_pages,
            log_level,
            memory_mode,
        } = self;

        fs::write(
            out_path,
            format!(
                r#"
    pub const STACK_SIZE_PAGES: usize = {stack_size_pages};
    pub const LOG_LEVEL: ::log::Level = ::log::Level::{log_level};
    #[allow(non_camel_case_types)]
    pub type MEMORY_MODE = ::kmm::{memory_mode};
    pub const PAGE_SIZE: usize = <MEMORY_MODE as ::kmm::Mode>::PAGE_SIZE;
    "#,
            ),
        )
        .unwrap();
    }
}

impl<'a> Default for KConfig<'a> {
    fn default() -> Self {
        Self {
            stack_size_pages: 32,
            log_level: "Debug",
            memory_mode: "Riscv64Sv39",
        }
    }
}

fn parse_kconfig(workspace_root: &Path) -> Option<Table> {
    let path = env::var_os("K23_CONFIG")?;

    Some(toml::from_str(&fs::read_to_string(workspace_root.join(path)).unwrap()).unwrap())
}

fn get_table<'a>(table: &'a Table, key: &str) -> Option<&'a Table> {
    table.get(key).and_then(|v| v.as_table())
}

fn get_str<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(|v| v.as_str())
}

fn get_uint(table: &Table, key: &str) -> Option<u64> {
    table
        .get(key)
        .and_then(|v| v.as_integer())
        .map(|v| v as u64)
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
