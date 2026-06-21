// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

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

    /// Path to the kernel ELF to embed at `EFI/k23/kernel.debug`.
    #[arg(long)]
    kernel_debuginfo: PathBuf,

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

    build_esp(
        &args.loader,
        &args.kernel,
        &args.kernel_debuginfo,
        args.arch,
        &args.esp_out,
    )
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
    // UEFI §13.3.2.1: an EFI no-emulation entry with `sector_count` < 2 tells
    // firmware the boot partition extends to end-of-CD. The ecma-119 layout
    // pass places boot images last, so this is always safe — and it's the
    // only way to express an ESP whose virtual sector count exceeds u16::MAX.
    let mut entry = BootEntry::new(EmulationType::NoEmulation, "BOOT/EFI.IMG");
    entry.set_load_size(0);
    img.set_boot_catalog(BootConfig::new(BootPlatform::Efi, entry));

    let out = File::create(&args.output)
        .with_context(|| format!("creating output {}", args.output.display()))?;
    img.finish(out).context("writing ISO")?;

    Ok(())
}

/// Build a minimally-sized FAT ESP containing `EFI/BOOT/{boot_file}`, `EFI/k23/kernel.elf`,
/// and  `EFI/k23/kernel.debug`. The ESP is materialized directly at `esp_path`, streaming
/// the loader and kernel (and debug file) from disk so we never hold them in memory.
fn build_esp(
    loader: &Path,
    kernel: &Path,
    kernel_debuginfo: &Path,
    arch: Arch,
    esp_path: &Path,
) -> io::Result<()> {
    // Floor the volume at 2 MiB so FAT12/16 always have room for cluster
    // tables + directories, then add headroom for FS overhead. fatfs picks
    // the smallest FAT variant that fits (FAT32 needs ~33 MiB minimum).
    let size = {
        const SECTOR: u64 = 512;
        const HEADROOM: u64 = 2 * 1024 * 1024;
        const MIN_SIZE: u64 = 2 * 1024 * 1024;

        let loader_len = fs::metadata(loader)?.len();
        let kernel_len = fs::metadata(kernel)?.len();
        let kernel_debuginfo_len = fs::metadata(kernel_debuginfo)?.len();

        let payload = loader_len
            .saturating_add(kernel_len)
            .saturating_add(kernel_debuginfo_len);

        let raw = cmp::max(payload, MIN_SIZE).saturating_add(HEADROOM);
        raw.div_ceil(SECTOR) * SECTOR
    };

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

        let mut d = k23.create_file("kernel.debug")?;
        d.truncate()?;
        io::copy(&mut File::open(kernel_debuginfo)?, &mut d)?;

        let mut k = k23.create_file("kernel.elf")?;
        k.truncate()?;
        io::copy(&mut File::open(kernel)?, &mut k)?;
    }
    fs.unmount()?;

    Ok(())
}
