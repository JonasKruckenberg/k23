use std::fs;

use ecma_119::{Directory, DirectoryEntry, Image};
use fallible_iterator::FallibleIterator;
use memmap2::MmapOptions;

#[test]
fn test_name() -> Result<(), Box<dyn core::error::Error>> {
    let file = fs::File::open("tests/debian-13.4.0-arm64-netinst.iso").unwrap();

    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };

    let image = Image::parse_relaxed(&mmap).inspect_err(|e| println!("{e}"))?;

    print_entries(image.root()?, "", 8)?;

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

fn print_entries(dir: Directory<'_, '_>, prefix: &str, limit: usize) -> anyhow::Result<()> {
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
