## Implementing a Secure ISO 9660 / ECMA-119 Disk Image Library in Rust

> **Prerequisites**: Systems programming experience, familiarity with Rust lifetimes and
> generics, basic understanding of file system concepts.
>
> **Deliverable**: A correct, secure, `no_std`-compatible ISO 9660 parser and builder in Rust.
> Your implementation will be tested against real Debian DVD images and validated with
> the `libcdio` reference tools.

---

## 1. Background and Motivation

ISO 9660 (standardized as ECMA-119) is the file system format used on CD-ROM and DVD media.
Despite being a 1980s standard, it remains relevant wherever disk images are exchanged: OS
installers, firmware update packages, VM templates, and — directly relevant to this course —
bootable RISC-V kernel images. If you have ever burned a Linux ISO to a USB drive, you have
interacted with this format.

Your goal is to implement a library that can both *parse* existing ISO images and *build* new
ones. The parser must be zero-copy and `no_std`-compatible (it will run inside an OS kernel).
The builder requires `std` and must produce images verifiable by third-party tools.

**Why is security a concern here?** ISO images are a common attack surface: a malicious image
fed to a parser inside a privileged kernel context can exploit parser bugs to corrupt memory or
hijack control flow. Your parser must treat every byte of input as potentially adversarial.

---

## 2. Authoritative References

Keep these open throughout the assignment. Implementation choices must be traceable to one of
these sources.

| Document | Purpose |
|----------|---------|
| [ECMA-119 4th edition (2019)](https://www.ecma-international.org/publications-and-standards/standards/ecma-119/) | The normative standard. Free PDF. §8.4, §9.1, §6.9 are your most-used sections. |
| [El Torito Specification](https://pdos.csail.mit.edu/6.828/2018/readings/boot-cdrom.pdf) | 25-page industry spec for bootable ISO extensions. Read it in full. |
| [OSDev Wiki: ISO 9660](https://wiki.osdev.org/ISO_9660) | Practical byte-offset tables. Excellent for cross-checking struct layouts. |
| [OSDev Wiki: El Torito](https://wiki.osdev.org/El-Torito) | Worked examples of boot catalog byte layouts. |

**Reference implementations** — study these before writing a line of code:

| Project | Language | What to learn |
|---------|----------|---------------|
| [libcdio](https://www.gnu.org/software/libcdio/) | C | `lib/iso9660/iso9660.h` — authoritative struct layouts. Reference for validation tools (`iso-info`, `iso-read`). |
| [pycdlib internals](https://clalancette.github.io/pycdlib/pycdlib-internals.html) | Python | Best-documented pure-software implementation. Read the "Writing" section for the two-pass layout algorithm. |
| [cdfs](https://github.com/nicholasbishop/cdfs) | Rust | Clean zero-copy `&[u8]` parsing. Parser only — no builder — but the approach is idiomatic. |

---

## 3. Conceptual Overview

Before touching byte offsets, you must understand three structural ideas that govern the entire
format. Misunderstanding any one of them will cause you to waste days chasing bugs.

### 3.1 The Disk as a Sequence of 2048-byte Sectors

ISO 9660 divides the disk into fixed 2048-byte *logical blocks* (also called sectors). Every
structure in the format is addressed by its *Logical Block Address* (LBA) — an integer index
from the beginning of the image. The byte offset of any LBA is simply:

```
byte_offset = lba × 2048
```

This uniformity is your friend. All pointer arithmetic in your implementation reduces to this
single formula.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        ISO 9660 Image Layout                                │
├──────────────┬──────────────┬──────────────┬──────────────┬─────────────────┤
│  Sectors     │  Sectors     │  Sectors     │  Sectors     │  Sectors        │
│   0 – 15     │   16 – N     │   N+1 – M    │   M+1 – P    │   P+1 – end     │
│              │              │              │              │                 │
│ System Area  │  Volume      │  Path        │  Directory   │  File Data      │
│ (16 × 2048   │  Descriptor  │  Tables      │  Extents     │  Extents        │
│  bytes of    │  Set         │  (Type L     │  (root       │                 │
│  zeros)      │  (PVD, Boot  │  and Type M) │  first, then │                 │
│              │  Record, VD  │              │  children)   │                 │
│              │  Terminator) │              │              │                 │
└──────────────┴──────────────┴──────────────┴──────────────┴─────────────────┘
         ▲
         │
  Sector 16 is always where
  the Volume Descriptor Set begins.
  Sectors 0-15 are always zero.
```

### 3.2 The Volume Descriptor Set — Your Entry Point

The Volume Descriptor Set (VDS) begins at sector 16 and continues until a *terminator*
descriptor is encountered. Each descriptor is exactly 2048 bytes and begins with:

- **Byte 0**: type code (`0` = Boot Record, `1` = Primary, `255` = Terminator)
- **Bytes 1–5**: the magic string `"CD001"` — reject anything else immediately
- **Byte 6**: version (`1`) — reject anything else

You walk the VDS linearly, sector by sector, collecting descriptors until you hit type 255.
The **Primary Volume Descriptor** (PVD, type 1) must appear exactly once. Everything in the
filesystem hangs off the PVD:

```
                    ┌──────────────────────────────────────┐
                    │       Primary Volume Descriptor       │
                    │  (sector 16, 2048 bytes)              │
                    │                                       │
                    │  volume_space_size ────────────────── │ → total sector count
                    │  logical_block_size (must be 2048) ── │
                    │  path_table_l_lba ─────────────────── │ → Type L Path Table sector
                    │  path_table_m_lba ─────────────────── │ → Type M Path Table sector
                    │  path_table_size ──────────────────── │ → size in bytes
                    │  root_directory_record ─────────────  │ → 34 bytes embedded here
                    └──────────────────┬───────────────────┘
                                       │
                     root_directory_record.extent_lba
                                       │
                                       ▼
                    ┌──────────────────────────────────────┐
                    │           Root Directory Extent       │
                    │  (variable sectors)                   │
                    │                                       │
                    │  [. record]  [.. record]              │
                    │  [BOOT/ dir record]                   │
                    │  [README.TXT;1 file record]           │
                    │  ...                                  │
                    └──────────────────────────────────────┘
```

### 3.3 Two Redundant Directory Structures

ISO 9660 represents the directory tree in *two* independent, redundant structures that must
describe the same tree. Both must be written; a secure parser validates they agree.

```
                            The Same Tree, Twice:

   Directory Records (hierarchical)       Path Table (flat list)
   ─────────────────────────────────      ──────────────────────────────
   Root/                                  #1  Root         parent=#1
   ├── BOOT/                              #2  BOOT         parent=#1
   │   ├── EFI.IMG;1                      #3  GRUB         parent=#2
   │   └── GRUB/                          #4  DOCS         parent=#1
   └── DOCS/

   Traversed via LBA pointers             Traversed linearly
   in directory records.                  (sorted: parents before children,
   Stores both files and dirs.            siblings lexicographic).
   Variable-length records.               Stores directories only.
   Packed in sector extents.             Stored in two copies (LE and BE).
```

**Why two structures?** The path table was originally an optimization for slow CD-ROM drives
that allowed random access to directory entries without traversing the full hierarchy. Today it
is mandatory for spec compliance even though it offers no performance benefit.

**Implication for your implementation**: when building, you must write both. When parsing with
security in mind, validate that the path table and directory records agree on directory LBAs.

### 3.4 Both-Endian Fields

Every multi-byte integer in ECMA-119 — except in El Torito — is stored *twice*: once in
little-endian, once in big-endian, back to back. A `u32` occupies 8 bytes in the format:

```
 Byte offset:  +0    +1    +2    +3    +4    +5    +6    +7
               ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
               │  LE  byte0│  LE  byte1│  LE  byte2│  LE  byte3│  ← little-endian copy
               ├─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┤
               │  BE  byte3│  BE  byte2│  BE  byte1│  BE  byte0│  ← big-endian copy (same value)
               └─────┴─────┴─────┴─────┴─────┴─────┴─────┴─────┘

   Parse: u32::from_le_bytes([b0,b1,b2,b3])  and  u32::from_be_bytes([b4,b5,b6,b7])
   Must be equal — reject if they differ.
```

This was a 1980s portability mechanism. Today it is your first security gate: any discrepancy
between the LE and BE copies is a sign of a malformed or malicious image.

---

## 4. Module Architecture

Your library is structured as seven modules with a strict dependency order. Lower-numbered
modules know nothing about higher-numbered ones.

```
                    ┌─────────────────────────────────────────────────────┐
                    │                     lib.rs                          │
                    │           Public API re-exports, #![no_std]         │
                    └────────────────────────┬────────────────────────────┘
                                             │ depends on all below
              ┌──────────────────────────────┼──────────────────────────────┐
              │                              │                              │
              ▼                              ▼                              ▼
     ┌─────────────────┐           ┌──────────────────┐           ┌──────────────────┐
     │    parse.rs     │           │    build.rs       │           │   eltorito.rs    │
     │                 │           │                   │           │                  │
     │  Iso9660Image   │           │  IsoBuilder       │           │  BootCatalog     │
     │  DirEntryIter   │           │  DirBuilder       │           │  ValidationEntry │
     │  zero-copy API  │           │  two-pass layout  │           │  BootEntry       │
     └────────┬────────┘           └────────┬──────────┘           └────────┬─────────┘
              │                             │                               │
              └──────────────┬──────────────┘                               │
                             │ all depend on:                               │
              ┌──────────────┼───────────────┬───────────────┐             │
              ▼              ▼               ▼               ▼             │
    ┌──────────────┐ ┌──────────────┐ ┌──────────────┐      │             │
    │  volume.rs   │ │ directory.rs │ │   types.rs   │      │             │
    │              │ │              │ │              │      └─────────────┘
    │  PrimaryVD   │ │ DirRecord    │ │ U16Both      │      (eltorito also
    │  BootRecord  │ │ PathTable    │ │ U32Both      │       uses types.rs)
    │  Terminator  │ │ DirEntryIter │ │ DecDatetime  │
    └──────────────┘ └──────────────┘ │ DirDatetime  │
                                      │ AString<N>   │
                                      │ DString<N>   │
                                      └──────┬───────┘
                                             │
                                             ▼
                                    ┌──────────────┐
                                    │   error.rs   │
                                    │              │
                                    │ ParseError   │
                                    │ BuildError   │
                                    └──────────────┘
```

`no_std` boundary: `error.rs`, `types.rs`, `volume.rs`, `directory.rs`, `eltorito.rs`, and
`parse.rs` must compile without `std`. Only `build.rs` requires `std` (for `Write + Seek` I/O).
Gate it with `#[cfg(feature = "builder")]` or a `std` feature flag.

---

## 5. Implementation Phases

Work through these phases in order. Each phase is independently testable before proceeding to
the next.

---

### Phase 1 — Primitive Types (`types.rs` and `error.rs`)

These are the atoms from which everything else is composed. Get them right before proceeding.

#### 5.1.1 Error Types (`error.rs`)

Define two error enums with rich context. Carry offsets and values — when staring at a
corrupted image you will be grateful for them.

```rust
// no_std — used by parser
pub enum ParseError {
    UnexpectedEof { offset: usize, needed: usize },
    InvalidMagic,
    InvalidVersion { found: u8 },
    BothEndianMismatch { offset: usize, le: u32, be: u32 },
    InvalidCharacter { offset: usize, byte: u8 },
    InvalidFieldValue { field: &'static str, value: u32 },
    NoPrimaryVolumeDescriptor,
    MultiplePrimaryVolumeDescriptors,
    ChecksumMismatch { expected: u16, found: u16 },
    // add more as you discover them
}

// std — used by builder
pub enum BuildError {
    Io(io::Error),
    InvalidFileName { name: String },
    DirectoryTooDeep { depth: u8 },
    ImageTooLarge { sectors: u64 },
    // add more as you discover them
}
```

#### 5.1.2 Both-Endian Integers (`types.rs`)

```rust
/// A u16 stored as 4 bytes: 2 LE bytes followed by 2 BE bytes.
/// Validates that both copies encode the same value on parse.
#[repr(transparent)]
pub struct U16Both { value: u16 }

/// A u32 stored as 8 bytes: 4 LE bytes followed by 4 BE bytes.
#[repr(transparent)]
pub struct U32Both { value: u32 }
```

Each type provides:
- `parse(bytes: &[u8]) -> Result<Self, ParseError>` — validates LE == BE
- `get(&self) -> u16/u32` — returns the value
- `to_bytes(&self) -> [u8; 4/8]` — serializes both copies

**Implementation hint**: never index `bytes` directly. Use:
```rust
let le = bytes.get(offset..offset + 4)
    .ok_or(ParseError::UnexpectedEof { offset, needed: 4 })?;
let le = u32::from_le_bytes(<[u8; 4]>::try_from(le).unwrap());
```
Every access is a potential panic if you use raw indexing. Bounds-check everything.

#### 5.1.3 Date/Time Types (`types.rs`)

Two distinct date/time formats are used in different contexts. Do **not** conflate them.

```
DecDatetime (17 bytes) — used in Volume Descriptors:

  Offset  Size  Content
  0       4     Year (e.g. "2024" as ASCII bytes 0x32 0x30 0x32 0x34)
  4       2     Month ("01"–"12")
  6       2     Day ("01"–"31")
  8       2     Hour ("00"–"23")
  10      2     Minute ("00"–"59")
  12      2     Second ("00"–"59")
  14      2     Centiseconds ("00"–"99")
  16      1     GMT offset in 15-minute intervals (signed i8, -48 to +52)

DirDatetime (7 bytes) — used in Directory Records:

  Offset  Size  Content
  0       1     Years since 1900 (e.g. 124 for 2024) — raw binary, NOT ASCII
  1       1     Month (1–12)
  2       1     Day (1–31)
  3       1     Hour (0–23)
  4       1     Minute (0–59)
  5       1     Second (0–59)
  6       1     GMT offset in 15-minute intervals (signed i8)
```

Validate each field is in the legal range. For `DecDatetime`, validate every byte is in
`b'0'..=b'9'` before converting to integers.

#### 5.1.4 Character Set Validation (`types.rs`)

ECMA-119 §7.4 defines two character sets:

- **d-characters**: `[A-Z0-9_]` — used for file and directory identifiers
- **a-characters**: d-characters plus `SPACE ! " % & ' ( ) * + , - . / : ; < = > ?`

Validate every string field on parse. This is where malformed images try to smuggle bad data.

```rust
pub fn validate_d_chars(s: &[u8]) -> Result<(), ParseError> { ... }
pub fn validate_a_chars(s: &[u8]) -> Result<(), ParseError> { ... }
```

Also implement padded fixed-length string types:

```rust
pub struct AString<const N: usize>([u8; N]);  // a-character, space-padded
pub struct DString<const N: usize>([u8; N]);  // d-character, space-padded
```

These hold exactly `N` bytes (no heap allocation — `no_std` compatible) and validate their
contents on construction.

---

### Phase 2 — Volume Descriptors (`volume.rs`)

**Goal**: parse and serialize all VD types needed.

```rust
pub struct PrimaryVolumeDescriptor<'a> {
    pub system_id:            &'a [u8; 32],   // a-characters, space-padded
    pub volume_id:            &'a [u8; 32],   // d-characters, space-padded
    pub volume_space_size:    U32Both,         // total number of sectors
    pub logical_block_size:   U16Both,         // must be 2048
    pub path_table_size:      U32Both,         // bytes
    pub path_table_l_lba:     u32,             // LE only
    pub path_table_m_lba:     u32,             // BE only
    pub root_directory_record: DirectoryRecord<'a>,
    pub volume_set_id:        &'a [u8; 128],
    pub publisher_id:         &'a [u8; 128],
    pub data_preparer_id:     &'a [u8; 128],
    pub application_id:       &'a [u8; 128],
    pub creation_date:        DecDatetime,
    pub modification_date:    DecDatetime,
    pub expiration_date:      DecDatetime,
    pub effective_date:       DecDatetime,
}

pub struct BootRecordDescriptor {
    pub boot_system_id:   [u8; 32],  // must be "EL TORITO SPECIFICATION\0..."
    pub boot_catalog_lba: u32,       // LE only — El Torito is LE-only
}

pub enum VolumeDescriptor<'a> {
    Primary(PrimaryVolumeDescriptor<'a>),
    BootRecord(BootRecordDescriptor),
    Terminator,
}
```

**Lifetimes and zero-copy**: notice that `PrimaryVolumeDescriptor<'a>` borrows slices directly
from the input `&'a [u8]`. This is intentional — no allocation, no copying. The tradeoff is
that the parsed struct cannot outlive the original byte slice. For your use case (parsing kernel
images in place), this is the right choice.

**Validation checklist** — reject on any violation:

| Check | Rejection reason |
|-------|-----------------|
| Bytes 1–5 != `"CD001"` | `InvalidMagic` |
| Byte 6 != `1` | `InvalidVersion` |
| `logical_block_size` != 2048 | `InvalidFieldValue` |
| `volume_set_size` != 1 | `InvalidFieldValue` |
| `volume_sequence_number` != 1 | `InvalidFieldValue` |
| Any both-endian field LE != BE | `BothEndianMismatch` |
| Any "unused" field != zero | `InvalidFieldValue` |
| String fields fail character set check | `InvalidCharacter` |

The check for nonzero "unused" bytes is a **security requirement**, not pedantry. Hidden data
in reserved fields is a real technique for embedding malicious payloads in ISO images.

**PVD byte layout** (partial — see plan.md for the full table):

```
Offset  Size   Field
0       1      Type code (1)
1       5      Standard Identifier "CD001"
6       1      Version (1)
7       1      Unused (must be 0)
8       32     System Identifier (a-chars)
40      32     Volume Identifier (d-chars)
72      8      Unused (must be zeros)
80      8      Volume Space Size (U32Both)
...
128     4      Logical Block Size (U16Both)
132     8      Path Table Size (U32Both)
140     4      Type L Path Table LBA (u32 LE)
144     4      Optional Type L Path Table LBA (u32 LE)
148     4      Type M Path Table LBA (u32 BE)
152     4      Optional Type M Path Table LBA (u32 BE)
156     34     Root Directory Record (embedded DirectoryRecord)
...
```

The root directory record at offset 156 is a complete `DirectoryRecord` embedded directly in
the PVD. Parse it as such. Its `file_identifier` is always `\x00` (the self-reference sentinel).

---

### Phase 3 — Directory Records and Path Tables (`directory.rs`)

This phase involves the trickiest parsing in the entire format.

#### 5.3.1 Directory Record

```rust
pub struct DirectoryRecord<'a> {
    pub extent_lba:              U32Both,
    pub data_length:             U32Both,
    pub recording_date:          DirDatetime,
    pub file_flags:              FileFlags,
    pub volume_sequence_number:  U16Both,
    pub file_identifier:         &'a [u8],
}

pub struct FileFlags(u8);
impl FileFlags {
    pub const HIDDEN:      u8 = 0x01;
    pub const DIRECTORY:   u8 = 0x02;
    pub const ASSOCIATED:  u8 = 0x04;
    pub const RECORD:      u8 = 0x08;
    pub const PROTECTION:  u8 = 0x10;
    pub const NOT_FINAL:   u8 = 0x80;
}
```

**Record layout** (minimum 33 bytes, variable length):

```
Offset  Size   Field
0       1      Length of Directory Record (L)  ← must be >= 33, even
1       1      Extended Attribute Record Length (must be 0)
2       8      LBA of Extent (U32Both)
10      8      Data Length (U32Both)
18      7      Recording Date (DirDatetime)
25      1      File Flags
26      1      File Unit Size (must be 0 — we don't support interleaving)
27      1      Interleave Gap Size (must be 0)
28      4      Volume Sequence Number (U16Both)
32      1      File Identifier Length (N)
33      N      File Identifier
33+N    0 or 1 Padding byte if N is even (so total record length is even)
```

**Validation requirements** (reject on any violation):
- `L >= 33` and `L` is even
- `file_identifier_length` is consistent with `L` (i.e., `33 + N + pad == L`)
- Extended attribute length is 0
- File unit size and interleave gap are both 0
- `extent_lba * 2048 + data_length <= image_total_bytes` (validated in parse.rs)

#### 5.3.2 Directory Entry Iterator

Records are packed sequentially within a directory extent. Your iterator must handle two cases:

```
  Directory Extent (one or more 2048-byte sectors):

  ┌────────────────────────────────────────────────────────────────────────────┐
  │  Sector N (bytes 0–2047)                                                   │
  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────────────────────────┐  │
  │  │ Record 1 │ │ Record 2 │ │ Record 3 │ │  \x00 \x00 ... \x00 padding │  │
  │  │ len=42   │ │ len=36   │ │ len=38   │ │  (not enough space for next │  │
  │  └──────────┘ └──────────┘ └──────────┘ │   record — pad to boundary) │  │
  │                                          └─────────────────────────────┘  │
  └────────────────────────────────────────────────────────────────────────────┘
  ┌────────────────────────────────────────────────────────────────────────────┐
  │  Sector N+1 (bytes 2048–4095)                                              │
  │  ┌──────────┐ ┌──────────┐ ...                                             │
  │  │ Record 4 │ │ Record 5 │                                                  │
  │  └──────────┘ └──────────┘                                                 │
  └────────────────────────────────────────────────────────────────────────────┘
```

**Critical**: when the length byte is `\x00`, it does **not** mean end of directory. It means
*skip to the next 2048-byte sector boundary*. This is the most common parser bug in ISO 9660
implementations. Your `DirEntryIter` must handle it:

```rust
pub struct DirEntryIter<'a> {
    data: &'a [u8],   // the full directory extent
    offset: usize,
}

impl<'a> Iterator for DirEntryIter<'a> {
    type Item = Result<DirectoryRecord<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let len = *self.data.get(self.offset)?; // bounds check
            if len == 0 {
                // Skip to next sector boundary
                let next_sector = (self.offset / 2048 + 1) * 2048;
                if next_sector >= self.data.len() { return None; }
                self.offset = next_sector;
                continue;  // ← loop again, don't return None here
            }
            // parse record at self.offset, advance self.offset by len
            // ...
        }
    }
}
```

**Special entries**: every directory extent begins with exactly two records:
- `\x00` identifier — the `.` (self) entry
- `\x01` identifier — the `..` (parent) entry

Validate these are present and correctly structured before processing any other entries.

#### 5.3.3 Path Table

```
Path Table Record (variable, 8+ bytes):

Offset  Size   Field
0       1      Length of Directory Identifier (N)
1       1      Extended Attribute Length (must be 0)
2       4      LBA of Extent (u32 — LE in Type L, BE in Type M)
6       2      Parent Directory Number (u16 — LE in Type L, BE in Type M)
8       N      Directory Identifier (d-characters)
8+N     0 or 1 Padding byte if N is odd
```

The path table is a flat, linear list of all directories. Directory numbering starts at 1 (root).
The root's parent directory number is `1` (itself — not `0`). Directories must appear in
*BFS order*: all directories at depth 1, then depth 2, etc. Within each depth level, siblings
are in lexicographic order.

Parse both the Type L (LE) and Type M (BE) tables and validate they agree. Consider
implementing a single generic `parse_path_table(bytes: &[u8], endian: Endian)` rather than
duplicating the parsing logic.

---

### Phase 4 — El Torito Boot Catalog (`eltorito.rs`)

The El Torito extension adds bootability to an ISO 9660 image. It lives entirely outside the
directory tree — accessible only via the LBA stored in the Boot Record VD.

#### 5.4.1 Boot Catalog Structure

```
  Boot Catalog (at LBA from Boot Record VD):

  ┌─────────────────────────────────────────────────────┐
  │  Entry 0: Validation Entry (32 bytes)               │
  │    Header ID = 1                                    │
  │    Platform ID (0=BIOS, 0xEF=EFI, 1=PPC, 2=Mac)    │
  │    ID String (manufacturer)                         │
  │    Checksum (all 16-bit LE words sum to 0)          │
  │    Key bytes: 0x55, 0xAA                            │
  ├─────────────────────────────────────────────────────┤
  │  Entry 1: Initial/Default Boot Entry (32 bytes)     │
  │    Boot Indicator (0x88=bootable, 0x00=not)         │
  │    Boot Media Type (0=no emulation)                 │
  │    Load Segment (0 = default 0x7C0)                 │
  │    Sector Count (in 512-byte virtual sectors)       │
  │    Load RBA (LBA of boot image in 2048-byte sectors)│
  ├─────────────────────────────────────────────────────┤
  │  Entry 2: Section Header (32 bytes)  [optional]     │
  │    Header Indicator (0x90=more, 0x91=last)          │
  │    Platform ID                                      │
  │    Number of Section Entries following              │
  ├─────────────────────────────────────────────────────┤
  │  Entry 3: Section Entry (32 bytes)                  │
  │    (same layout as Initial/Default Entry)           │
  └─────────────────────────────────────────────────────┘
```

Typical layout for a dual-boot (BIOS + EFI) image:

```
[Validation Entry]   platform=x86 BIOS
[Initial Entry]      no-emulation, Load RBA → BIOS boot image
[Section Header]     platform=EFI, entry_count=1, is_last=true
[Section Entry]      no-emulation, Load RBA → EFI boot image
```

#### 5.4.2 The Checksum Algorithm

The validation entry checksum is computed such that the sum of all 16 two-byte LE words in the
32-byte entry equals zero (wrapping u16 arithmetic).

**To validate**: sum all 16 words; assert the result is `0`.

**To build**: fill all fields, set checksum bytes to `0`, compute the sum, store `sum.wrapping_neg()` at bytes 28–29.

```rust
fn compute_checksum(entry: &[u8; 32]) -> u16 {
    (0..16)
        .map(|i| u16::from_le_bytes([entry[i * 2], entry[i * 2 + 1]]))
        .fold(0u16, u16::wrapping_add)
}
// For validation: assert compute_checksum(entry) == 0
// For building: entry[28..30] = compute_checksum(entry).wrapping_neg().to_le_bytes()
```

#### 5.4.3 Units Trap — Load RBA vs. Sector Count

**This is a common mistake**. Two fields use different units:

| Field | Unit |
|-------|------|
| `load_rba` | 2048-byte sectors (ISO LBAs) |
| `sector_count` | 512-byte "virtual" sectors |

One 2048-byte ISO sector = 4 virtual sectors. For a BIOS no-emulation boot image: conventional
`sector_count = 4`. For EFI: use the actual image size in 512-byte units.

---

### Phase 5 — Zero-Copy Parser (`parse.rs`)

This phase assembles the pieces into the public parsing API.

```rust
pub struct Iso9660Image<'a> {
    data:        &'a [u8],
    pvd:         PrimaryVolumeDescriptor<'a>,
    boot_record: Option<BootRecordDescriptor>,
}

impl<'a> Iso9660Image<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError>;
    pub fn primary_volume_descriptor(&self) -> &PrimaryVolumeDescriptor<'a>;
    pub fn root_dir(&self) -> Result<DirEntryIter<'a>, ParseError>;
    pub fn read_dir(&self, record: &DirectoryRecord<'a>) -> Result<DirEntryIter<'a>, ParseError>;
    pub fn file_contents(&self, record: &DirectoryRecord<'a>) -> Result<&'a [u8], ParseError>;
    pub fn boot_catalog(&self) -> Result<Option<ParsedBootCatalog<'a>>, ParseError>;
}
```

#### 5.5.1 Parse Strategy

```
Iso9660Image::parse(data):

  1. Validate data.len() >= 17 * 2048  (system area + at least 1 VD)
  2. Walk sectors starting at 16:
       for lba in 16.. :
         sector = lba_to_slice(data, lba, 2048)?
         match sector[0] {
           1   => parse PVD (set seen_primary=true, reject if already seen)
           0   => parse Boot Record
           255 => break  (terminator)
           _   => ignore unknown VD types (don't reject — extensions may add new types)
         }
  3. Reject if !seen_primary (NoPrimaryVolumeDescriptor)
  4. Return Iso9660Image { data, pvd, boot_record }
  5. Everything else is lazy (directories, path tables, boot catalog)
```

**Lazy vs. eager**: parse the VD set eagerly (it is small and always needed). Parse directory
contents, path tables, and boot catalogs *lazily* — only when the caller asks. This keeps
`parse()` proportional to the VD set size, not the entire filesystem.

#### 5.5.2 The LBA Helper

You will call `lba_to_slice` from nearly every method. Implement it once, correctly:

```rust
fn lba_to_slice(data: &[u8], lba: u32, len: usize) -> Result<&[u8], ParseError> {
    let start = (lba as usize)
        .checked_mul(2048)
        .ok_or(ParseError::UnexpectedEof { offset: 0, needed: len })?;
    let end = start
        .checked_add(len)
        .ok_or(ParseError::UnexpectedEof { offset: start, needed: len })?;
    data.get(start..end)
        .ok_or(ParseError::UnexpectedEof { offset: start, needed: len })
}
```

Note the use of `checked_mul` and `checked_add` — integer overflow in LBA arithmetic is a
real attack vector. A malicious image could set an LBA of `u32::MAX` to cause an overflow that
wraps around to a valid index, bypassing your bounds check. Never use unchecked arithmetic on
untrusted values.

#### 5.5.3 File Data

`file_contents` is the simplest method — it returns a slice into the image at the record's
extent:

```rust
pub fn file_contents(&self, record: &DirectoryRecord<'a>) -> Result<&'a [u8], ParseError> {
    lba_to_slice(self.data, record.extent_lba.get(), record.data_length.get() as usize)
}
```

Zero-copy, zero-allocation. The caller gets a `&[u8]` that references the original image
memory directly.

---

### Phase 6 — Builder (`build.rs`)

The builder is conceptually the most complex phase. Its core insight is the **two-pass
algorithm**: you cannot write the PVD until you know every LBA, but you cannot know every LBA
until you have laid out the entire image.

#### 5.6.1 Builder API

```rust
pub struct IsoBuilder {
    volume_id:  [u8; 32],
    system_id:  [u8; 32],
    root:       DirBuilder,
    boot:       Option<BootConfig>,
}

pub struct DirBuilder {
    name:    Option<String>,  // None for root
    entries: Vec<Entry>,
}

pub enum Entry {
    File(FileSource),
    Dir(DirBuilder),
}

pub struct FileSource {
    pub name:   FileName,    // validated 8.3 name
    pub source: FileData,
}

pub enum FileData {
    Bytes(Vec<u8>),
    External {
        len:    u64,
        writer: Box<dyn FnOnce(&mut dyn Write) -> io::Result<()>>,
    },
}
```

#### 5.6.2 The Two-Pass Algorithm

```
Pass 1: Layout (assign LBAs)
────────────────────────────

  current_lba = 0

  System Area:         lba 0–15        (16 sectors of zeros)
  PVD:                 lba 16          (always sector 16)
  Boot Record VD:      lba 17          (if boot_config present)
  VD Terminator:       lba 17 or 18
  Boot Catalog:        next lba        (if boot_config present)
  Path Table Type L:   next lba
  Path Table Type M:   next lba
  Directory extents:   next lba (BFS: root, then its children, then their children...)
  File extents:        next lba (after all directories)
  Boot image extents:  next lba (conventionally last)

  ┌─────────────────────────────────────────────────────────────────────────┐
  │                         Layout Pass Diagram                             │
  │                                                                         │
  │  In-memory tree:          Assigned LBAs (example):                     │
  │                                                                         │
  │  Root/                    Root dir extent   → LBA 20                   │
  │  ├── BOOT/                BOOT/ dir extent  → LBA 21                   │
  │  │   └── EFI.IMG;1        DOCS/ dir extent  → LBA 22                   │
  │  └── DOCS/                EFI.IMG;1 data    → LBA 23                   │
  │      └── README.TXT;1     README.TXT;1 data → LBA 24                   │
  │                                                                         │
  │  After layout, every node knows its LBA.                                │
  │  Total sector count → written into PVD.volume_space_size.              │
  └─────────────────────────────────────────────────────────────────────────┘

Pass 2: Write (serialize in LBA order)
───────────────────────────────────────

  Seek to lba 0,  write 16 sectors of zeros
  Seek to lba 16, write PVD (now all LBAs are known — fill them in)
  Seek to lba 17, write Boot Record VD (if present)
  Seek to lba N,  write Terminator
  Seek to lba N+1, write boot catalog (if present)
  Seek to lba N+2, write Path Table Type L
  Seek to lba N+3, write Path Table Type M
  Seek to lba 20, write Root dir extent (padded to 2048)
  Seek to lba 21, write BOOT/ dir extent
  ...
  Seek to lba 23, invoke EFI.IMG writer closure → data streams to output
  Seek to lba 24, invoke README.TXT writer closure
```

**Sector padding**: every written region must end on a 2048-byte boundary. Implement one
shared helper:

```rust
fn pad_to_sector<W: Write + Seek>(w: &mut W) -> io::Result<()> {
    let pos = w.stream_position()? as usize;
    let remainder = pos % 2048;
    if remainder != 0 {
        let padding = 2048 - remainder;
        w.write_all(&vec![0u8; padding])?;
    }
    Ok(())
}
```

#### 5.6.3 The `FileData::External` Pattern

Rather than buffering entire files in memory, accept a closure that streams data directly to
the output writer. During pass 1, use `len` for sector allocation. During pass 2, seek to
the allocated LBA and invoke the closure:

```rust
// Pass 2 — writing a file:
let byte_offset = (file_lba as u64) * 2048;
writer.seek(SeekFrom::Start(byte_offset))?;
match file.source {
    FileData::Bytes(bytes) => writer.write_all(&bytes)?,
    FileData::External { writer: f, .. } => f(writer)?,
}
pad_to_sector(writer)?;
```

This allows callers to stream multi-gigabyte files without any intermediate buffering.

#### 5.6.4 Level 1 Enforcement

Validate at `add_file`/`add_dir` time, not at build time, so errors surface early:

| Rule | Rejection |
|------|-----------|
| Filename must match `^[A-Z0-9_]{1,8}(\.[A-Z0-9_]{1,3})?$` | `InvalidFileName` |
| Directory name must match `^[A-Z0-9_]{1,8}$` | `InvalidFileName` |
| Directory depth must be <= 8 | `DirectoryTooDeep` |
| Total sectors must fit in u32 | `ImageTooLarge` |

The builder automatically appends `;1` to file identifiers — callers provide `KERNEL.ELF`,
the builder writes `KERNEL.ELF;1` in the directory record.

---

## 6. Testing Strategy

### 6.1 Parser Tests

Two Debian ISO images are provided in `tests/` as fixtures. Your parser must handle them
correctly.

**Ground truth**: use `libcdio` tools to generate expected output, then assert your parser
matches:

```bash
# Full structure dump
iso-info -d tests/debian-13.4.0-arm64-netinst.iso

# Directory listing
iso-read -l tests/debian-13.4.0-arm64-netinst.iso

# Extract a specific file
iso-read -i tests/debian-13.4.0-arm64-netinst.iso -e /BOOT/GTK/INITRD.GZ -o /tmp/initrd.gz

# Boot catalog info
iso-info --no-header -d tests/debian-13.4.0-arm64-netinst.iso | grep -A20 "El Torito"
```

**Required test cases**:
1. Parse both Debian ISOs without error
2. PVD fields match `iso-info` output
3. Full directory tree walk matches `iso-read` listing
4. El Torito boot catalog platform IDs and Load RBAs are correct
5. All both-endian fields have matching LE and BE copies (spot-check 100 records)
6. Truncated image → `UnexpectedEof`
7. Corrupt magic bytes → `InvalidMagic`
8. Mismatched both-endian field → `BothEndianMismatch`
9. Zero length byte mid-extent → iterator skips to next sector correctly

### 6.2 Builder Tests

```bash
# Validate a generated image
iso-info -d output.iso
iso-read -l output.iso
```

**Required test cases**:
1. Build minimal ISO (root dir, no files) → parse back → round-trip matches
2. Build ISO with files → `iso-read` can extract them correctly
3. Build bootable ISO (BIOS only) → `iso-info -d` shows correct boot catalog
4. Build dual-boot ISO (BIOS + EFI) → both entries present with correct LBAs
5. `FileData::External` with a 100 MiB file → builds without allocating 100 MiB
6. Filename validation: reject lowercase, reject name > 8 chars, reject ext > 3 chars
7. Directory depth: reject tree with depth 9
8. Round-trip: parse Debian ISO → extract structure → rebuild → re-parse → compare PVD fields

### 6.3 Integration Test Pattern

```rust
#[test]
fn test_builder_roundtrip() {
    let mut iso_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut iso_bytes);

    let mut builder = IsoBuilder::new("TESTDISK").unwrap();
    builder.root()
        .add_file(FileSource {
            name: FileName::new("README.TXT").unwrap(),
            source: FileData::Bytes(b"hello world".to_vec()),
        }).unwrap();
    builder.build(&mut cursor).unwrap();

    // Parse it back
    let parsed = Iso9660Image::parse(&iso_bytes).unwrap();
    let mut root = parsed.root_dir().unwrap();
    // skip . and ..
    root.next(); root.next();
    let readme = root.next().unwrap().unwrap();
    assert_eq!(readme.file_identifier, b"README.TXT;1");
    assert_eq!(parsed.file_contents(&readme).unwrap(), b"hello world");
}
```

---

## 7. Common Pitfalls

These are mistakes that will cost you hours if not internalized before you start.

| Pitfall | Consequence | Fix |
|---------|-------------|-----|
| Treating `\x00` length byte as end-of-directory | Missing entries, test failures | Skip to next sector boundary; continue iterating |
| Using unchecked arithmetic on LBA values | Integer overflow → wrong slice bounds → security hole | Use `checked_mul`, `checked_add` everywhere |
| Forgetting to append `;1` to file identifiers | `iso-read` cannot find files | Builder auto-appends; parser accepts both |
| Root's path table parent number = 0 | Spec violation, some readers reject | Root's parent = 1 (itself) |
| Confusing `load_rba` units (2048-byte) with `sector_count` units (512-byte) | Boot image loaded at wrong sector | Keep the unit distinction explicit in type names |
| Forgetting zero-padding between records at sector boundary | Records across sectors cause UB/panics | `pad_to_sector` after every extent |
| Boot catalog LBA not in directory tree | Directory listing looks correct but boot fails | Boot catalog referenced only by Boot Record VD |
| Indexing slices directly without bounds-checking | Panic or out-of-bounds read | All access through `lba_to_slice` and `bytes.get()` |
| Writing PVD before layout pass completes | LBAs in PVD are wrong | Two-pass algorithm: write PVD in second pass only |

---

## 8. Deliverable Checklist

Before submitting, verify:

- [ ] `cargo test` passes with no `#[ignore]` tests skipped
- [ ] `cargo clippy -- -D warnings` produces no warnings
- [ ] `cargo build --no-default-features` succeeds (confirms `no_std` compatibility of core modules)
- [ ] `iso-info -d` accepts your builder output without errors
- [ ] `iso-read -l` lists exactly the files you added
- [ ] Both Debian ISOs parse without errors
- [ ] Truncated image returns `ParseError::UnexpectedEof`, not a panic
- [ ] `BothEndianMismatch` is returned for any image with mismatched endian copies
- [ ] All "unused" fields are rejected if nonzero
- [ ] The round-trip test passes: parse → rebuild → re-parse → identical structure

---

## 9. Grading

| Component | Weight | Criteria |
|-----------|--------|----------|
| `error.rs` + `types.rs` | 10% | Both-endian parse+validate, date/time parse, character set validation |
| `volume.rs` | 15% | PVD parse, all fields correct, all validation enforced |
| `directory.rs` | 20% | DirRecord parse, iterator with sector-skip, path table both endiannesses |
| `eltorito.rs` | 15% | Checksum algorithm, all entry types, platform IDs |
| `parse.rs` | 15% | Lazy/eager split, LBA helper, Debian ISO passes, security cases |
| `build.rs` | 20% | Two-pass algorithm, `iso-info` validation, round-trip test |
| Security hardening | 5% | Checked arithmetic, unused-field rejection, `BothEndianMismatch` |

Good luck. The format is more regular than it first appears — once you have `types.rs` and
`directory.rs` solid, the rest falls into place quickly.
