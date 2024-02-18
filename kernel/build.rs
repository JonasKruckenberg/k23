use quote::quote;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{env, fs};

const LINKER: &[u8] = include_bytes!("riscv64-virt.ld");

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    make_kconfig(&out_dir)?;

    let ld = out_dir.join("linker.ld");
    fs::write(&ld, LINKER).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LOG");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    Ok(())
}

fn make_kconfig(out_dir: &Path) -> anyhow::Result<()> {
    let mut file = File::create(out_dir.join("kconfig.rs"))?;
    writeln!(file, "// Generated by build.rs, do not touch!")?;

    let default_stack_size = 16;
    let default_uart_baud_rate = 38400;
    let default_log_level = quote!(::log::Level::Info);
    let default_memory_mode = quote!(::vmm::Riscv64Sv39);

    let stack_size_pages = option_env!("K23_KCONFIG_STACK_SIZE_PAGES")
        .map(usize::from_str)
        .transpose()?
        .unwrap_or(default_stack_size);

    let uart_baud_rate = option_env!("K23_KCONFIG_UART_BAUD_RATE")
        .map(u32::from_str)
        .transpose()?
        .unwrap_or(default_uart_baud_rate);

    let log_level = option_env!("K23_KCONFIG_LOG_LEVEL")
        .or(option_env!("RUST_LOG"))
        .map(usize::from_str)
        .transpose()?;
    let log_level = match log_level {
        Some(1) => quote!(::log::Level::Error),
        Some(2) => quote!(::log::Level::Warn),
        Some(3) => quote!(::log::Level::Info),
        Some(4) => quote!(::log::Level::Debug),
        Some(5) => quote!(::log::Level::Trace),
        Some(_) => panic!("invalid log level"),
        None => default_log_level,
    };

    let memory_mode = match option_env!("K23_KCONFIG_MEMORY_MODE") {
        Some("Riscv64Sv39") => quote!(::vmm::Riscv64Sv39),
        Some("Riscv64Sv48") => quote!(::vmm::Riscv64Sv48),
        Some("Riscv64Sv57") => quote!(::vmm::Riscv64Sv57),
        Some(_) => panic!("invalid memory mode level"),
        None => default_memory_mode,
    };

    writeln!(
        file,
        "{}",
        quote!(pub const KCONFIG: ::kconfig::KConfig = ::kconfig::KConfig {
        stack_size_pages: #stack_size_pages,
        log_level: #log_level,
        uart_baud_rate: #uart_baud_rate,
    };)
    )?;

    writeln!(
        file,
        "{}",
        quote!(
            #[allow(non_camel_case_types)]
            pub type MEMORY_MODE = #memory_mode;
        )
    )?;

    Ok(())
}
