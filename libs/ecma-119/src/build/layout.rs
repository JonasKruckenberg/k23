//! Code for laying out an image on disk

use std::collections::VecDeque;
use std::io::{self, SeekFrom};
use std::mem::size_of;

use zerocopy::byteorder::{BigEndian, U16, U32};
use zerocopy::{ByteOrder, FromZeros, IntoBytes, LittleEndian};

use super::directory::{DirectoryBuilder, FileSource};
use crate::build::BootConfigBuilder;
use crate::raw;
use crate::raw::{
    BothEndianU16, BothEndianU32, DirDateTime, DirectoryRecordHeader, FileFlags,
    PathTableRecordHeader, RootDirectoryRecord, SECTOR_SIZE, VolumeDescriptorHeader,
};

#[derive(Debug)]
pub(super) struct DirNode<'a> {
    pub(super) extent_lba: u32, // 0 until LBA assignment pass
    pub(super) extent_len: u32,

    pub(super) identifier: &'a str, // used to construct the PathTableRecord and DirectoryRecord
    pub(super) parent_directory: u16, // used to construct the PathTableRecord

    pub(super) files: Vec<FileNode<'a>>,

    /// Identifiers of `children` and `files` merged and sorted by name.
    /// Built once during flatten so `required_extent_size` doesn't have
    /// to re-sort on every call.
    sorted_entry_ids: Vec<(&'a str, DirNodeEntry)>,
}

#[derive(Debug)]
enum DirNodeEntry {
    // this is a direct index into the flat list of directories
    ChildDirectory(u16),
    // an index into this nodes list of files
    File(u16),
}

#[derive(Debug)]
pub(super) struct FileNode<'a> {
    pub(super) lba: u32, // 0 until LBA pass
    pub(super) len: usize,
    pub(super) identifier: &'a str,
    pub(super) source: FileSource<'a>,
}

impl<'a> FileNode<'a> {
    fn new(identifier: &'a str, source: FileSource<'a>) -> Self {
        Self {
            lba: 0,
            len: source.len(),
            identifier,
            source,
        }
    }
}

impl<'a> DirNode<'a> {
    fn new(identifier: &'a str, parent_directory: u16, children_size_hint: usize) -> Self {
        Self {
            extent_lba: 0,
            extent_len: 0,
            identifier,
            parent_directory,
            files: Vec::new(),
            sorted_entry_ids: Vec::with_capacity(children_size_hint),
        }
    }

    fn attach_child_directory(&mut self, name: &'a str, child_idx: u16) {
        self.sorted_entry_ids
            .push((name, DirNodeEntry::ChildDirectory(child_idx)));
    }

    fn attach_file(&mut self, name: &'a str, source: FileSource<'a>) {
        let file_idx = u16::try_from(self.files.len()).expect("too many directories");
        self.files.push(FileNode::new(name, source));
        self.sorted_entry_ids
            .push((name, DirNodeEntry::File(file_idx)));
    }

    fn required_path_table_record_size(&self) -> u32 {
        let header_size = const { size_of::<raw::PathTableRecordHeader<LittleEndian>>() as u32 };
        let id_len = self.identifier.len() as u32;

        // round up so the size is always even
        header_size + id_len + (id_len & 1)
    }

    fn required_extent_size(&self) -> u32 {
        let mut offset_in_sector = 0;
        let mut total = 0;

        let mut add_record = |id: &str| {
            let header_size = const { size_of::<raw::DirectoryRecordHeader>() as u32 };
            let id_len = id.len() as u32;

            let record_size = header_size + id_len + (1 - (id_len & 1));

            // no record may straddle a logical block boundary.
            // so we pad out to the end of the sector and continue
            if offset_in_sector + record_size > SECTOR_SIZE as u32 {
                total += SECTOR_SIZE as u32 - offset_in_sector; // pad rest of sector
                offset_in_sector = 0;
            }

            total += record_size;
            offset_in_sector = (offset_in_sector + record_size) % SECTOR_SIZE as u32;
        };

        // account for the `.` and `..`  special directories
        add_record("\x00");
        add_record("\x01");

        // ECMA-119 §6.8.1.1: subdirectory and file records share one
        // sequence sorted by file identifier within the dir extent.
        for (id, _) in &self.sorted_entry_ids {
            add_record(id);
        }

        total.next_multiple_of(SECTOR_SIZE as u32)
    }
}

pub(super) struct Layout<'a> {
    pub(super) dirs: Vec<DirNode<'a>>,
    pub(super) path_table_size: u32,
    pub(super) path_table_l_lba: u32,
    pub(super) path_table_m_lba: u32,
    pub(super) boot_catalog_lba: Option<u32>,
    pub(super) total_sectors: u32,
}

impl<'a> Layout<'a> {
    pub(super) fn flatten(root: DirectoryBuilder<'a>) -> Self {
        let mut dirs = Vec::new();
        let mut path_table_size = 0;

        // Queue holds (parent_index_in_dirs, builder_to_process).
        let mut queue: VecDeque<(u16, DirectoryBuilder<'a>)> = VecDeque::new();

        // NB: indices are 0-based in memory; the +1 to match the ECMA-119
        // 1-based path table numbering is applied at serialize time only.
        // Root's parent is itself (the wire value 1 becomes 0 + 1).
        let dir = DirNode::new("\0", 0, root.subdirs.len());
        path_table_size += dir.required_path_table_record_size();
        dirs.push(dir);
        queue.push_back((0, root));

        while let Some((parent_idx, mut dir)) = queue.pop_front() {
            // sort each category by name
            dir.subdirs.sort_unstable_by(|a, b| a.name.cmp(b.name));
            dir.files
                .sort_unstable_by(|(a_name, _), (b_name, _)| a_name.cmp(b_name));

            for subdir in dir.subdirs {
                let child_idx = u16::try_from(dirs.len()).expect("too many directories");

                let child = DirNode::new(subdir.name, parent_idx, subdir.subdirs.len());

                path_table_size += child.required_path_table_record_size();
                dirs.push(child);
                dirs[parent_idx as usize].attach_child_directory(subdir.name, child_idx);

                queue.push_back((child_idx, subdir));
            }

            for (file, source) in dir.files {
                dirs[parent_idx as usize].attach_file(file, source);
            }
        }

        // Pre-build the per-directory sorted entry list now that all children
        // are known. ECMA-119 §6.8.1.1 requires subdirectory and file records
        // to share one sequence sorted by identifier within the dir extent.
        dirs.iter_mut().for_each(|dir| {
            dir.sorted_entry_ids
                .sort_unstable_by(|(a_id, _), (b_id, _)| a_id.cmp(b_id))
        });

        Self {
            dirs,
            path_table_size,
            path_table_l_lba: 0,
            path_table_m_lba: 0,
            boot_catalog_lba: None,
            total_sectors: 0,
        }
    }

    pub(super) fn assign_lbas(&mut self, boot_config: Option<&BootConfigBuilder>) {
        // On-disk layout:
        //
        //   sectors 0..16   system area (zeroed)
        //   16              PVD
        //   17              Boot Record VD (if boot)
        //   17 or 18        VD Set Terminator
        //   ...             boot catalog (if boot)
        //   ...             Path Table L, then Path Table M
        //   ...             every directory extent, in BFS order
        //   ...             every file's data, in BFS order
        let mut lba: u32 = 16 + 1 + 1; // system area + PVD + terminator
        if let Some(boot_config) = boot_config {
            lba += 1; // Boot Record VD

            self.boot_catalog_lba = Some(lba);
            lba += boot_config.required_size().div_ceil(SECTOR_SIZE) as u32;
        }

        // account for both the L and M path tables
        self.path_table_l_lba = lba;
        lba += self.path_table_size.div_ceil(SECTOR_SIZE as u32);
        self.path_table_m_lba = lba;
        lba += self.path_table_size.div_ceil(SECTOR_SIZE as u32);

        // every directory extent, back to back
        for dir in &mut self.dirs {
            let extent_len = dir.required_extent_size();
            dir.extent_lba = lba;
            dir.extent_len = extent_len;
            lba += extent_len.div_ceil(SECTOR_SIZE as u32);
        }

        // every file's data, back to back. Zero-byte files get LBA 0
        // (libisofs convention) and consume no sectors.
        for dir in &mut self.dirs {
            for file in &mut dir.files {
                if file.len == 0 {
                    file.lba = 0;
                    continue;
                }
                file.lba = lba;
                lba += (file.len as u32).div_ceil(SECTOR_SIZE as u32);
            }
        }

        self.total_sectors = lba;
    }

    pub(crate) fn serialize(
        self,
        mut w: impl io::Write + io::Seek,
        mut pvd: raw::PrimaryVolumeDescriptor,
        boot_config: Option<&BootConfigBuilder>,
    ) -> io::Result<()> {
        // El Torito serialization is not yet implemented; bail before touching
        // the writer so no partial image is produced.
        if boot_config.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "El Torito boot image serialization is not yet implemented",
            ));
        }

        // Patch the PVD with the layout-computed fields that can only be
        // known after assign_lbas has run.
        let root = &self.dirs[0];
        let mut root_record = RootDirectoryRecord::new_zeroed();
        root_record.header = build_dir_record_header(root.extent_lba, root.extent_len, true, 1);

        pvd.logical_block_size = BothEndianU16::new(SECTOR_SIZE as u16);
        pvd.volume_set_size = BothEndianU16::new(1);
        pvd.volume_sequence_number = BothEndianU16::new(1);
        pvd.file_structure_version = 1;
        pvd.path_table_size = BothEndianU32::new(self.path_table_size);
        pvd.path_table_l_lba = U32::new(self.path_table_l_lba);
        pvd.path_table_m_lba = U32::new(self.path_table_m_lba);
        pvd.volume_space_size = BothEndianU32::new(self.total_sectors);
        pvd.root_directory_record = root_record;

        // On-disk layout:
        //
        //   sectors 0..16   system area (zeroed)
        //   16              PVD
        //   17              VD Set Terminator
        //   ...             Path Table L, then Path Table M
        //   ...             every directory extent, in BFS order
        //   ...             every file's data, in BFS order

        // system area: sectors 0–15 are skipped (left zeroed by the writer)
        w.seek_relative(16 * SECTOR_SIZE as i64)?;

        // sector 16: Primary Volume Descriptor
        write_descriptor(&mut w, 1, &pvd)?;

        // sector 17: VD Set Terminator
        write_vd_terminator(&mut w)?;

        // Path Table L (little-endian)
        w.seek(SeekFrom::Start(
            self.path_table_l_lba as u64 * SECTOR_SIZE as u64,
        ))?;
        write_path_table::<LittleEndian>(&mut w, &self.dirs)?;
        pad_to_sector(&mut w)?;

        // Path Table M (big-endian)
        w.seek(SeekFrom::Start(
            self.path_table_m_lba as u64 * SECTOR_SIZE as u64,
        ))?;
        write_path_table::<BigEndian>(&mut w, &self.dirs)?;
        pad_to_sector(&mut w)?;

        // Directory extents, in BFS order (which is the order flatten appended them)
        for i in 0..self.dirs.len() {
            serialize_dir_extent(&mut w, &self.dirs[i], &self.dirs)?;
        }

        // File data, in BFS order. Consuming self.dirs lets us move FileSource out.
        for dir in self.dirs {
            for file in dir.files {
                if file.len == 0 {
                    continue; // zero-byte files occupy no sectors (LBA stays 0)
                }
                w.seek(SeekFrom::Start(file.lba as u64 * SECTOR_SIZE as u64))?;
                match file.source {
                    FileSource::InMemory(bytes) => w.write_all(bytes)?,
                    FileSource::OnDisk { mut reader, .. } => {
                        io::copy(&mut reader, &mut w)?;
                    }
                }
                pad_to_sector(&mut w)?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Writes a `VolumeDescriptorHeader` (type byte + "CD001" + version) followed
/// by the descriptor body. Together they must fill exactly one sector; the
/// const assertions on each descriptor type in `raw.rs` guarantee this.
fn write_descriptor<T: IntoBytes + zerocopy::Immutable>(
    w: &mut impl io::Write,
    ty: u8,
    body: &T,
) -> io::Result<()> {
    let header = VolumeDescriptorHeader {
        volume_descriptor_ty: ty,
        standard_id: *b"CD001",
        volume_descriptor_version: 1,
    };
    header.write_to_io(&mut *w)?;
    body.write_to_io(&mut *w)
}

/// Writes a Volume Descriptor Set Terminator (type 255): a header followed by
/// zeros to fill the rest of the sector.
fn write_vd_terminator(w: &mut impl io::Write) -> io::Result<()> {
    let header = VolumeDescriptorHeader {
        volume_descriptor_ty: 255,
        standard_id: *b"CD001",
        volume_descriptor_version: 1,
    };
    header.write_to_io(&mut *w)?;
    static ZEROS: [u8; SECTOR_SIZE] = [0u8; SECTOR_SIZE];
    w.write_all(&ZEROS[..SECTOR_SIZE - size_of::<VolumeDescriptorHeader>()])
}

/// Zero-fills the writer up to the next sector boundary.
fn pad_to_sector(w: &mut (impl io::Write + io::Seek)) -> io::Result<()> {
    let pos = w.stream_position()? as usize;
    let rem = pos % SECTOR_SIZE;
    if rem != 0 {
        static ZEROS: [u8; SECTOR_SIZE] = [0u8; SECTOR_SIZE];
        w.write_all(&ZEROS[..SECTOR_SIZE - rem])?;
    }
    Ok(())
}

/// Builds a `DirectoryRecordHeader` for a file or subdirectory record.
///
/// `id_len` is the byte length of the file identifier that immediately follows
/// the header on disk. The record length (`len` field) is computed to always
/// be even, matching the calculation in `DirNode::required_extent_size`.
fn build_dir_record_header(
    extent_lba: u32,
    data_length: u32,
    is_dir: bool,
    id_len: u8,
) -> DirectoryRecordHeader {
    // DirectoryRecordHeader is 33 bytes (odd). A 1-byte pad is added when
    // id_len is even so the total record length is always even.
    let pad: u8 = 1 - (id_len & 1);
    let len = size_of::<DirectoryRecordHeader>() as u8 + id_len + pad;

    DirectoryRecordHeader {
        len,
        extended_attribute_record_len: 0,
        extent_lba: BothEndianU32::new(extent_lba),
        data_length: BothEndianU32::new(data_length),
        recording_date: DirDateTime {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            timezone_offset: 0,
        },
        flags: if is_dir {
            FileFlags::DIRECTORY.bits()
        } else {
            0
        },
        interleaved_file_unit_size: 0,
        interleaved_gap_size: 0,
        volume_sequence_number: BothEndianU16::new(1),
        file_identifier_len: id_len,
    }
}

/// Writes a single path table (L or M) for all directories in BFS order.
///
/// Caller is responsible for seeking to the correct LBA and for calling
/// `pad_to_sector` afterwards.
fn write_path_table<O>(w: &mut impl io::Write, dirs: &[DirNode<'_>]) -> io::Result<()>
where
    O: ByteOrder,
    PathTableRecordHeader<O>: IntoBytes + zerocopy::Immutable,
{
    for dir in dirs {
        let id_bytes = dir.identifier.as_bytes();
        let id_len = id_bytes.len() as u8;

        // ECMA-119 1-based parent index: our in-memory indices are 0-based,
        // so root's parent wire value becomes 0 + 1 = 1 (pointing at itself).
        let header = PathTableRecordHeader::<O> {
            len: id_len,
            extended_attribute_record_len: 0,
            extent_lba: U32::<O>::new(dir.extent_lba),
            parent_directory: U16::<O>::new(dir.parent_directory + 1),
        };
        header.write_to_io(&mut *w)?;
        w.write_all(id_bytes)?;
        if id_len % 2 != 0 {
            w.write_all(&[0u8])?; // pad to even record length
        }
    }
    Ok(())
}

/// Writes a single directory record into an already-open writer positioned
/// anywhere within the directory extent.
///
/// `offset_in_sector` tracks the byte offset within the current sector so
/// that sector-boundary straddling can be detected and prevented with padding.
fn write_dir_record(
    w: &mut impl io::Write,
    offset_in_sector: &mut u32,
    id: &[u8],
    extent_lba: u32,
    data_length: u32,
    is_dir: bool,
) -> io::Result<()> {
    let id_len = id.len() as u8;
    let pad: u8 = 1 - (id_len & 1);
    let record_size = size_of::<DirectoryRecordHeader>() as u32 + id_len as u32 + pad as u32;

    // ECMA-119 §9.1: no directory record may straddle a sector boundary.
    if *offset_in_sector + record_size > SECTOR_SIZE as u32 {
        static ZEROS: [u8; SECTOR_SIZE] = [0u8; SECTOR_SIZE];
        let pad_len = (SECTOR_SIZE as u32 - *offset_in_sector) as usize;
        w.write_all(&ZEROS[..pad_len])?;
        *offset_in_sector = 0;
    }

    let header = build_dir_record_header(extent_lba, data_length, is_dir, id_len);
    header.write_to_io(&mut *w)?;
    w.write_all(id)?;
    if pad > 0 {
        w.write_all(&[0u8])?;
    }
    *offset_in_sector = (*offset_in_sector + record_size) % SECTOR_SIZE as u32;
    Ok(())
}

/// Writes the full directory extent for `dir` at its pre-assigned LBA.
///
/// The extent contains:
///  - `.`  record (this directory)
///  - `..` record (parent, or self for root)
///  - one record per entry in `sorted_entry_ids` (subdirs and files interleaved)
///
/// Records that would straddle a sector boundary are preceded by zero padding
/// to push them to the start of the next sector, mirroring `required_extent_size`.
fn serialize_dir_extent(
    w: &mut (impl io::Write + io::Seek),
    dir: &DirNode<'_>,
    dirs: &[DirNode<'_>],
) -> io::Result<()> {
    w.seek(SeekFrom::Start(dir.extent_lba as u64 * SECTOR_SIZE as u64))?;

    let mut offset: u32 = 0;

    // "." — this directory
    write_dir_record(
        &mut *w,
        &mut offset,
        &[0x00],
        dir.extent_lba,
        dir.extent_len,
        true,
    )?;

    // ".." — parent (root's parent is itself)
    let parent = &dirs[dir.parent_directory as usize];
    write_dir_record(
        &mut *w,
        &mut offset,
        &[0x01],
        parent.extent_lba,
        parent.extent_len,
        true,
    )?;

    // All entries (subdirs and files) in the pre-sorted order.
    for (_, entry) in &dir.sorted_entry_ids {
        match entry {
            DirNodeEntry::ChildDirectory(idx) => {
                let child = &dirs[*idx as usize];

                write_dir_record(
                    &mut *w,
                    &mut offset,
                    child.identifier.as_bytes(),
                    child.extent_lba,
                    child.extent_len,
                    true,
                )?;
            }
            DirNodeEntry::File(idx) => {
                let file = &dir.files[*idx as usize];

                write_dir_record(
                    &mut *w,
                    &mut offset,
                    file.identifier.as_bytes(),
                    file.lba,
                    file.len as u32,
                    false,
                )?;
            }
        }
    }

    pad_to_sector(w)
}
