# ecma-119 TODO List
> Scoped to the primary use case: building bootable UEFI ISO images for RISC-V.
> Issues are ordered within each section by impact.

---

## Silent correctness bugs — will produce broken images

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

## Code quality — clean up when convenient

- [ ] **`pad_to_sector` calls `stream_position()` in tight write loop** — `build/layout.rs:367`
  — track write offset explicitly or use a `CountingWriter` wrapper

---

## Simplifications — readability, maintainability, speed

### Data model / representations that fight the borrow checker

- [ ] **`DirNode::sorted_entry_ids` stores every identifier twice** —
  `build/layout.rs:36`. The `Cow<str>` in each entry duplicates the name already
  held by the child `DirNode` or `FileNode`. After the BTreeMap change (or even
  without it), store only `DirNodeEntry` and look up the name via the index.
  Saves an allocation per entry and kills the `subdir_name.clone()` at line 159.

### API clarity

- [ ] **`DirectoryEntry::identifier` / `recorded_at`** — `parse/directory.rs:85–97`.
  Both variants forward to the inner `record`. Lift `record: DirectoryRecord<'a>`
  onto `DirectoryEntry` directly and keep `is_dir`/`is_file` as a single flag check;
  the `Directory` / `File` wrappers only matter for `entries()` / `as_slice()`.

### Micro / speed

- [ ] **El Torito validation-entry checksum uses `chunks_exact(2)` + `u16::from_le_bytes`**
  — `build/layout.rs:656–660`. Since `ValidationEntry` is `IntoBytes` and
  sector-aligned, cast once via `zerocopy` to `&[U16<LE>]` and fold. Minor but
  tidier, and avoids the per-chunk bounds check.
- [ ] **`FileNode::new` eagerly calls `source.len()`** — `build/layout.rs:56–63`.
  Fine today, but once `add_file` validates `u32` fit (see API-design section),
  store `len: u32` on the node directly and drop the `len: usize` field's
  silent truncation at `build/directory.rs:24`.

---

## Spec compliance — not blockers for UEFI boot but produce non-compliant images

- [ ] **All directory record timestamps are zero**
  `build/layout.rs:398`
  `build_dir_record_header` sets `recording_date` to all zeros. Set it to the current time.
  Tools like `xorriso --check_media` flag this; it won't affect UEFI firmware.

- [ ] **`BootRecord.boot_id` not validated**
  `raw.rs:817`
  El Torito spec requires `boot_id` to be all zeros. The current `Validate for BootRecord`
  only checks `boot_system_id`.

- [ ] **`MULTI_EXTENT` flag not handled**
  `parse/directory.rs:232`
  Files split across multiple extents yield separate directory entries instead of being
  reassembled. Add a layer that stitches them together before returning to the caller.

---

## Deferred — not needed for UEFI boot, revisit later

- Rock Ridge — needed to read Linux ISOs, not to write boot images
- Joliet (UCS-2 Supplementary VD) — UEFI firmware does not need it
- Hybrid ISO / protective MBR / embedded ESP — RISC-V is UEFI-only; revisit if USB boot
  on finicky hardware becomes a requirement
