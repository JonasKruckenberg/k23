use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let workspace_root = Path::new(env!("CARGO_RUSTC_CURRENT_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    copy_linker_script();

    // handle the kernel inclusion
    let kernel = include_from_env(workspace_root, env::var_os("K23_KERNEL_PATH"));
    println!("cargo::rerun-if-env-changed=K23_KERNEL_PATH");

    fs::write(
        out_dir.join("kernel.rs"),
        format!(
            r#"
    /// Raw kernel image, inlined by the build script
    pub static KERNEL_BYTES: &[u8] = {kernel};
    "#,
        ),
    )
    .unwrap();
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

fn copy_linker_script() {
    use std::{fs::File, io::Write};

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir);
    let mut f = File::create(dest_path.join("link.ld")).unwrap();
    f.write_all(include_bytes!("./riscv64-qemu.ld")).unwrap();

    println!("cargo:rustc-link-search={}", dest_path.display());
    println!("cargo:rustc-link-arg=-Tlink.ld");
}
