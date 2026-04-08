# ISO 9660 (ECMA-119) Disk Image Library

Builder and parser for ISO 9660 / ECMA-119 disk images with El Torito boot support.

## Design Decisions (from discussion)

- **Builder target**: `Write + Seek`, avoid reading files from disk just to copy them (pass file metadata + reader/offset, let builder stream directly)
- **Parser input**: `&[u8]` zero-copy slices, `no_std` compatible
- **Extensions**: Rock Ridge / Joliet deferred to later
- **File system level**: Level 1 (8.3 filenames, max 8 directory depth)
- **Error handling**: Hand-written error enums, no `thiserror`
- **Unsafe policy**: Prefer safe code; allow unsafe only where it measurably improves performance
- **Endian validation**: Strict — always validate LE and BE copies match
- **Security posture**: Maximally strict by default, reject malformed input

## Validation

Two Debian ISO images in `tests/` as parsing fixtures. Use `iso-info` and `iso-read` (libcdio) to validate builder output.

---

## ECMA-119 Reference Summary

### Disk Layout (2048-byte logical sectors)

```
Sector 0-15:   System Area (unused by ISO 9660, reserved for boot loaders)
Sector 16+:    Volume Descriptor Set (one descriptor per sector)
               - Primary Volume Descriptor (type 1) — exactly one required
               - Boot Record (type 0) — El Torito places one here
               - Volume Descriptor Set Terminator (type 255) — exactly one, ends the set
After VDS:     Path Table(s), Directory Records, File Data (order flexible)
```

### Both-Endian Integers

All multi-byte integers are stored as both little-endian and big-endian copies, concatenated:
- `u16_both`: 4 bytes (2 LE + 2 BE)
- `u32_both`: 8 bytes (4 LE + 4 BE)

Parser MUST validate LE == BE and reject on mismatch.

### Date/Time Formats

**DecDatetime (17 bytes)** — used in Volume Descriptors:
- 4 digits year, 2 month, 2 day, 2 hour, 2 minute, 2 second, 2 centiseconds
- All ASCII digits
- 1 byte GMT offset in 15-minute intervals (signed i8)

**DirDatetime (7 bytes)** — used in Directory Records:
- 1 byte: years since 1900
- 1 byte each: month (1-12), day (1-31), hour (0-23), minute (0-59), second (0-59)
- 1 byte: GMT offset in 15-minute intervals

### Character Sets

- **d-characters**: `A-Z 0-9 _` (used for file identifiers)
- **a-characters**: d-characters plus space and `! " % & ' ( ) * + , - . / : ; < = > ?`
- Level 1 filenames: `FILENAME.EXT;VERSION` — name max 8, ext max 3, version `1`

### Primary Volume Descriptor (2048 bytes, type 1)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Type (1) |
| 1 | 5 | Standard Identifier "CD001" |
| 6 | 1 | Version (1) |
| 7 | 1 | Unused |
| 8 | 32 | System Identifier (a-chars, padded with spaces) |
| 40 | 32 | Volume Identifier (d-chars, padded with spaces) |
| 72 | 8 | Unused |
| 80 | 8 | Volume Space Size (u32_both) — total sectors |
| 88 | 32 | Unused |
| 120 | 4 | Volume Set Size (u16_both, must be 1) |
| 124 | 4 | Volume Sequence Number (u16_both, must be 1) |
| 128 | 4 | Logical Block Size (u16_both, must be 2048) |
| 132 | 8 | Path Table Size (u32_both) — bytes |
| 140 | 4 | Type L Path Table LBA (u32 LE) |
| 144 | 4 | Optional Type L Path Table LBA (u32 LE, 0 if absent) |
| 148 | 4 | Type M Path Table LBA (u32 BE) |
| 152 | 4 | Optional Type M Path Table LBA (u32 BE, 0 if absent) |
| 156 | 34 | Root Directory Record |
| 190 | 128 | Volume Set Identifier |
| 318 | 128 | Publisher Identifier |
| 446 | 128 | Data Preparer Identifier |
| 574 | 128 | Application Identifier |
| 702 | 37 | Copyright File Identifier |
| 739 | 37 | Abstract File Identifier |
| 776 | 37 | Bibliographic File Identifier |
| 813 | 17 | Volume Creation Date (DecDatetime) |
| 830 | 17 | Volume Modification Date (DecDatetime) |
| 847 | 17 | Volume Expiration Date (DecDatetime) |
| 864 | 17 | Volume Effective Date (DecDatetime) |
| 881 | 1 | File Structure Version (1) |
| 882 | 1 | Unused |
| 883 | 512 | Application Use |
| 1395 | 653 | Reserved |

### Directory Record (variable length, 33+ bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Length of Directory Record |
| 1 | 1 | Extended Attribute Record Length |
| 2 | 8 | LBA of Extent (u32_both) |
| 10 | 8 | Data Length (u32_both) |
| 18 | 7 | Recording Date (DirDatetime) |
| 25 | 1 | File Flags (bit 0: hidden, bit 1: directory, bit 2: associated, bit 3: record format, bit 4: permissions, bit 7: not final record) |
| 26 | 1 | File Unit Size (0 for non-interleaved) |
| 27 | 1 | Interleave Gap Size (0 for non-interleaved) |
| 28 | 4 | Volume Sequence Number (u16_both) |
| 32 | 1 | File Identifier Length (N) |
| 33 | N | File Identifier |
| 33+N | pad | Padding byte if N is even (to align to even offset) |

Special identifiers: `\x00` = `.` (self), `\x01` = `..` (parent)

### Path Table Record (variable length)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Length of Directory Identifier (N) |
| 1 | 1 | Extended Attribute Record Length |
| 2 | 4 | LBA of Extent (u32 — LE in Type L, BE in Type M) |
| 6 | 2 | Parent Directory Number (u16 — LE in Type L, BE in Type M) |
| 8 | N | Directory Identifier |
| 8+N | pad | Padding byte if N is odd |

### Boot Record Volume Descriptor (type 0, 2048 bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Type (0) |
| 1 | 5 | Standard Identifier "CD001" |
| 6 | 1 | Version (1) |
| 7 | 32 | Boot System Identifier ("EL TORITO SPECIFICATION" padded with \x00) |
| 39 | 32 | Unused (zeros) |
| 71 | 4 | Absolute LBA of Boot Catalog (u32 LE) |
| 75 | 1973 | Unused (zeros) |

## El Torito Specification

### Boot Catalog Structure

The boot catalog is a sequence of 32-byte entries at the LBA specified in the Boot Record VD.

**Validation Entry (first entry, 32 bytes):**

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Header ID (1) |
| 1 | 1 | Platform ID (0=x86 BIOS, 1=PPC, 2=Mac, 0xEF=EFI) |
| 2 | 2 | Reserved (0) |
| 4 | 24 | ID String (manufacturer/developer) |
| 28 | 2 | Checksum (all 16-bit LE words in entry must sum to 0) |
| 30 | 1 | Key byte 0x55 |
| 31 | 1 | Key byte 0xAA |

**Initial/Default Entry (second entry, 32 bytes):**

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Boot Indicator (0x88 = bootable, 0x00 = not bootable) |
| 1 | 1 | Boot Media Type (0=no emulation, 1=1.2M floppy, 2=1.44M floppy, 3=2.88M floppy, 4=hard disk) |
| 2 | 2 | Load Segment (0 = use default 0x7C0) |
| 4 | 1 | System Type |
| 5 | 1 | Unused (0) |
| 6 | 2 | Sector Count (number of 512-byte virtual sectors to load) |
| 8 | 4 | Load RBA (LBA of boot image, absolute sector number) |
| 12 | 20 | Unused (0) |

**Section Header (32 bytes):**

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Header Indicator (0x90 = more sections follow, 0x91 = last section) |
| 1 | 1 | Platform ID |
| 2 | 2 | Number of Section Entries following |
| 4 | 28 | ID String |

**Section Entry (32 bytes):** Same layout as Initial/Default Entry.

### Typical Boot Layout (BIOS + EFI)

```
Boot Catalog:
  [Validation Entry]     platform=x86 BIOS
  [Initial Entry]        no-emulation, points to BIOS boot image
  [Section Header]       platform=EFI, 1 section entry follows
  [Section Entry]        no-emulation, points to EFI boot image
```

---

## Implementation Plan

### Module Structure

```
src/
  lib.rs              — public API re-exports, #![no_std]
  error.rs            — error enums for parsing and building
  types.rs            — both-endian integers, date/time types, character set validation
  volume.rs           — volume descriptor types (Primary, Boot Record, Terminator)
  directory.rs        — directory record and path table types
  eltorito.rs         — El Torito boot catalog entries
  parse.rs            — zero-copy parser from &[u8]
  build.rs            — builder API (requires std)
```

### Phase 1: Primitive Types (`types.rs`)

Foundational types that everything else depends on.

```rust
/// Both-endian u16: 4 bytes (LE then BE). Validates LE == BE on parse.
pub struct U16Both { value: u16 }

/// Both-endian u32: 8 bytes (LE then BE). Validates LE == BE on parse.
pub struct U32Both { value: u32 }

/// 17-byte decimal date/time (Volume Descriptors).
pub struct DecDatetime { ... }

/// 7-byte directory record date/time.
pub struct DirDatetime { ... }

// Character set validation
pub fn validate_d_chars(s: &[u8]) -> Result<(), ParseError>
pub fn validate_a_chars(s: &[u8]) -> Result<(), ParseError>

// Padded string types for fixed-length fields
pub struct AString<const N: usize> { ... }  // a-character string
pub struct DString<const N: usize> { ... }  // d-character string
```

Each both-endian type provides:
- `parse(bytes: &[u8]) -> Result<Self, ParseError>` — validates LE==BE
- `to_bytes(&self) -> [u8; N]` — writes both copies
- `get(&self) -> u16/u32` — returns the value

### Phase 2: Volume Descriptors (`volume.rs`)

```rust
pub struct PrimaryVolumeDescriptor<'a> {
    pub system_id: &'a [u8; 32],
    pub volume_id: &'a [u8; 32],
    pub volume_space_size: U32Both,
    pub logical_block_size: U16Both,
    pub path_table_size: U32Both,
    pub path_table_l_lba: u32,
    pub path_table_m_lba: u32,
    pub root_directory_record: DirectoryRecord<'a>,
    pub volume_set_id: &'a [u8; 128],
    pub publisher_id: &'a [u8; 128],
    pub data_preparer_id: &'a [u8; 128],
    pub application_id: &'a [u8; 128],
    pub creation_date: DecDatetime,
    pub modification_date: DecDatetime,
    pub expiration_date: DecDatetime,
    pub effective_date: DecDatetime,
    // ...
}

pub struct BootRecordDescriptor {
    pub boot_system_id: [u8; 32],     // must be "EL TORITO SPECIFICATION\0..."
    pub boot_catalog_lba: u32,        // LE only (El Torito spec)
}

pub struct VolumeDescriptorSetTerminator; // just type=255 + "CD001" + version=1

pub enum VolumeDescriptor<'a> {
    Primary(PrimaryVolumeDescriptor<'a>),
    BootRecord(BootRecordDescriptor),
    Terminator,
}
```

Validation on parse:
- Magic "CD001" check
- Version == 1
- Logical block size == 2048
- Volume set size == 1, sequence number == 1
- All character set constraints on string fields
- All both-endian field consistency
- Reserved/unused fields are zero

### Phase 3: Directory Records & Path Tables (`directory.rs`)

```rust
pub struct DirectoryRecord<'a> {
    pub extent_lba: U32Both,
    pub data_length: U32Both,
    pub recording_date: DirDatetime,
    pub file_flags: FileFlags,
    pub volume_sequence_number: U16Both,
    pub file_identifier: &'a [u8],
}

bitflags-style FileFlags:
  HIDDEN      = 0x01
  DIRECTORY   = 0x02
  ASSOCIATED  = 0x04
  RECORD      = 0x08
  PROTECTION  = 0x10
  NOT_FINAL   = 0x80

pub struct PathTableRecord<'a> {
    pub extent_lba: u32,
    pub parent_directory_number: u16,
    pub directory_identifier: &'a [u8],
}
```

Validation:
- Record length consistent with identifier length
- File identifier is valid d-characters (or \x00/\x01 for . and ..)
- Level 1: filename matches `^[A-Z0-9_]{1,8}(\.[A-Z0-9_]{1,3})?;1$` for files
- Interleave fields must be 0 (we don't support interleaving)
- Extent LBA + data length must not exceed volume space size

### Phase 4: El Torito Boot Catalog (`eltorito.rs`)

```rust
pub enum BootPlatform {
    X86Bios,    // 0
    PowerPC,    // 1
    Mac,        // 2
    Efi,        // 0xEF
}

pub enum BootMediaType {
    NoEmulation,      // 0
    Floppy12,         // 1
    Floppy144,        // 2
    Floppy288,        // 3
    HardDisk,         // 4
}

pub struct ValidationEntry {
    pub platform: BootPlatform,
    pub id_string: [u8; 24],
    // checksum validated on parse, computed on build
}

pub struct BootEntry {
    pub bootable: bool,
    pub media_type: BootMediaType,
    pub load_segment: u16,
    pub system_type: u8,
    pub sector_count: u16,
    pub load_rba: u32,
}

pub struct SectionHeader {
    pub is_last: bool,
    pub platform: BootPlatform,
    pub entry_count: u16,
    pub id_string: [u8; 28],
}

pub struct BootCatalog<'a> {
    pub validation: ValidationEntry,
    pub default_entry: BootEntry,
    pub sections: Vec<(SectionHeader, Vec<BootEntry>)>,  // builder only
}
```

Validation:
- Validation entry checksum must sum to 0 across all 16-bit LE words
- Key bytes must be 0x55, 0xAA
- Header ID must be 1
- Section entry counts must match actual entries before next header/end
- Boot indicator must be 0x00 or 0x88

### Phase 5: Zero-Copy Parser (`parse.rs`)

```rust
/// Top-level parser. Borrows the entire image as &[u8].
pub struct Iso9660Image<'a> {
    data: &'a [u8],
    pvd: PrimaryVolumeDescriptor<'a>,
    boot_record: Option<BootRecordDescriptor>,
}

impl<'a> Iso9660Image<'a> {
    /// Parse an ISO image from a byte slice.
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError> { ... }

    /// Get the primary volume descriptor.
    pub fn primary_volume_descriptor(&self) -> &PrimaryVolumeDescriptor<'a> { ... }

    /// Iterate directory entries at a given directory record's extent.
    pub fn read_dir(&self, dir: &DirectoryRecord<'a>) -> Result<DirEntryIter<'a>, ParseError> { ... }

    /// Read the root directory.
    pub fn root_dir(&self) -> Result<DirEntryIter<'a>, ParseError> { ... }

    /// Read file contents (returns slice into the image).
    pub fn file_contents(&self, record: &DirectoryRecord<'a>) -> Result<&'a [u8], ParseError> { ... }

    /// Parse the El Torito boot catalog, if present.
    pub fn boot_catalog(&self) -> Result<Option<ParsedBootCatalog<'a>>, ParseError> { ... }

    /// Parse path tables (both L and M) and validate they match.
    pub fn path_table(&self) -> Result<Vec<PathTableRecord<'a>>, ParseError> { ... }
}

/// Iterator over directory entries within a directory extent.
pub struct DirEntryIter<'a> { ... }
```

Parse strategy:
1. Validate image is >= 17 sectors (system area + at least 1 VD)
2. Walk volume descriptors at sector 16+ until terminator
3. Extract and validate PVD (exactly one required)
4. Optionally extract boot record
5. Lazy parsing of directories / path tables / boot catalog on access

Bounds checking on every access — all slice indexing goes through helper functions that return `ParseError::UnexpectedEof` rather than panicking.

### Phase 6: Builder API (`build.rs`)

```rust
/// A file to include in the image. Avoids reading file data eagerly.
pub struct FileSource {
    pub name: FileName,            // validated 8.3 name
    pub source: FileData,
}

pub enum FileData {
    /// Data already in memory.
    Bytes(Vec<u8>),
    /// Read from an external source at build time.
    /// The closure receives a &mut dyn Write to stream into.
    External {
        len: u64,
        writer: Box<dyn FnOnce(&mut dyn Write) -> io::Result<()>>,
    },
}

/// Validated Level 1 file name (NAME.EXT;1).
pub struct FileName { ... }

pub struct DirBuilder {
    name: FileName,
    entries: Vec<Entry>,
}

enum Entry {
    File(FileSource),
    Dir(DirBuilder),
}

pub struct IsoBuilder {
    system_id: [u8; 32],
    volume_id: [u8; 32],
    publisher_id: [u8; 128],
    // ... other PVD string fields
    root: DirBuilder,
    boot_config: Option<BootConfig>,
}

pub struct BootConfig {
    pub default_entry: BootEntryConfig,
    pub sections: Vec<BootSectionConfig>,
}

pub struct BootEntryConfig {
    pub platform: BootPlatform,
    pub media_type: BootMediaType,
    pub load_segment: u16,
    pub sector_count: u16,
    pub boot_image: FileData,       // the actual boot image payload
}

pub struct BootSectionConfig {
    pub platform: BootPlatform,
    pub id_string: [u8; 28],
    pub entries: Vec<BootEntryConfig>,
}

impl IsoBuilder {
    pub fn new(volume_id: &str) -> Result<Self, BuildError> { ... }

    pub fn set_system_id(&mut self, id: &str) -> Result<&mut Self, BuildError> { ... }
    // ... other PVD fields

    pub fn root(&mut self) -> &mut DirBuilder { ... }

    pub fn set_boot(&mut self, config: BootConfig) -> &mut Self { ... }

    /// Build the ISO image, writing to the given target.
    /// Two-pass: first pass computes layout (LBAs), second pass writes.
    pub fn build<W: Write + Seek>(self, writer: &mut W) -> Result<(), BuildError> { ... }
}

impl DirBuilder {
    pub fn add_file(&mut self, source: FileSource) -> Result<&mut Self, BuildError> { ... }
    pub fn add_dir(&mut self, name: &str) -> Result<&mut DirBuilder, BuildError> { ... }
}
```

Build strategy (two-pass):
1. **Layout pass**: Walk the tree, assign sector numbers (LBAs) to:
   - System area (sectors 0-15, zeros)
   - PVD (sector 16)
   - Boot Record VD (sector 17, if boot)
   - VD Set Terminator (sector 17 or 18)
   - Boot Catalog (if boot)
   - Path Table Type L
   - Path Table Type M
   - Directory extents (root first, then depth-first)
   - File data extents
   - Boot image data extents
2. **Write pass**: Seek to each position and write. For `FileData::External`, invoke the closure directly with the writer positioned at the right offset — the file data streams straight to the output without an intermediate buffer.

Level 1 enforcement in builder:
- Reject filenames that don't match 8.3 pattern
- Reject directory depth > 8
- Reject any non-d-character in identifiers
- Validate total image size fits in u32 sectors

### Phase 7: Testing

**Parser tests (using test fixtures):**
1. Parse both Debian ISOs successfully
2. Validate PVD fields against `iso-info` output
3. Walk full directory tree, compare against `iso-read` listing
4. Parse El Torito boot catalogs, verify platform IDs and boot image LBAs
5. Validate all both-endian fields match
6. Fuzz-style edge cases: truncated images, corrupt magic, mismatched endian values

**Builder tests:**
1. Build minimal ISO (empty root dir), parse it back, verify round-trip
2. Build ISO with files, verify contents via parser and `iso-info`
3. Build bootable ISO (BIOS), verify with `iso-info -d`
4. Build bootable ISO (BIOS + EFI dual boot), verify catalog structure
5. Validate `iso-read` can extract files from builder output
6. Reject invalid filenames, excessive depth, invalid characters

**Round-trip tests:**
1. Parse fixture ISO -> extract structure -> rebuild -> parse again -> compare

### Implementation Order

| Step | Module | Depends On | Deliverable |
|------|--------|------------|-------------|
| 1 | `error.rs` | — | `ParseError`, `BuildError` enums |
| 2 | `types.rs` | `error.rs` | Both-endian ints, dates, char validation |
| 3 | `volume.rs` | `types.rs` | VD parsing + serialization |
| 4 | `directory.rs` | `types.rs` | Directory record + path table parsing + serialization |
| 5 | `eltorito.rs` | `types.rs` | Boot catalog parsing + serialization |
| 6 | `parse.rs` | all above | Full image parser, test against Debian ISOs |
| 7 | `build.rs` | all above | Builder, test via `iso-info` + round-trip |

Each step is independently testable. Steps 1-5 are `no_std` compatible. Step 7 requires `std` (gated behind a feature or cfg).
