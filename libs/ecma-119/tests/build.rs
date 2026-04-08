use std::fs;

use ecma_119::build::{self, BootConfig, BootEntry, Directory as DirBuilder, ImageBuilder};
use ecma_119::eltorito::{BootPlatform, EmulationType};
use ecma_119::{Directory, DirectoryEntry, Image};
use fallible_iterator::FallibleIterator;
use memmap2::MmapOptions;
use rand::seq::SliceRandom;

#[test]
fn replicate() -> Result<(), Box<dyn core::error::Error>> {
    let file = fs::File::open(
        "/Users/jonaskruckenberg/Documents/k23/libs/ecma-119/tests/debian-13.4.0-arm64-netinst.iso",
    )
    .unwrap();

    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };

    let img = Image::parse_relaxed(&mmap).unwrap();
    let mut img_builder = ImageBuilder::new();

    img_builder.system_id(&img.system_id().as_str()?)?;
    img_builder.volume_id(&img.volume_id().as_str()?.to_uppercase().replace('.', &"_"))?;

    img_builder.volume_set_id(img.volume_set_id().as_str()?)?;
    img_builder.publisher_id(img.publisher_id().as_str()?)?;
    img_builder.data_preparer_id(img.data_preparer_id().as_str()?)?;
    img_builder.application_id(img.application_id().as_str()?)?;

    img_builder.set_root(build_dir(img.root().unwrap()));

    img_builder.set_boot_catalog(BootConfig::new(
        BootPlatform::Efi,
        BootEntry::new(EmulationType::NoEmulation, "EFI/BOOT/BOOTAA64.EFI;1"),
    ));

    let mut file = fs::File::create_new("./out.iso").unwrap();
    img_builder.finish(&mut file).unwrap();

    Ok(())
}

fn build_dir<'a>(dir: Directory<'_, 'a>) -> DirBuilder<'a> {
    let mut out = DirBuilder::new();
    let mut entries: Vec<_> = dir.entries().unwrap().unwrap().collect();

    entries.shuffle(&mut rand::rng());

    for e in entries {
        let name = e.identifier().unwrap();

        if matches!(name, "." | "..") {
            continue;
        }

        match e {
            DirectoryEntry::Directory(directory) => {
                out.add_subdir(name.replace('.', "_"), build_dir(directory))
                    .inspect_err(|e| eprintln!("{e}"))
                    .unwrap();
            }
            DirectoryEntry::File(file) => {
                out.add_file(
                    file.identifier().unwrap(),
                    build::File::from_bytes(file.as_slice().unwrap()),
                )
                .unwrap();
            }
        }
    }
    out
}
