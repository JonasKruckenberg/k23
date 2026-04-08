# ISO 9660 Library — Implementation Guide

A mentor-style guide for building a correct, secure ISO 9660 / ECMA-119 parser and builder in Rust.
You're an experienced systems dev and Rust expert, so this skips basics and focuses on the domain-specific
knowledge, key decisions, and the pitfalls that will actually cost you time.

---

## Primary References

These are the authoritative sources. Keep them open.

- **[ECMA-119 (4th edition, 2019)](https://www.ecma-international.org/publications-and-standards/standards/ecma-119/)** — the actual standard. Free PDF. The 4th edition is the most recent. Everything in your library should be traceable to a section here.
- **[El Torito specification](https://pdos.csail.mit.edu/6.828/2018/readings/boot-cdrom.pdf)** — the boot catalog extension. Not part of ECMA-119 itself; it's a separate industry spec. Short (25 pages) — read it fully.
- **[OSDev Wiki: ISO 9660](https://wiki.osdev.org/ISO_9660)** — excellent practical summary with byte-offset tables. Great for cross-checking your struct layouts.
- **[OSDev Wiki: El Torito](https://wiki.osdev.org/El-Torito)** — same, for the boot catalog.

---

## Existing Implementations Worth Reading

Study these before writing a line. They'll save you hours.

| Library | Language | What to study |
|---------|----------|---------------|
| **[libcdio](https://www.gnu.org/software/libcdio/)** | C | `lib/iso9660/` — the reference impl. Messy but authoritative. Look at `iso9660.h` for struct layouts. |
| **[pycdlib](https://github.com/clalancette/pycdlib)** | Python | Best-documented pure-software ISO library. The [internals doc](https://clalancette.github.io/pycdlib/pycdlib-internals.html) is exceptional — read it. Explains the two-pass layout algorithm clearly. |
| **[cdfs](https://github.com/nicholasbishop/cdfs)** | Rust | Small, readable. Good zero-copy `&[u8]` parsing approach. Note: parsing only, no builder. |
| **[iso9660 (crate)](https://crates.io/crates/iso9660)** | Rust | Heavier but more complete. Compare API surface. |

---

## Conceptual Map — Read This First

Before touching byte offsets, understand the three structural concepts:

### 1. The Volume Descriptor Set

The image starts with 16 silent sectors (the "system area"), then a sequence of 2048-byte Volume Descriptors
beginning at sector 16. Each VD has a 1-byte type code at offset 0 and the magic `"CD001"` at offsets 1–5.
You walk them in order until you hit type 255 (the terminator). The order matters: the Primary VD (type 1)
must exist, the Boot Record (type 0) is optional, there must be exactly one terminator.

**Key insight**: the VD set is your entry point to everything else. The Primary VD contains the LBA of the
root directory record, and the Boot Record VD contains the LBA of the El Torito boot catalog. You find
everything else by following these pointers.

### 2. Two Directory Structures (They Must Agree)

ISO 9660 stores the directory tree in *two* redundant structures:

- **Directory Records**: each directory occupies one or more sectors ("extents"). The root dir record is
  embedded in the PVD. Each record is a variable-length struct followed by child records packed end-to-end
  within the extent. Records cannot cross sector boundaries — a record that won't fit is skipped with
  zero-padding.

- **Path Tables**: a flat linear table of all directories, in a specific traversal order. Stored in both
  little-endian (Type L) and big-endian (Type M) versions. Historically for performance on CD-ROM drives —
  less relevant now, but still mandatory.

Both structures must describe the same tree. When building, you write both. When parsing with security in
mind, you should validate they agree.

### 3. Both-Endian Fields

Every multi-byte integer in ECMA-119 is stored as its LE bytes followed immediately by its BE bytes.
This was designed for portability across CPU architectures in the 1980s. Today it just means you validate
both copies match. The El Torito spec is an exception — some of its fields are LE-only.

---

## Implementation Phases

### Phase 1: Primitive Types (`types.rs`)

**Goal**: build the types everything else composes.

**What to build**:
- `U16BothEndian` and `U32BothEndian` — parse-validate, get, serialize
- `DecDatetime` (17 bytes, ASCII digits + GMT offset) — Section 9.1.5 of ECMA-119
- `DirDatetime` (7 bytes, binary fields + GMT offset) — Section 9.1.5
- Character set validators for d-characters and a-characters — Section 7.4
- Fixed-length padded string types (`AString<N>`, `DString<N>`)

**Key decisions you'll face**:

*Parsing both-endian types*: your parse function will receive a `&[u8]`. You'll need to extract 4 or 8
bytes. The cleanest approach is a helper that takes `bytes: &[u8]` and an `offset: usize` and returns
`Result<T, ParseError>` after bounds-checking. Avoid indexing directly — every access is a potential
panic. Look at how `cdfs` does it: it uses a `get_both_endian` style function. With Rust you can do this
cleanly with `bytes.get(offset..offset+4).ok_or(ParseError::UnexpectedEof)?` followed by
`<[u8; 4]>::try_from(...)`.

*Validating both-endian equality*: parse LE with `u16::from_le_bytes`, parse BE with `u16::from_be_bytes`,
then compare. Reject if they differ. This is your first security gate.

*Date/time*: `DecDatetime` stores all fields as ASCII digit bytes (e.g., year `"2024"` is bytes
`[0x32, 0x30, 0x32, 0x34]`). You validate each field is in `b'0'..=b'9'`. `DirDatetime` uses raw binary
values instead. Don't conflate them.

*Character sets*: d-characters are `[A-Z0-9_]`. a-characters add space and punctuation.
See ECMA-119 §7.4 for the exact list. Validate every string field on parse — this is where malformed images
typically try to smuggle bad data.

**Useful Rust patterns here**:
- `#[repr(transparent)]` newtype wrappers give zero-cost abstractions
- `impl TryFrom<&[u8]> for U32BothEndian` is a natural fit for parsing
- For fixed-length strings, `[u8; N]` works well — no allocation, `no_std` compatible

---

### Phase 2: Volume Descriptors (`volume.rs`)

**Goal**: parse and serialize all three VD types you need (Primary, Boot Record, Terminator).

**What to build**: see the byte-offset table in the plan and the OSDev wiki. The PVD is 2048 bytes, almost
entirely defined. The Boot Record is simple (mostly zeros + a 4-byte LBA). The Terminator is trivial.

**Key decisions you'll face**:

*Borrowed vs owned*: since you're parsing zero-copy from `&[u8]`, your structs will carry lifetimes.
`PrimaryVolumeDescriptor<'a>` borrows string fields directly from the input slice. This is idiomatic and
avoids allocation. The tradeoff: you can't easily store parsed VDs in a struct that outlives the original
slice without cloning. For your use case (OS images, parsing-heavy), the zero-copy approach wins.

*The embedded root directory record*: at PVD offset 156, there's a 34-byte directory record for the root
directory. You'll parse this as a `DirectoryRecord<'a>` in-place. This is the entry point to the whole
filesystem tree. The root directory record's `file_identifier` is always `\x00` (the `.` self-reference).

*Validating reserved/unused fields*: ECMA-119 marks many fields as "unused" and specifies they must be
zero. Under your "maximally strict" policy, reject on any nonzero unused byte. This matters for security:
hidden data in unused fields is a real attack vector in ISO images used as OS inputs.

**Resources**:
- ECMA-119 §8.4 (Primary Volume Descriptor) — the authoritative field list
- OSDev wiki layout tables are easier to scan for implementation

---

### Phase 3: Directory Records & Path Tables (`directory.rs`)

**Goal**: parse directory extents and both path table variants.

**What to build**:
- `DirectoryRecord<'a>` with all fields
- `FileFlags` — a flags byte, 8 bits, use `u8` with named constants or a bitflags-style type
- `DirEntryIter<'a>` — an iterator over records within a directory extent
- `PathTableRecord<'a>` for Type L (LE) and Type M (BE) variants

**Key decisions you'll face**:

*Iterating directory records*: records are packed sequentially within the extent. Each record starts with
a length byte. To advance, you add that length to your current offset. Critical: a length byte of `0`
means "skip to the next sector boundary" (zero-padding). Your iterator must handle this — it's a common
bug omission. See ECMA-119 §9.1.

*Validating directory record length*: the record length must be even (records are always padded to even
length), >= 33 (minimum record size), and the identifier length must be consistent with the total length.
Reject anything that doesn't add up — malformed lengths are the classic way to cause parsers to go out
of bounds.

*The `.` and `..` entries*: every directory extent starts with two special records: `\x00` (self) and
`\x01` (parent). Validate they're present and correctly structured before processing any other entries.

*Path table numbering*: directories in the path table are numbered from 1 (root). Each entry references
its parent by number. The root's parent is 1 (itself). You'll need this for the builder to correctly fill
`parent_directory_number`.

*Type L vs Type M*: identical structure except integers are LE in Type L and BE in Type M. Consider a
generic `parse_path_table(bytes, endian)` approach rather than duplicating code.

**A subtlety worth knowing**: the path table must contain directories in a specific order — parent before
children, siblings in lexicographic order. The builder must sort accordingly. ECMA-119 §6.9.1.

---

### Phase 4: El Torito Boot Catalog (`eltorito.rs`)

**Goal**: parse and serialize the boot catalog.

**What to build**: validation entry, initial/default boot entry, section headers, section entries.

**Key decisions you'll face**:

*Finding the catalog*: the Boot Record VD gives you an absolute LBA. That sector contains the catalog.
It's not referenced in the directory tree — it's a raw sector pointer. When building, you must allocate
a sector for it explicitly.

*The checksum*: the validation entry (32 bytes) must checksum to zero when all 16 sixteen-bit LE words
are summed (wrapping). To build a valid entry: fill all fields, set checksum bytes to `0`, compute the
sum, negate it (wrapping), store the negation in bytes 28–29. To validate: sum all 16 words and assert
the result is zero. ECMA-119 doesn't document this — it's in the El Torito spec, §2.

*Key bytes*: the validation entry ends with `0x55, 0xAA`. Reject if missing. This is a trivial guard
but required.

*Platform IDs*: `0x00` = x86 BIOS, `0xEF` = EFI, `0x01` = PowerPC, `0x02` = Mac. For your k23 use
case you likely only care about BIOS and EFI. Represent as an enum with a catch-all variant for unknown
platform IDs rather than rejecting them on parse (some valid images use values you don't know about).

*Boot media types*: `0` = no emulation (almost always what modern boot images use), `1-3` = floppy
emulation (legacy), `4` = hard disk emulation. For UEFI, always `0`.

**Resources**:
- El Torito spec PDF (linked above) — short, read it all
- [OSDev El Torito](https://wiki.osdev.org/El-Torito) — has worked examples of catalog byte layouts

---

### Phase 5: Zero-Copy Parser (`parse.rs`)

**Goal**: assemble the pieces into a usable top-level API.

**What to build**:

```rust
pub struct Iso9660<'a> { ... }   // wraps &'a [u8]

impl<'a> Iso9660<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError>
    pub fn primary_volume_descriptor(&self) -> &PrimaryVolumeDescriptor<'a>
    pub fn root_dir(&self) -> Result<DirEntryIter<'a>, ParseError>
    pub fn read_dir(&self, record: &DirectoryRecord<'a>) -> Result<DirEntryIter<'a>, ParseError>
    pub fn file_data(&self, record: &DirectoryRecord<'a>) -> Result<&'a [u8], ParseError>
    pub fn boot_catalog(&self) -> Result<Option<BootCatalog<'a>>, ParseError>
}
```

**Key decisions you'll face**:

*Lazy vs eager parsing*: parse the VD set eagerly (it's small, always needed), but parse directory
contents and boot catalogs lazily on access. This avoids doing unnecessary work and keeps parse time
proportional to what you actually use.

*LBA-to-byte-offset conversion*: everything in ISO 9660 is addressed by Logical Block Address. The
formula is `lba * logical_block_size`. Your PVD gives you `logical_block_size` (must be 2048). You'll
call `lba_to_slice(data, lba, len)` constantly — implement this as a single shared helper that does
bounds checking in one place.

*File data*: `file_data` is simply a slice into the image at the extent LBA with the extent's data
length. Zero-copy, zero-allocation. Validate that `lba * 2048 + data_length <= data.len()`.

*Multiple volume descriptors*: walk VDs from sector 16 forward. ECMA-119 allows any number before
the terminator. Keep a `seen_primary: bool` and reject images with zero or more than one PVD.

**Testing approach**: parse your two Debian ISOs and walk the full directory tree. Cross-check with
`iso-info -d tests/debian-*.iso` and `iso-read -l tests/debian-*.iso` for exact expected output.
Writing a test that compares your parsed directory listing against `iso-info` output is high-value.

---

### Phase 6: Builder (`build.rs`)

**Goal**: write a valid ISO image to a `Write + Seek` target.

**What to build**: `IsoBuilder`, `DirBuilder`, `FileSource`/`FileData` types.

**The two-pass algorithm** — this is the conceptual core of the builder:

**Pass 1 (layout)**: walk your in-memory tree and assign a sector (LBA) to every region of the image.
The order is fixed:
1. System area: sectors 0–15 (16 sectors of zeros)
2. Primary VD: sector 16
3. Boot Record VD: sector 17 (if present)
4. VD Set Terminator: sector 17 or 18
5. Boot catalog: next sector (if present)
6. Path Table Type L: next sector
7. Path Table Type M: next sector
8. Directory extents: root first, then all subdirectories (BFS order matches path table numbering)
9. File data extents: after all directories
10. Boot image extents: conventionally last, but can be anywhere

pycdlib's [internals doc](https://clalancette.github.io/pycdlib/pycdlib-internals.html) explains this
algorithm with great clarity — read the "Writing" section specifically.

**Pass 2 (write)**: now that every LBA is known, serialize each region in order. Seek to the correct
position, write the bytes. For `FileData::External { len, writer }`, seek to the file's assigned LBA
and invoke the closure — the data streams straight to the output.

**Key decisions you'll face**:

*Sector padding*: every region must be padded to the next sector boundary (2048 bytes). If a directory
extent is 500 bytes, you write 500 bytes of content then 1548 bytes of zeros. Use a helper
`pad_to_sector(writer, current_pos) -> io::Result<()>`.

*Computing total volume size*: after pass 1, you know the total sector count. This goes into the PVD's
`volume_space_size` field. You can only write the PVD after layout is complete.

*Path table ordering*: directories in the path table must be in BFS order. Track directory numbers
during layout pass — each directory gets a sequential number starting from 1 (root).

*Computing directory extent sizes*: during layout, compute exactly how many bytes each directory
extent requires (sum of all child record lengths, with sector-crossing padding). Some directories
may span multiple sectors if they have many children.

*Level 1 enforcement*: reject filenames not matching `^[A-Z0-9_]{1,8}(\.[A-Z0-9_]{1,3})?$`,
directory identifiers not matching `^[A-Z0-9_]{1,8}$`, directory depth > 8. Do this at file-add
time, not at build time, so errors surface early.

*The `FileData::External` pattern*: rather than buffering file content, accept a closure
`Box<dyn FnOnce(&mut dyn Write) -> io::Result<()>>` plus a `len: u64`. During pass 1, use `len`
to compute the sector allocation. During pass 2, seek to the allocated position and call the closure.
The data flows directly from the closure to the output writer. Clean and zero-intermediate-copy.

---

## Error Handling Strategy

You're writing hand-rolled error enums. Here's the structure that works well:

```rust
// no_std, for parse errors
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
    // ...
}

// std, for build errors
pub enum BuildError {
    Io(io::Error),
    InvalidFileName { name: String },
    DirectoryTooDeep { depth: u8 },
    ImageTooLarge { sectors: u64 },
    // ...
}
```

Carry as much context as possible in each variant — offset, found value, expected value. When you're
staring at a corrupted image at 2am, you'll be glad you did.

---

## Validation Workflow

Use the libcdio tools as your ground truth:

```bash
# Full structure dump of your generated image
iso-info -d output.iso

# List all files
iso-read -l output.iso

# Read a specific file out
iso-read -i output.iso -e /BOOT/EFI.IMG -o /tmp/extracted.img

# El Torito boot catalog info
iso-info --no-header -d output.iso | grep -A20 "El Torito"
```

Write integration tests that:
1. Build an image with your builder
2. Write it to a `tempfile::NamedTempFile`
3. Shell out to `iso-info` and parse stdout
4. Assert your expected structure matches

---

## Common Pitfalls

- **Zero-padding between records**: the most common bug. A `\x00` length byte doesn't mean end of
  directory — it means skip to the next sector. Your `DirEntryIter` must handle this.

- **Root directory `.` record**: the LBA and size in the root directory record embedded in the PVD
  must exactly match the first entry (`\x00`) within the root directory extent itself.

- **Path table parent of root**: root's parent directory number is `1` (itself), not `0`.

- **Version suffix on file identifiers**: Level 1 files must have `;1` appended. `KERNEL.ELF` is
  wrong; `KERNEL.ELF;1` is correct. Your builder should append this automatically.

- **Sector alignment of all extents**: every extent (directory or file) must start on a 2048-byte
  boundary. Even if the previous file was 1 byte, the next file starts at the next sector.

- **Boot catalog not in directory tree**: the boot catalog is referenced only by its LBA in the
  Boot Record VD. It does NOT appear as a file in the directory. Some builders optionally add it
  as a hidden file — that's optional, skip it for MVP.

- **El Torito load RBA vs sector count units**: `load_rba` is in 2048-byte sectors. `sector_count`
  in the boot entry is in 512-byte *virtual* sectors. One 2048-byte sector = 4 virtual sectors.
  For BIOS no-emulation, `sector_count = 4` is conventional. For EFI, use the actual image size
  in 512-byte units.
