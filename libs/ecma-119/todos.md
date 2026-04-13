# ecma-119 TODO List
> Scoped to the primary use case: building bootable UEFI ISO images for RISC-V.
> Issues are ordered within each section by impact.

---

## Blockers — nothing boots until these are fixed

- [ ] **El Torito serialization: architectural gap in LBA allocation**
  `build/layout.rs:194`, `build/layout.rs:251`
  `assign_lbas` never allocates a sector for the boot image's `FileSource` (which lives inside
  `BootConfigBuilder`, invisible to the layout engine). `serialize` then bails immediately with
  `Err(Unsupported)`. Fix: require the boot image to be a file already present in the directory
  tree (e.g. `EFI/BOOT/BOOTRISCV64.EFI`); `BootEntryBuilder` holds a reference into the tree
  and layout looks up the already-assigned LBA. This also gives firmware the filesystem fallback
  path it expects.

- [ ] **El Torito serialization: nothing is written**
  `build/layout.rs:251`
  Once LBA allocation is fixed, implement the actual write path:
  - Write `BootRecord` VD at sector 17; push VD Set Terminator to sector 18
    (the current `assign_lbas` already reserves this sector with `lba += 1` but `serialize`
    doesn't write it)
  - Write `ValidationEntry` with `platform_id = 0xEF` and correct 16-bit word-sum checksum
    (sum of all 16-bit words in the 32-byte entry must equal zero)
  - Write `InitialEntry` with `boot_indicator = 0x88`, `boot_media_ty = 0x00` (no emulation),
    `sector_count = 1`, and `load_rba` = the boot image's assigned LBA

---

## Crash bugs — malformed input must not panic

The parser uses `panic!`, `assert!`, and `.unwrap()` throughout hot paths. Any malformed
or attacker-controlled ISO crashes the process. Every one of these must return a `ParseError`
variant instead.

- [ ] **`parser.rs:58–65`** — `bytes()` panics on out-of-bounds; add `ParseError::UnexpectedEof`
- [ ] **`parser.rs:105–107`** — `peek()` panics via `.unwrap()`
- [ ] **`parser.rs:119–121`** — `assert!` on sector alignment; return an error
- [ ] **`parser.rs:148`** — `panic!("unknown volume descriptor type")` — skip or error, never panic
- [ ] **`parser.rs:153`** — `primary.unwrap()` — missing PVD should be `ParseError::NoPrimaryVolumeDescriptor`
- [ ] **`parse/mod.rs:169–174`** — `assert_eq!` in `Image::root()` should be an error return
- [ ] **`rock_ridge.rs:213–215`** — `panic!("unexpected rock ridge entry")` when a known SUSP entry is passed to the RR iterator
- [ ] **`rock_ridge.rs:225–226, 238`** — `assert_eq!` / `assert!` → proper errors
- [ ] **`rock_ridge.rs:273`** — `AlternateNameFlags::from_bits(...).unwrap()` — unknown flags → error
- [ ] **`rock_ridge.rs:350`** — `panic!("unknowmn rock ridge entry")` (also a typo) — unknown RR entries should yield `RockRidgeEntry::Unknown` or be skipped
- [ ] **`eltorito/parse.rs:89–91`** — three `assert_eq!` should be `ParseError::Invalid`
- [ ] **`directory.rs:199–211`** — FIXME: when `header.len < min_len`, falls through to a usize
  underflow; subtracting `file_identifier_len + pad` from a smaller `len` wraps in release mode
  and then `bytes()` panics or reads garbage. Return an error here.

---

## Silent correctness bugs — will produce broken images

- [ ] **`is_a_char` accepts lowercase — spec violation**
  `validate.rs:98–99`
  ECMA-119 §7.4.2 a-characters are uppercase only. Remove `b.is_ascii_lowercase()` from
  `is_a_char`. Then fix `ImageBuilder::new()` (`build/mod.rs:61`) which sets a lowercase
  `data_preparer_id` — uppercase it or use a compliant string. The current `.unwrap()` there
  will panic once the validator is correct.

- [ ] **`validate_dir_identifier` allows `.` and `;` in directory names**
  `build/directory.rs:115`
  Directory identifiers are d-characters only (ECMA-119 §6.8.2.2). Remove the
  `b != b'.' && b != b';'` exceptions. File identifiers are unaffected.

- [ ] **`required_extent_size` and `write_dir_record` duplicate the record-size formula**
  `build/layout.rs:98–101` and `build/layout.rs:463–464`
  Both compute `header_size + id_len + pad`. If one changes without the other, directory
  extents are misaligned and the image is silently corrupt. Extract to a shared
  `fn dir_record_size(id_len: u8) -> u32`.

- [ ] **Missing `;1` version suffix on file identifiers**
  `build/directory.rs:73`
  ECMA-119 Level 1 requires file identifiers to end in `;1`. Some firmware and all
  spec-compliant parsers expect it. Append `;1` automatically in the builder and adjust
  `validate_file_identifier` to enforce the `NAME.EXT;VER` structure (one `.`, one `;`,
  version digits only, version 1–32767).

- [ ] **`validate_file_identifier` does not enforce `NAME.EXT;VER` structure**
  `build/directory.rs:72–95`
  The validator checks individual byte legality but allows `;;;`, `.....`, etc. Enforce:
  at most one `.`, at most one `;`, `;` must be followed by decimal digits only.

- [ ] **`DirEntryIter` exposes `.` and `..` entries**
  `parse/directory.rs:181`
  Every caller manually filters `if matches!(identifier, "." | "..") { continue }`. Skip
  these by default; expose an opt-in `include_dot_entries()` adapter if needed.

---

## API design — will get in the way as the codebase grows

- [ ] **Boot image and directory tree are decoupled — fix the ownership model**
  `build/boot.rs:101`, `build/directory.rs:38`
  `BootEntryBuilder` holds its own `FileSource`. Once the boot image is required to live in
  the directory tree (see blocker above), `BootEntryBuilder` should hold a path into the tree
  (or a reference to an already-added `FileNode`) rather than duplicating file data.

- [ ] **`boot_catalog()` borrows `ImageBuilder` for too long**
  `build/mod.rs:115`
  `boot_catalog()` returns `&mut BootConfigBuilder<'a>` which keeps `ImageBuilder` mutably
  borrowed. You can't configure boot and directories in the same scope without borrow
  gymnastics. Decouple them — configure boot separately and pass it into `finish()`.

- [ ] **`FileSource` lifetime forces callers to manage all byte lifetimes externally**
  `build/directory.rs:10`
  `FileSource::InMemory(&'a [u8])` requires callers to keep all file data alive for the
  builder's lifetime. Add a `FileSource::Owned(Vec<u8>)` variant (or use `Cow<'a, [u8]>`)
  so in-memory builds don't require external lifetime management.

- [ ] **`add_dir` and `add_file` return inconsistently**
  `build/directory.rs:45, 55`
  `add_dir` returns `&mut DirectoryBuilder` (the child). `add_file` returns `&mut Self`
  (the parent). Pick one convention.

- [ ] **`FileSource::len()` silently truncates `u64` to `usize`**
  `build/directory.rs:22`
  `OnDisk { len: u64, .. } => *len as usize` wraps on 32-bit hosts. Validate that file
  length fits in `u32` (the `data_length` field is 32-bit) at `add_file` time and return
  an error.

- [ ] **`PathTableIter` is exported but unreachable**
  `src/lib.rs:8`
  The path table accessor methods on `Image` are commented out. Either expose them or remove
  `PathTableIter` from the public API.

---

## Spec compliance — not blockers for UEFI boot but produce non-compliant images

- [ ] **All directory record timestamps are zero**
  `build/layout.rs:398`
  `build_dir_record_header` sets `recording_date` to all zeros. Set it to the current time.
  Tools like `xorriso --check_media` flag this; it won't affect UEFI firmware.

- [ ] **`VolumeDescriptorHeader` validation rejects Enhanced VDs (version 2)**
  `raw.rs:637`
  The validator requires `volume_descriptor_version == 1`, but Enhanced VDs have version 2.
  Strict mode will reject any image with an Enhanced VD. Fix the check to allow version 2
  for descriptor type 2.

- [ ] **`BootRecord.boot_id` not validated**
  `raw.rs:817`
  El Torito spec requires `boot_id` to be all zeros. The current `Validate for BootRecord`
  only checks `boot_system_id`.

- [ ] **`MULTI_EXTENT` flag not handled**
  `parse/directory.rs:232`
  Files split across multiple extents yield separate directory entries instead of being
  reassembled. Add a layer that stitches them together before returning to the caller.

---

## Code quality — clean up when convenient

- [ ] **`DecDateTime.hundreth` typo in public API** — `raw.rs:301` — rename to `hundredth`
- [ ] **`LOCKABLE` has the same bit value as `SET_GID`** — `rock_ridge.rs:121` — delete `LOCKABLE`
- [ ] **Sorting in `flatten` happens twice** — `build/layout.rs:155, 179` — the per-category
  sorts during BFS are redundant; only the final `sorted_entry_ids` sort matters
- [ ] **`pad_to_sector` calls `stream_position()` in tight write loop** — `build/layout.rs:367`
  — track write offset explicitly or use a `CountingWriter` wrapper
- [ ] **Tests require an external Debian ISO not in the repo** — `tests/parse.rs:15, 98`
  — gate both tests with `#[ignore]`, or replace with a small synthetic fixture built
  in-process
- [ ] **`replicate()` fails on second run** — `tests/parse.rs:137` — `create_new` errors if
  `out.iso` already exists; use `tempfile`
- [ ] **Dead commented-out code in tests** — `tests/parse.rs:22–62, 106–116` — delete it
- [ ] **`copy_boot_catalog` is dead code** — `tests/parse.rs:184` — delete it
- [ ] **`guide.md`, `plan.md`, `assignment.md` are inside the crate** — will be included in
  published crate tarballs; move them to a location outside `libs/ecma-119/`

---

## Deferred — not needed for UEFI boot, revisit later

- Rock Ridge `SL` (symlinks), `CL`, `PL`, `RE`, `SF` — needed to read Linux ISOs, not to
  write boot images
- `CE` (SUSP continuation area) — same
- Joliet (UCS-2 Supplementary VD) — UEFI firmware does not need it
- Hybrid ISO / protective MBR / embedded ESP — RISC-V is UEFI-only; revisit if USB boot
  on finicky hardware becomes a requirement
