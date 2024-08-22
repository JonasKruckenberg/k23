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
}

struct KConfig<'a> {
    stack_size_pages: u64,
    trap_stack_size_pages: u64,
    heap_size_pages: u64,
    log_level: &'a str,
    memory_mode: &'a str,
}

impl<'a> KConfig<'a> {
    fn from_table(table: &'a Table) -> Self {
        let kernel_config = get_table(table, "kernel").expect("config is missing `kernel`");

        Self {
            stack_size_pages: get_uint(kernel_config, "stack-size-pages")
                .expect("config is missing `kernel.stack-size-pages`"),
            trap_stack_size_pages: get_uint(kernel_config, "trap-stack-size-pages")
                .expect("config is missing `kernel.trap-stack-size-pages`"),
            heap_size_pages: get_uint(kernel_config, "heap-size-pages")
                .expect("config is missing `kernel.heap-size-pages`"),
            log_level: get_str(kernel_config, "log-level")
                .expect("config is missing `kernel.log-level`"),
            memory_mode: get_str(table, "memory-mode").expect("config is missing `memory-mode`"),
        }
    }

    fn into_file(self, out_path: &Path) {
        let Self {
            stack_size_pages,
            trap_stack_size_pages,
            heap_size_pages,
            log_level,
            memory_mode,
        } = self;

        fs::write(
            out_path,
            format!(
                r#"
    pub const STACK_SIZE_PAGES: usize = {stack_size_pages};
    pub const TRAP_STACK_SIZE_PAGES: usize = {trap_stack_size_pages};
    pub const HEAP_SIZE_PAGES: usize = {heap_size_pages};
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
            stack_size_pages: 128,
            trap_stack_size_pages: 16,
            heap_size_pages: 8192, // 32 MiB
            log_level: "Debug",
            memory_mode: "Riscv64Sv39",
        }
    }
}

fn parse_kconfig(workspace_root: &Path) -> Option<toml::Table> {
    let path = env::var_os("K23_CONFIG")?;

    Some(toml::from_str(&std::fs::read_to_string(workspace_root.join(path)).unwrap()).unwrap())
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
