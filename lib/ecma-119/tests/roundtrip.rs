// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Round-trip integration test: build a small image in-memory, parse it back,
//! and assert the directory tree, file contents, and El Torito boot catalog
//! all survive the trip.

use std::io::Cursor;

use ecma_119::build::{
    BootConfig, BootEntry, Directory as DirBuilder, File as FileBuilder, ImageBuilder,
};
use ecma_119::eltorito::{BootPlatform, EmulationType, InitialEntry, ValidationEntry};
use ecma_119::{DirectoryEntry, Image};
use fallible_iterator::FallibleIterator;
use zerocopy::FromBytes;

const HELLO: &[u8] = b"hello, world\n";
const DEEP: &[u8] = b"deeper content";
const EFI_STUB: &[u8] = b"MZ\x90\x00fake EFI binary payload";

const SECTOR: usize = 2048;
// Per assign_lbas with boot on: 0..15 system, 16 PVD, 17 Boot Record VD,
// 18 VD Set Terminator, 19 Boot Catalog.
const BOOT_CATALOG_LBA: usize = 19;

fn build_image(with_boot: bool) -> Vec<u8> {
    let mut root = DirBuilder::new();
    root.add_file("HELLO.TXT;1", FileBuilder::from_bytes(HELLO).unwrap())
        .unwrap();
    root.add_file("EMPTY.BIN;1", FileBuilder::from_bytes(&[][..]).unwrap())
        .unwrap();

    let mut sub = DirBuilder::new();
    sub.add_file("DEEP.TXT;1", FileBuilder::from_bytes(DEEP).unwrap())
        .unwrap();
    root.add_subdir("SUB", sub).unwrap();

    if with_boot {
        let mut efi_dir = DirBuilder::new();
        let mut boot_dir = DirBuilder::new();
        boot_dir
            .add_file("BOOTAA64.EFI;1", FileBuilder::from_bytes(EFI_STUB).unwrap())
            .unwrap();
        efi_dir.add_subdir("BOOT", boot_dir).unwrap();
        root.add_subdir("EFI", efi_dir).unwrap();
    }

    let mut builder = ImageBuilder::new();
    builder.system_id("TEST SYS").unwrap();
    builder.volume_id("ROUNDTRIP").unwrap();
    builder.set_root(root);

    if with_boot {
        builder.set_boot_catalog(BootConfig::new(
            BootPlatform::Efi,
            BootEntry::new(EmulationType::NoEmulation, "EFI/BOOT/BOOTAA64.EFI;1"),
        ));
    }

    let mut buf = Cursor::new(Vec::new());
    builder.finish(&mut buf).unwrap();
    buf.into_inner()
}

fn collect_entries(dir: ecma_119::Directory<'_, '_>) -> Vec<(String, bool, Option<u32>)> {
    let mut entries: Vec<(String, bool, Option<u32>)> = Vec::new();
    let mut iter = dir.entries().unwrap();
    while let Some(entry) = iter.next().unwrap() {
        let id = entry.identifier().unwrap().to_owned();
        if matches!(id.as_str(), "." | "..") {
            continue;
        }
        let (is_dir, size) = match &entry {
            DirectoryEntry::Directory(_) => (true, None),
            DirectoryEntry::File(f) => (false, Some(f.size())),
        };
        entries.push((id, is_dir, size));
    }
    entries.sort();
    entries
}

/// Reads the InitialEntry that immediately follows the ValidationEntry at the
/// known boot-catalog LBA. The Image API doesn't currently expose boot
/// records, so we go through the bytes the writer produced.
fn read_initial_entry(bytes: &[u8]) -> InitialEntry {
    let catalog_off = BOOT_CATALOG_LBA * SECTOR;
    let initial_off = catalog_off + size_of::<ValidationEntry>();
    let initial_bytes = &bytes[initial_off..initial_off + size_of::<InitialEntry>()];
    InitialEntry::read_from_bytes(initial_bytes).expect("InitialEntry layout matches catalog body")
}

#[test]
fn build_then_parse() {
    let bytes = build_image(false);

    let img = Image::parse_relaxed(&bytes).expect("relaxed parse of builder output");

    assert_eq!(img.system_id().as_str().unwrap().trim_end(), "TEST SYS");
    assert_eq!(img.volume_id().as_str().unwrap().trim_end(), "ROUNDTRIP");

    assert_eq!(
        collect_entries(img.root().unwrap()),
        vec![
            ("EMPTY.BIN;1".to_string(), false, Some(0)),
            ("HELLO.TXT;1".to_string(), false, Some(HELLO.len() as u32)),
            ("SUB".to_string(), true, None),
        ],
    );

    let hello = img
        .root()
        .unwrap()
        .entries()
        .unwrap()
        .find(|e| Ok(e.identifier()? == "HELLO.TXT;1"))
        .unwrap()
        .expect("HELLO.TXT;1 not found");
    let DirectoryEntry::File(hello) = hello else {
        panic!("HELLO.TXT;1 should be a file");
    };
    assert_eq!(hello.as_slice().unwrap(), HELLO);

    let sub = img
        .root()
        .unwrap()
        .entries()
        .unwrap()
        .find(|e| Ok(e.identifier()? == "SUB"))
        .unwrap()
        .expect("SUB not found");
    let DirectoryEntry::Directory(sub) = sub else {
        panic!("SUB should be a directory");
    };
    assert_eq!(
        collect_entries(sub),
        vec![("DEEP.TXT;1".to_string(), false, Some(DEEP.len() as u32))],
    );
}

#[test]
fn boot_catalog_auto_sector_count() {
    let bytes = build_image(true);
    let _ = Image::parse_relaxed(&bytes).expect("relaxed parse with boot catalog");

    let initial = read_initial_entry(&bytes);

    assert_eq!(initial.boot_indicator, 0x88, "bootable flag");
    // EFI_STUB is 25 bytes → ceil(25 / 512) = 1 virtual sector.
    assert_eq!(initial.sector_count.get(), 1, "auto-computed sector count");
    assert_ne!(initial.load_rba.get(), 0, "boot image LBA must be resolved");
}

#[test]
fn explicit_load_size_overrides_auto() {
    let mut root = DirBuilder::new();
    let mut efi = DirBuilder::new();
    let mut boot = DirBuilder::new();
    boot.add_file(
        "BOOTAA64.EFI;1",
        FileBuilder::from_bytes(&b"x"[..]).unwrap(),
    )
    .unwrap();
    efi.add_subdir("BOOT", boot).unwrap();
    root.add_subdir("EFI", efi).unwrap();

    let mut entry = BootEntry::new(EmulationType::NoEmulation, "EFI/BOOT/BOOTAA64.EFI;1");
    entry.set_load_size(42);

    let mut builder = ImageBuilder::new();
    builder.volume_id("OVERRIDE").unwrap();
    builder.set_root(root);
    builder.set_boot_catalog(BootConfig::new(BootPlatform::Efi, entry));

    let mut buf = Cursor::new(Vec::new());
    builder.finish(&mut buf).unwrap();
    let bytes = buf.into_inner();

    let initial = read_initial_entry(&bytes);
    assert_eq!(
        initial.sector_count.get(),
        42,
        "explicit load_size must win over auto-computed",
    );
}
