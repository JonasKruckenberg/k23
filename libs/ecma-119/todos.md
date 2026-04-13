## ecma-119 Audit — Issues by Priority

### TODOs

- Implement variable len DStr and FileIdentifier 

### Showstoppers
1. **El Torito serialization not implemented** — `layout.rs:241`: `serialize()` immediately returns `Err` if boot config is present. Nothing boots.

### Correctness
6. **Parser panics on any malformed input** — `parser.rs:63,93,98`, `eltorito/parse.rs:82-84`, `parse/mod.rs:76`: `assert!`, `panic!`, `unwrap()` throughout. Must return errors instead.
12. **`MULTI_EXTENT` flag not handled** — `directory.rs:134`: multi-extent files yield separate entries instead of being reassembled.

### Spec Compliance
13. **No `;1` version suffix on filenames** — ISO 9660 Level 1 requires `NAME.EXT;1`. Some firmware requires it.

### API Design
15. **`ImageBuilder` can't set any PVD string fields** — `build/mod.rs:21`: volume ID, system ID, creation date, etc. are all zeroed. Bootloaders show the volume label.
18. **`DirEntryIter` exposes `.` and `..`** — every caller filters them manually; skip by default.
20. **`FileSource` missing `is_empty()`** — minor; `len() == 0` is a common check.

### Code Quality
23. **Tests require an external ISO not in the repo** — `tests/parse.rs:13`: both tests crash (not skip) if the file is absent. Gate with `#[ignore]` or use a synthetic fixture.
24. **`replicate()` creates `./out.iso` and never cleans up** — fails on second run (`create_new`). Use `tempfile`.
25. **Large blocks of dead commented-out code** — `tests/parse.rs:22-59, 106-116`: delete it.
27. **`required_extent_size()` and `write_dir_record()` must stay in sync** — `layout.rs:74,436`: if one changes without the other, the image is silently corrupted. Factor into a shared helper.
30. **`guide.md`, `plan.md`, `assignment.md` live inside the crate** — will appear in published tarballs; move them outside the crate directory.
