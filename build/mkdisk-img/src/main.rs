use std::cmp;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ecma_119::build::{self, BootConfig, BootEntry};
use ecma_119::eltorito::{BootPlatform, EmulationType};
use fatfs::{FileSystem, FormatVolumeOptions, FsOptions};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Arch {
    #[value(name = "aarch64")]
    Aarch64,
    #[value(name = "riscv64")]
    Riscv64,
    #[value(name = "x86_64")]
    X86_64,
}

impl Arch {
    fn boot_file_name(self) -> &'static str {
        match self {
            Arch::Aarch64 => "BOOTAA64.EFI",
            Arch::Riscv64 => "BOOTRISCV64.EFI",
            Arch::X86_64 => "BOOTX64.EFI",
        }
    }
}

#[derive(Parser, Debug)]
#[command(about = "Build a UEFI-bootable ISO image from an EFI application")]
struct Args {
    /// Path to the EFI application to embed.
    #[arg(long)]
    loader: PathBuf,

    /// Path to the kernel ELF to embed at `EFI/k23/kernel.elf`.
    #[arg(long)]
    kernel: PathBuf,

    /// Target architecture — determines the `EFI/BOOT/BOOT{ARCH}.EFI` filename.
    #[arg(long)]
    arch: Arch,

    /// Output ISO path.
    #[arg(short, long)]
    output: PathBuf,

    /// Path for the intermediate FAT ESP image. Declared as a separate Buck
    /// output so the build graph tracks it.
    #[arg(long)]
    esp_out: PathBuf,

    /// Optional ISO volume identifier.
    #[arg(long)]
    volume_id: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    build_esp(&args.loader, &args.kernel, args.arch, &args.esp_out)
        .context("building ESP FAT image")?;

    let mut boot = build::Directory::new();
    boot.add_file("EFI.IMG", build::File::from_path(&args.esp_out)?)?;
    let mut root = build::Directory::new();
    root.add_subdir("BOOT", boot)?;

    let mut img = build::ImageBuilder::new();
    img.set_root(root);
    if let Some(vid) = args.volume_id.as_deref() {
        img.volume_id(vid)?;
    }
    img.set_boot_catalog(BootConfig::new(
        BootPlatform::Efi,
        BootEntry::new(EmulationType::NoEmulation, "BOOT/EFI.IMG"),
    ));

    let out = File::create(&args.output)
        .with_context(|| format!("creating output {}", args.output.display()))?;
    img.finish(out).context("writing ISO")?;

    Ok(())
}

/// Build a minimally-sized FAT ESP containing `EFI/BOOT/{boot_file}` and
/// `EFI/k23/kernel.elf`. The ESP is materialized directly at `esp_path`,
/// streaming the loader and kernel from disk so we never hold them in memory.
fn build_esp(loader: &Path, kernel: &Path, arch: Arch, esp_path: &Path) -> io::Result<()> {
    // Floor the volume at 2 MiB so FAT12/16 always have room for cluster
    // tables + directories, then add headroom for FS overhead. fatfs picks
    // the smallest FAT variant that fits (FAT32 needs ~33 MiB minimum).
    const SECTOR: u64 = 512;
    const HEADROOM: u64 = 2 * 1024 * 1024;
    const MIN_SIZE: u64 = 2 * 1024 * 1024;

    let loader_len = fs::metadata(loader)?.len();
    let kernel_len = fs::metadata(kernel)?.len();

    let payload = loader_len.saturating_add(kernel_len);
    let raw = cmp::max(payload, MIN_SIZE).saturating_add(HEADROOM);
    let size = raw.div_ceil(SECTOR) * SECTOR;

    let mut img = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(esp_path)?;
    img.set_len(size)?;
    img.seek(SeekFrom::Start(0))?;

    fatfs::format_volume(
        &mut img,
        FormatVolumeOptions::new().volume_label(*b"K23BOOT    "),
    )?;

    let fs = FileSystem::new(&mut img, FsOptions::new())?;
    {
        let root = fs.root_dir();
        let efi = root.create_dir("EFI")?;
        let boot = efi.create_dir("BOOT")?;
        let k23 = efi.create_dir("k23")?;

        let mut f = boot.create_file(arch.boot_file_name())?;
        f.truncate()?;
        io::copy(&mut File::open(loader)?, &mut f)?;

        let mut k = k23.create_file("kernel.elf")?;
        k.truncate()?;
        io::copy(&mut File::open(kernel)?, &mut k)?;
    }
    fs.unmount()?;

    Ok(())
}
