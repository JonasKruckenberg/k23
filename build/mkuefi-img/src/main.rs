use std::fs::{self, File};
use std::io::{self, Cursor};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ecma_119::build::{self, BootConfig, BootEntry};
use ecma_119::eltorito::{BootPlatform, EmulationType};
use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Arch {
    Aarch64,
    Riscv64,
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
    efi: PathBuf,

    /// Target architecture — determines the `EFI/BOOT/BOOT{ARCH}.EFI` filename.
    #[arg(long)]
    arch: Arch,

    /// Output ISO path.
    #[arg(short, long)]
    output: PathBuf,

    /// Optional ISO volume identifier.
    #[arg(long)]
    volume_id: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let efi_app =
        fs::read(&args.efi).with_context(|| format!("reading EFI app {}", args.efi.display()))?;

    let esp = build_esp(&efi_app, args.arch).context("building ESP FAT image")?;

    let mut boot = build::Directory::new();
    boot.add_file("EFI.IMG", build::File::from_bytes(esp))?;
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

    println!("wrote {}", args.output.display());
    Ok(())
}

/// Build a minimally-sized FAT ESP containing `EFI/BOOT/{boot_file}`.
fn build_esp(efi_app: &[u8], arch: Arch) -> io::Result<Vec<u8>> {
    // FAT12 minimum is ~64 KiB. Size the image as efi_app + generous FS overhead,
    // rounded up to a sector boundary, with a 512 KiB floor for safe formatting.
    const SECTOR: usize = 512;
    const OVERHEAD: usize = 256 * 1024;
    const MIN_SIZE: usize = 512 * 1024;

    let raw = efi_app.len().saturating_add(OVERHEAD).max(MIN_SIZE);
    let size = raw.div_ceil(SECTOR) * SECTOR;

    let mut img = Cursor::new(vec![0u8; size]);
    fatfs::format_volume(
        &mut img,
        FormatVolumeOptions::new()
            .fat_type(FatType::Fat32)
            .volume_label(*b"K23BOOT    "),
    )?;

    let fs = FileSystem::new(&mut img, FsOptions::new())?;

    {
        let root = fs.root_dir();
        root.create_dir("EFI")?;
        root.create_dir("EFI/BOOT")?;
        let mut f = root.create_file(&format!("EFI/BOOT/{}", arch.boot_file_name()))?;
        f.truncate()?;
        io::Write::write_all(&mut f, efi_app)?;
    }
    fs.unmount()?;

    Ok(img.into_inner())
}
