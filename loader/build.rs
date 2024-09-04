use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let kernel = build_kernel(&out_dir);
    let kernel = compress_kernel(&out_dir, &kernel);
    println!("cargo::rustc-env=KERNEL={}", kernel.display());

    copy_linker_script();
}

fn build_kernel(out_dir: &Path) -> PathBuf {
    let res = escargot::CargoBuild::new()
        .package("kernel")
        .current_release()
        .target("../kernel/riscv64gc-k23-none-kernel.json")
        .args(["-Zbuild-std=core,alloc"])
        .target_dir(out_dir)
        .run()
        .unwrap();

    res.path().to_path_buf()
}

fn compress_kernel(out_dir: &Path, kernel: &Path) -> PathBuf {
    let kernel_compressed = out_dir.join("kernel.lz4");

    let input = fs::read(&kernel).expect("failed to read file");
    let compressed = lz4_flex::compress_prepend_size(&input);
    fs::write(&kernel_compressed, &compressed).expect("failed to write file");

    kernel_compressed
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
