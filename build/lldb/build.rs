use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rustc-link-search=/Users/jonas/Documents/GitHub/k23/lldb-build/lib");
    println!("cargo:rustc-link-lib=dylib=lldb");
    println!("cargo:include=/Users/jonas/Documents/GitHub/k23/lldb-build");

    let mut build_config = cpp_build::Config::new();
    build_config.include("/Users/jonas/Documents/GitHub/k23/lldb-build");
    build_config.cpp_set_stdlib(Some("c++"));
    build_config.build("src/lib.rs");

    let generated_lib = Path::new(&env::var("OUT_DIR").unwrap()).join(if cfg!(unix) {
        "librust_cpp_generated.a"
    } else {
        "rust_cpp_generated.lib"
    });
    println!("cargo:GENERATED={}", generated_lib.display());
}

// use glob::{MatchOptions, Pattern};
// use std::fs::File;
// use std::io::{Error, ErrorKind, Read, Seek, SeekFrom};
// use std::path::PathBuf;
// use std::process::Command;
// use std::{env, fs, io, path::Path};
//
// fn main() {
//     let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
//     let no_link_args = env::var("CARGO_FEATURE_NO_LINK_ARGS").is_ok();
//
//     // Rebuild if any of the source files change
//     rerun_if_changed_in(Path::new("src"));
//
//     let mut build_config = cpp_build::Config::new();
//
//     let includedir = {
//         let output = run_llvm_config(&["--includedir"]);
//         let directory = PathBuf::from(output.trim_end());
//         directory
//     };
//
//     println!("cargo:include={}", includedir.display());
//     build_config.include(includedir.clone());
//
//     if no_link_args {
//         build_config.cpp_set_stdlib(None);
//     } else {
//         // This branch is used when building unit tests, etc.
//
//         println!("cargo:rustc-link-search=/Users/jonas/Documents/GitHub/k23/lldb-build/lib/");
//
//         let (_directory, filename) = find_liblldb().unwrap();
//
//         // println!("cargo:rustc-link-search={}", directory.display());
//
//         let name = filename.trim_start_matches("lib");
//
//         let name = match name.find(".dylib").or_else(|| name.find(".so")) {
//             Some(index) => &name[0..index],
//             None => name,
//         };
//
//         println!("cargo:rustc-link-lib=dylib={name}");
//
//         if target_os == "linux" {
//             build_config.cpp_set_stdlib(Some("c++"));
//             println!("cargo:rustc-link-arg=--no-undefined");
//         } else if target_os == "macos" {
//             build_config.cpp_set_stdlib(Some("c++"));
//         }
//     }
//
//     // Generate C++ bindings
//     build_config.build("src/lib.rs");
//
//     let generated_lib = Path::new(&env::var("OUT_DIR").unwrap()).join(if cfg!(unix) {
//         "librust_cpp_generated.a"
//     } else {
//         "rust_cpp_generated.lib"
//     });
//     println!("cargo:GENERATED={}", generated_lib.display());
// }
//
// fn rerun_if_changed_in(dir: &Path) {
//     for entry in fs::read_dir(dir).unwrap() {
//         let entry = entry.unwrap();
//         if entry.file_type().unwrap().is_file() {
//             println!("cargo:rerun-if-changed={}", entry.path().display());
//         } else {
//             rerun_if_changed_in(&entry.path());
//         }
//     }
// }
//
// fn run_command(path: &str, arguments: &[&str]) -> String {
//     println!("path {path}");
//     let output = match Command::new(path).args(arguments).output() {
//         Ok(output) => output,
//         Err(error) => {
//             panic!("error: {error} from command {path} {arguments:?}");
//         }
//     };
//
//     if output.status.success() {
//         String::from_utf8_lossy(&output.stdout).into_owned()
//     } else {
//         panic!("exit code: {}", output.status);
//     }
// }
//
// /// Executes the `llvm-config` command and returns the `stdout` output if the
// /// command was successfully executed (errors are added to `COMMAND_ERRORS`).
// pub fn run_llvm_config(arguments: &[&str]) -> String {
//     let path = env::var("LLVM_CONFIG_PATH").unwrap_or_else(|_| "llvm-config".into());
//     run_command(&path, arguments)
// }
//
// /// Executes the `xcode-select` command and returns the `stdout` output if the
// /// command was successfully executed (errors are added to `COMMAND_ERRORS`).
// pub fn run_xcode_select(arguments: &[&str]) -> String {
//     run_command("xcode-select", arguments)
// }
//
// const DIRECTORIES_HAIKU: &[&str] = &[
//     "/boot/home/config/non-packaged/develop/lib",
//     "/boot/home/config/non-packaged/lib",
//     "/boot/system/non-packaged/develop/lib",
//     "/boot/system/non-packaged/lib",
//     "/boot/system/develop/lib",
//     "/boot/system/lib",
// ];
//
// const DIRECTORIES_LINUX: &[&str] = &[
//     "/usr/local/llvm*/lib*",
//     "/usr/local/lib*/*/*",
//     "/usr/local/lib*/*",
//     "/usr/local/lib*",
//     "/usr/lib*/*/*",
//     "/usr/lib*/*",
//     "/usr/lib*",
// ];
//
// const DIRECTORIES_MACOS: &[&str] = &[
//     "/usr/local/opt/llvm*/lib/llvm*/lib",
//     "/Library/Developer/CommandLineTools/usr/lib",
//     "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
//     "/usr/local/opt/llvm*/lib",
// ];
//
// const DIRECTORIES_WINDOWS: &[(&str, bool)] = &[
//     // LLVM + Clang can be installed using Scoop (https://scoop.sh).
//     // Other Windows package managers install LLVM + Clang to other listed
//     // system-wide directories.
//     ("C:\\Users\\*\\scoop\\apps\\llvm\\current\\lib", true),
//     ("C:\\MSYS*\\MinGW*\\lib", false),
//     ("C:\\Program Files*\\LLVM\\lib", true),
//     ("C:\\LLVM\\lib", true),
//     // LLVM + Clang can be installed as a component of Visual Studio.
//     // https://github.com/KyleMayes/clang-sys/issues/121
//     (
//         "C:\\Program Files*\\Microsoft Visual Studio\\*\\BuildTools\\VC\\Tools\\Llvm\\**\\lib",
//         true,
//     ),
// ];
//
// const DIRECTORIES_ILLUMOS: &[&str] = &["/opt/ooce/llvm-*/lib", "/opt/ooce/lldb-*/lib"];
//
// fn search_directory(directory: &Path, filenames: &[String]) -> Vec<(PathBuf, String)> {
//     // Escape the specified directory in case it contains characters that have
//     // special meaning in glob patterns (e.g., `[` or `]`).
//     let directory = Pattern::escape(directory.to_str().unwrap());
//     let directory = Path::new(&directory);
//
//     // Join the escaped directory to the filename glob patterns to obtain
//     // complete glob patterns for the files being searched for.
//     let paths = filenames
//         .iter()
//         .map(|f| directory.join(f).to_str().unwrap().to_owned());
//
//     // Prevent wildcards from matching path separators to ensure that the search
//     // is limited to the specified directory.
//     let mut options = MatchOptions::new();
//     options.require_literal_separator = true;
//
//     paths
//         .map(|p| glob::glob_with(&p, options))
//         .filter_map(Result::ok)
//         .flatten()
//         .filter_map(|p| {
//             let path = p.ok()?;
//             let filename = path.file_name()?.to_str().unwrap();
//
//             // The `libclang_shared` library has been renamed to `libclang-cpp`
//             // in Clang 10. This can cause instances of this library (e.g.,
//             // `libclang-cpp.so.10`) to be matched by patterns looking for
//             // instances of `libclang`.
//             if filename.contains("-cpp.") {
//                 return None;
//             }
//
//             Some((directory.to_owned(), filename.into()))
//         })
//         .collect::<Vec<_>>()
// }
//
// fn search_directories(directory: &Path, filenames: &[String]) -> Vec<(PathBuf, String)> {
//     let mut results = search_directory(directory, filenames);
//
//     // On Windows, `libclang.dll` is usually found in the LLVM `bin` directory
//     // while `libclang.lib` is usually found in the LLVM `lib` directory. To
//     // keep things consistent with other platforms, only LLVM `lib` directories
//     // are included in the backup search directory globs so we need to search
//     // the LLVM `bin` directory here.
//     if cfg!(target_os = "windows") && directory.ends_with("lib") {
//         let sibling = directory.parent().unwrap().join("bin");
//         results.extend(search_directory(&sibling, filenames));
//     }
//
//     results
// }
//
// pub fn search_libclang_directories(filenames: &[String], variable: &str) -> Vec<(PathBuf, String)> {
//     // Search only the path indicated by the relevant environment variable
//     // (e.g., `LIBCLANG_PATH`) if it is set.
//     if let Ok(path) = env::var(variable).map(|d| Path::new(&d).to_path_buf()) {
//         // Check if the path is a matching file.
//         if let Some(parent) = path.parent() {
//             let filename = path.file_name().unwrap().to_str().unwrap();
//             let libraries = search_directories(parent, filenames);
//             if libraries.iter().any(|(_, f)| f == filename) {
//                 return vec![(parent.into(), filename.into())];
//             }
//         }
//
//         // Check if the path is directory containing a matching file.
//         return search_directories(&path, filenames);
//     }
//
//     let mut found = vec![];
//
//     // Search the `bin` and `lib` directories in the directory returned by
//     // `llvm-config --prefix`.
//     let output = run_llvm_config(&["--prefix"]);
//     let directory = Path::new(output.lines().next().unwrap()).to_path_buf();
//     found.extend(search_directories(&directory.join("bin"), filenames));
//     found.extend(search_directories(&directory.join("lib"), filenames));
//     found.extend(search_directories(&directory.join("lib64"), filenames));
//
//     // Search the toolchain directory in the directory returned by
//     // `xcode-select --print-path`.
//     if cfg!(target_os = "macos") {
//         let output = run_xcode_select(&["--print-path"]);
//
//         let directory = Path::new(output.lines().next().unwrap()).to_path_buf();
//         let directory = directory.join("Toolchains/XcodeDefault.xctoolchain/usr/lib");
//         found.extend(search_directories(&directory, filenames));
//     }
//
//     // Search the directories in the `LD_LIBRARY_PATH` environment variable.
//     if let Ok(path) = env::var("LD_LIBRARY_PATH") {
//         for directory in env::split_paths(&path) {
//             found.extend(search_directories(&directory, filenames));
//         }
//     }
//
//     // Determine the `liblldb` directory patterns.
//     let directories: Vec<&str> = if cfg!(target_os = "haiku") {
//         DIRECTORIES_HAIKU.into()
//     } else if cfg!(target_os = "linux") || cfg!(target_os = "freebsd") {
//         DIRECTORIES_LINUX.into()
//     } else if cfg!(target_os = "macos") {
//         DIRECTORIES_MACOS.into()
//     } else if cfg!(target_os = "windows") {
//         let msvc = cfg!(target_env = "msvc");
//         DIRECTORIES_WINDOWS
//             .iter()
//             .filter(|d| d.1 || !msvc)
//             .map(|d| d.0)
//             .collect()
//     } else if cfg!(target_os = "illumos") {
//         DIRECTORIES_ILLUMOS.into()
//     } else {
//         vec![]
//     };
//
//     // Search the directories provided by the `liblldb` directory patterns.
//     let mut options = MatchOptions::new();
//     options.case_sensitive = false;
//     options.require_literal_separator = true;
//     for directory in directories {
//         if let Ok(directories) = glob::glob_with(directory, options) {
//             for directory in directories.filter_map(Result::ok).filter(|p| p.is_dir()) {
//                 found.extend(search_directories(&directory, filenames));
//             }
//         }
//     }
//
//     found
// }
//
// /// Extracts the ELF class from the ELF header in a shared library.
// fn parse_elf_header(path: &Path) -> io::Result<u8> {
//     let mut file = File::open(path)?;
//     let mut buffer = [0; 5];
//     file.read_exact(&mut buffer)?;
//     if buffer[..4] == [127, 69, 76, 70] {
//         Ok(buffer[4])
//     } else {
//         Err(Error::new(ErrorKind::InvalidData, "invalid ELF header"))
//     }
// }
//
// /// Extracts the magic number from the PE header in a shared library.
// fn parse_pe_header(path: &Path) -> io::Result<u16> {
//     let mut file = File::open(path)?;
//
//     // Extract the header offset.
//     let mut buffer = [0; 4];
//     let start = SeekFrom::Start(0x3C);
//     file.seek(start)?;
//     file.read_exact(&mut buffer)?;
//     let offset = i32::from_le_bytes(buffer);
//
//     // Check the validity of the header.
//     #[allow(clippy::cast_sign_loss)]
//     file.seek(SeekFrom::Start(offset as u64))?;
//     file.read_exact(&mut buffer)?;
//     if buffer != [80, 69, 0, 0] {
//         return Err(Error::new(ErrorKind::InvalidData, "invalid PE header"));
//     }
//
//     // Extract the magic number.
//     let mut buffer = [0; 2];
//     file.seek(SeekFrom::Current(20))?;
//     file.read_exact(&mut buffer)?;
//     Ok(u16::from_le_bytes(buffer))
// }
//
// /// Checks that a `libclang` shared library matches the target platform.
// fn validate_library(path: &Path) -> Result<(), String> {
//     if cfg!(any(target_os = "linux", target_os = "freebsd")) {
//         let class = parse_elf_header(path).map_err(|e| e.to_string())?;
//
//         if cfg!(target_pointer_width = "32") && class != 1 {
//             return Err("invalid ELF class (64-bit)".into());
//         }
//
//         if cfg!(target_pointer_width = "64") && class != 2 {
//             return Err("invalid ELF class (32-bit)".into());
//         }
//
//         Ok(())
//     } else if cfg!(target_os = "windows") {
//         let magic = parse_pe_header(path).map_err(|e| e.to_string())?;
//
//         if cfg!(target_pointer_width = "32") && magic != 267 {
//             return Err("invalid DLL (64-bit)".into());
//         }
//
//         if cfg!(target_pointer_width = "64") && magic != 523 {
//             return Err("invalid DLL (32-bit)".into());
//         }
//
//         Ok(())
//     } else {
//         Ok(())
//     }
// }
//
// fn parse_version(filename: &str) -> Vec<u32> {
//     let version = if let Some(version) = filename.strip_prefix("liblldb.so.") {
//         version
//     } else if filename.starts_with("liblldb-") {
//         &filename[9..filename.len() - 3]
//     } else {
//         return vec![];
//     };
//
//     version.split('.').map(|s| s.parse().unwrap_or(0)).collect()
// }
//
// fn search_liblldb_directories() -> Result<Vec<(PathBuf, String, Vec<u32>)>, String> {
//     let mut files = vec![format!(
//         "{}lldb{}",
//         env::consts::DLL_PREFIX,
//         env::consts::DLL_SUFFIX
//     )];
//
//     if cfg!(target_os = "linux") {
//         // Some Linux distributions don't create a `libclang.so` symlink, so we
//         // need to look for versioned files (e.g., `libclang-3.9.so`).
//         files.push("liblldb-*.so".into());
//     }
//
//     if cfg!(target_os = "freebsd")
//         || cfg!(target_os = "haiku")
//         || cfg!(target_os = "netbsd")
//         || cfg!(target_os = "openbsd")
//     {
//         // Some BSD distributions don't create a `liblldb.so` symlink either
//         files.push("liblldb.so.*".into());
//     }
//
//     if cfg!(target_os = "windows") {
//         // The official LLVM build uses `liblldb.dll` on Windows instead of
//         // `clang.dll`. However, unofficial builds such as MinGW use `lldb.dll`.
//         files.push("liblldb.dll".into());
//     }
//
//     let mut valid = vec![];
//     let mut invalid = vec![];
//
//     for (directory, filename) in search_libclang_directories(&files, "LIBCLANG_PATH") {
//         let path = directory.join(&filename);
//         match validate_library(&path) {
//             Ok(()) => {
//                 let version = parse_version(&filename);
//                 valid.push((directory, filename, version));
//             }
//             Err(message) => invalid.push(format!("({}: {})", path.display(), message)),
//         }
//     }
//
//     if !valid.is_empty() {
//         return Ok(valid);
//     }
//
//     panic!("invalid libs {invalid:?}");
// }
//
// fn find_liblldb() -> Result<(PathBuf, String), String> {
//     search_liblldb_directories()?
//         .iter()
//         .rev()
//         .max_by_key(|f| &f.2)
//         .cloned()
//         .map(|(path, filename, _)| (path, filename))
//         .ok_or_else(|| "unreachable".into())
// }
