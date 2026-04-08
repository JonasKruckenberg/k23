## ecma-119 Audit — Issues by Priority

### TODOs

- Implement validation (trait? TryFromBytes?) that allows validation of inputs, should be optional and controlled by a flag
- Implement variable len DStr and FileIdentifier 

### Showstoppers
1. **El Torito serialization not implemented** — `layout.rs:241`: `serialize()` immediately returns `Err` if boot config is present. Nothing boots.
2. **`File::size()` returns LBA, not file size** — `directory.rs:50`: `self.record.header.extent_lba.get()` should be `data_length.get()`.
3. **Extension peek consumes before checking** — `eltorito/parse.rs:144`: calls `parser.read()` then conditionally rewinds state without rewinding the parser. Catalog misread from that point on.
4. **`BootEntryBuilder` modifier methods are uncallable** — `build/boot.rs:109`: `no_emulation(mut self) -> Self` takes by value, but `default_entry()` / `entry()` return `&mut BootEntryBuilder`. Change to `&mut self -> &mut Self`.

### Correctness
5. **`ParseError` / `BuildError` are empty enums** — you can never construct one, so every error site uses `panic!` / `unwrap()` instead. Add real variants.
6. **Parser panics on any malformed input** — `parser.rs:63,93,98`, `eltorito/parse.rs:82-84`, `parse/mod.rs:76`: `assert!`, `panic!`, `unwrap()` throughout. Must return errors instead.
7. **`standard_id` and `volume_descriptor_version` never validated** — `parser.rs:68`: arbitrary binary data parses silently as an ISO.
8. **`BothEndianU16/U32` never validates LE == BE** — `raw.rs:27-35`: silently reads one side; mismatch on corrupted images goes undetected.
9. **Boot catalog slice is unbounded** — `eltorito/parse.rs:28`: passes entire rest of image as catalog; iterator can walk into file data.
10. **`DirEntryIter` can underflow** — `directory.rs:122`: `header.len - 33 - file_identifier_len` underflows if `header.len` is too small. Validate first.
11. **`FileFlags::from_bits().unwrap()` panics on reserved bits** — `raw.rs:172`: use `from_bits_truncate()`.
12. **`MULTI_EXTENT` flag not handled** — `directory.rs:134`: multi-extent files yield separate entries instead of being reassembled.

### Spec Compliance
13. **No `;1` version suffix on filenames** — ISO 9660 Level 1 requires `NAME.EXT;1`. Some firmware requires it.
14. **No filename character set enforcement** — Level 1 requires uppercase d-characters, 8.3 format. Builder accepts anything.

### API Design
15. **`ImageBuilder` can't set any PVD string fields** — `build/mod.rs:21`: volume ID, system ID, creation date, etc. are all zeroed. Bootloaders show the volume label.
16. **`root_at()` and `open()` are `pub` but `todo!()`** — `parse/mod.rs:94,99`: delete them or hide them.
17. **`_path_table_le()` / `_path_table_be()` are underscore-prefixed but `pub`** — `parse/mod.rs:148,159`: make them private or rename them.
18. **`DirEntryIter` exposes `.` and `..`** — every caller filters them manually; skip by default.
19. **`BootPlatform` / `EmulationType` missing `PartialEq`, `Eq`, `Clone`, `Copy`** — `eltorito/mod.rs:15,25`.
20. **`FileSource` missing `is_empty()`** — minor; `len() == 0` is a common check.

### Code Quality
21. **`SectionHeaderEntry::entries` and `ValidationEntry::checksum` are plain `u16`** — `eltorito/raw.rs:52,15`: should be `U16<LittleEndian>`; wrong on big-endian hosts.
22. **`mycelium-bitfield` declared but unused** — `Cargo.toml:9`: dead dependency.
23. **Tests require an external ISO not in the repo** — `tests/parse.rs:13`: both tests crash (not skip) if the file is absent. Gate with `#[ignore]` or use a synthetic fixture.
24. **`replicate()` creates `./out.iso` and never cleans up** — fails on second run (`create_new`). Use `tempfile`.
25. **Large blocks of dead commented-out code** — `tests/parse.rs:22-59, 106-116`: delete it.
26. **`serialize_dir_extent()` is O(n²) per directory** — `layout.rs:496`: linear scan of children and files for each sorted entry. Use pre-built maps.
27. **`required_extent_size()` and `write_dir_record()` must stay in sync** — `layout.rs:74,436`: if one changes without the other, the image is silently corrupted. Factor into a shared helper.
28. **`parse/` uses `std::` instead of `core::`** — `parse/mod.rs:8`, `path_table.rs:1`: blocks `no_std + alloc` support.
29. **`lib.rs` re-exports all of `raw`** — exposes internal types as public API; impossible to change later.
30. **`guide.md`, `plan.md`, `assignment.md` live inside the crate** — will appear in published tarballs; move them outside the crate directory.
