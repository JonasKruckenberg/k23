use std::fs::File;
use std::process::Command;

use ecma_119::build::{BootConfigBuilder, DirectoryBuilder, FileSource, ImageBuilder};
use ecma_119::eltorito::CatalogEntry;
use ecma_119::{Directory, DirectoryEntry, Image, ParseError, SECTOR_SIZE, VIRTUAL_SECTOR_SIZE};
use fallible_iterator::FallibleIterator;
use memmap2::MmapOptions;
use rand::seq::SliceRandom;

#[test]
fn test_name() -> Result<(), Box<dyn core::error::Error>> {
    let file = File::open("tests/debian-13.4.0-arm64-netinst.iso").unwrap();

    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };

    let image = Image::parse(&mmap).unwrap();
    print_entries(image.root(), "", 8).unwrap();

    Ok(())
}
//     // println!("{image:?}");

//     // let Some(DirectoryEntry::Directory(doc_dir)) = image
//     //     .root()
//     //     .entries()?
//     //     .find(|r| Ok(r.identifier().unwrap() == "DOC"))?
//     // else {
//     //     panic!("directory DOC not found")
//     // };

//     // let Some(DirectoryEntry::File(mailing)) = doc_dir
//     //     .entries()?
//     //     .find(|r| Ok(r.identifier().unwrap() == "MAILING_.TXT;1"))?
//     // else {
//     //     panic!("file MAILING_.TXT;1 not found")
//     // };

//     // let mailing = mailing.as_slice().unwrap();
//     // println!("{}", str::from_utf8(mailing).unwrap());

//     // print_entries(image.root(), "", 8).unwrap();

//     for entry in image._path_table_le().unwrap().unwrap() {
//         println!("{entry:?}");
//     }

//     // for entry in image._path_table_be().unwrap().unwrap() {
//     //     println!("{entry:?}");
//     // }

//     // for boot_record in image._boot_records() {
//     //     for entry in boot_record.boot_catalog_entries(&image).unwrap().unwrap() {
//     //         println!("{entry:?}");
//     //     }
//     // }

//     Ok(())
// }

fn print_entries(dir: Directory<'_, '_>, prefix: &str, limit: usize) -> Result<(), ParseError> {
    if limit == 0 {
        panic!("recursion limit")
    }

    let mut entries = dir.entries().unwrap().peekable();

    while let Some(entry) = entries.next()? {
        let is_last = entries.peek()?.is_none();
        let branch = if is_last { "└── " } else { "├── " };

        let identifier = entry.identifier().unwrap();
        if matches!(identifier, "." | "..") {
            continue;
        }

        println!("{}{}{identifier}", prefix, branch);

        if let DirectoryEntry::Directory(entry) = entry {
            let extension = if is_last { "    " } else { "│   " };
            let new_prefix = format!("{}{}", prefix, extension);
            print_entries(entry, &new_prefix, limit - 1)?;
        }
    }

    Ok(())
}

#[test]
fn replicate() -> Result<(), Box<dyn core::error::Error>> {
    let file = File::open("tests/debian-13.4.0-arm64-netinst.iso").unwrap();

    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };

    let img = Image::parse(&mmap).unwrap();
    let mut img_builder = ImageBuilder::new();

    // copy the files
    {
        let root_builder = img_builder.root();
        let root = img.root();

        copy_dir(root, root_builder);
    }

    // // copy the boot catalog
    // {
    //     let boot_record = img._boot_records().next().unwrap();
    //     let catalog = img_builder.boot_catalog();
    //     copy_boot_catalog(
    //         boot_record.boot_catalog_entries(&img).unwrap(),
    //         catalog,
    //         &mmap,
    //     );
    // }

    let mut file = File::create_new("./out.iso").unwrap();
    img_builder.finish(&mut file).unwrap();

    // Command::new("xorriso").args([
    //  "-abort_on", "WARNING", "-return_with", "WARNING", "32", \
    //         "-indev", "libs/ecma-119/out.iso", \
    //         "-check_media"

    // ])

    Ok(())
}

fn copy_dir<'a>(dir: Directory<'_, 'a>, builder: &mut DirectoryBuilder<'a>) {
    let mut entries: Vec<_> = dir.entries().unwrap().unwrap().collect();

    entries.shuffle(&mut rand::rng());

    for e in entries {
        let name = e.identifier().unwrap();

        if matches!(name, "." | "..") {
            continue;
        }

        match e {
            DirectoryEntry::Directory(directory) => {
                copy_dir(directory, builder.add_dir(name).unwrap());
            }
            DirectoryEntry::File(file) => {
                builder
                    .add_file(
                        file.identifier().unwrap(),
                        FileSource::from_bytes(file.as_slice().unwrap()),
                    )
                    .unwrap();
            }
        }
    }
}

fn copy_boot_catalog<'a>(
    mut entries: impl FallibleIterator<Item = CatalogEntry<'a>, Error = ParseError>,
    builder: &mut BootConfigBuilder<'a>,
    disk: &'a [u8],
) {
    // Slice a boot image's bytes out of the mmap.
    // load_rba is in 2048-byte ISO sectors; sector_count is in 512-byte virtual sectors.
    let image_bytes = |rba: u32, sector_count: u32| -> &'a [u8] {
        let start = (rba as usize) * SECTOR_SIZE;
        let len = (sector_count as usize) * VIRTUAL_SECTOR_SIZE;
        &disk[start..start + len]
    };

    while let Some(entry) = entries.next().unwrap() {
        match entry {
            CatalogEntry::Validation(_) => {} // NB: will be added automatically
            CatalogEntry::InitialEntry(e) => {
                let bytes = image_bytes(e.load_rba.get() as u32, e.sector_count.get() as u32);
                builder.default_entry(e.emulation(), FileSource::from_bytes(bytes));
            }
            CatalogEntry::Header(h) => {
                let section = builder.section(h.platform(), h.id).unwrap();

                while let Some(entry) = entries.next().unwrap() {
                    match entry {
                        CatalogEntry::Entry(e) => {
                            let bytes = image_bytes(e.load_rba.get(), e.sector_count.get() as u32);
                            section
                                .entry(e.emulation(), FileSource::from_bytes(bytes))
                                .unwrap();
                        }
                        CatalogEntry::Extension(_) => {
                            todo!("extension builder")
                        }
                        _ => break,
                    }
                }
            }
            _ => unreachable!(),
        }
    }
}
