# Cloud & VM Image Formats

[PR #730](https://github.com/JonasKruckenberg/k23/pull/730) gives k23 a real boot
artifact: a UEFI-bootable **ISO-9660** image (`build/mkdisk-img`, built on the
first-party `ecma-119` crate) with an embedded FAT EFI System Partition (ESP)
holding `EFI/BOOT/BOOT{ARCH}.EFI` (the loader, PE-converted by
`build/riscv-elf2efi`), `kernel.elf`, and `kernel.debug`.

An ISO is removable-media (CD) boot. Clouds want a **block device**: a
partitioned disk, in a hypervisor-native container, sometimes wrapped in a
provider envelope. This note scopes what to build to get there.

## 1. Which formats make sense?

The cloud ecosystem splits along **how the guest is loaded**, not along container
format. k23 therefore needs *two* artifact families:

### A. Firmware / disk-image boot (UEFI)
The VMM provides firmware (OVMF/EDK2, AWS Nitro, rust-hypervisor-firmware); the
guest is a **partitioned disk** (GPT + ESP) the firmware boots like bare metal.
This is the natural extension of the PR #730 ESP. Consumers:

| Target | Container | Notes |
|---|---|---|
| QEMU/KVM, libvirt | `qcow2` (or `raw`) | `-bios OVMF.fd -drive`. The everyday dev/CI path. |
| AWS EC2 (Nitro) | stream-optimized `vmdk` / fixed `vhd` / `raw` | UEFI boot mode supported; needs fallback `BOOTX64.EFI` on the ESP. |
| Azure | **fixed** `vhd`, virtual size 1 MiB-aligned | Dynamic VHD is rejected. |
| GCP | `tar.gz` (oldgnu, sparse) of a single `disk.raw` | UEFI guests supported. |
| VirtualBox / VMware (dev) | `vdi` / `vmdk` | Cheap to emit; nice for contributors. |

### B. Direct kernel boot (no firmware)
The VMM loads a kernel binary directly — no ESP, no GPT, no firmware. This is the
**Firecracker / microVM** world and the fast path for cloud-hypervisor and
QEMU's `microvm`/`-kernel`. k23 already does the sibling of this for QEMU
(`-kernel`, FDT in `a1`), so the gap is small.

| Target | Kernel format | Notes |
|---|---|---|
| Firecracker | x86_64: uncompressed **PVH ELF**; aarch64: **PE `Image`** | No UEFI/ACPI on x86 (MPTable + Linux boot ABI). Also needs a raw block device as the boot drive. |
| cloud-hypervisor `--kernel` | PVH ELF / bzImage | Same PVH note as QEMU/Firecracker. |
| QEMU `microvm` / `-kernel` | PVH ELF | Already close to the existing run path. |

**Recommendation — support, in priority order:** `raw` and `qcow2` (family A,
immediately useful on every arch under QEMU); `vhd` (fixed) and `vmdk`
(stream-optimized) plus the GCP `tar.gz` (cloud reach); a **PVH-noted kernel**
for Firecracker/cloud-hypervisor (family B). `vdi` is a near-free add via
qemu-img. Skip OVA (just a tar of VMDK+OVF — wrap on demand) and VHDX (no
advantage over VHD for us).

> **Arch gating:** Firecracker, AWS, GCP and Azure are x86_64/aarch64 only.
> k23's cloud story is therefore gated on the x86_64/aarch64 ports. Family A in
> `raw`/`qcow2` is the *only* part useful on today's riscv64 (QEMU + bare metal).

## 2. Components & transformations to build

The pipeline is **one raw disk → many containers → optional provider envelope**,
which is exactly how `nixpkgs/nixos/lib/make-disk-image.nix` works (build raw,
then `qemu-img convert`). Mirror it.

1. **Raw GPT disk builder — the missing primitive.** Today `mkdisk-img` emits
   ISO-9660. Add a sibling output: a GPT-partitioned `disk.raw` with a single
   ESP partition reusing the *existing* `fatfs` ESP code. The only new piece is a
   small GPT writer (protective MBR + primary/backup GPT headers + partition
   array). In keeping with the repo's first-party-tools ethos (`ecma-119`), write
   it in-tree rather than shelling to `sgdisk`. `ecma-119` stays for the CD path;
   the two share the ESP builder.
2. **Container conversion.** `qemu-img` ships with the already-vendored QEMU
   toolchain. Wrap `qemu-img convert` as a BUCK rule producing `qcow2`,
   `vmdk` (`subformat=streamOptimized`), `vpc` (`subformat=fixed`), and `vdi`
   from the raw image. *Exception:* fixed-VHD is just `raw` + a 512-byte
   `conectix` footer — trivial to emit first-party with exact 1 MiB size
   rounding, sidestepping qemu-img's alignment quirks for Azure. Recommend
   native `raw`+`vhd`, qemu-img for `qcow2`/`vmdk`/`vdi`.
3. **Provider envelopes.**
   - GCP: `tar --format=oldgnu -Sczf image.tar.gz disk.raw` — a one-line genrule.
   - AWS: VMDK/VHD feed VM Import/Export, *or* the faster snapshot path
     (`dd` raw → EBS volume → snapshot → `register-image`, as nixos `amazonImage`
     does). The latter needs credentials, so it lives in out-of-band CI tooling,
     not the hermetic graph.
   - Azure: upload the fixed VHD (page blob).
4. **Direct-boot enablement (family B).** Add the PVH ELF note
   (`XEN_ELFNOTE_PHYS32_ENTRY`) to the kernel image so Firecracker/cloud-hypervisor/
   QEMU-microvm can load it without firmware. This is a kernel/linker change, not
   packaging. Decide k23's "rootfs" contract: Firecracker *requires* a boot drive
   because Linux needs `/`; k23 is the OS, so the drive is just whatever data k23
   chooses to consume — likely an empty or payload raw image. Ship a Firecracker
   VM-config JSON template + boot args alongside.

Wire all outputs through the `k23_image` rule as named sub-targets
(`[raw]`, `[qcow2]`, `[vhd]`, `[vmdk]`, `[gce]`, `[pvh]`) so `buck2 build
//sys:k23-riscv64[qcow2]` Just Works and the graph tracks each artifact.

## 3. Roadmap

- **Phase 0 — raw disk primitive (arch-neutral, do first).** GPT+ESP `disk.raw`
  from `mkdisk-img`; `k23_image` exposes `[raw]`/`[qcow2]`. Immediately yields a
  QEMU+OVMF-bootable qcow2 on riscv64. Highest leverage, no arch dependency.
- **Phase 1 — conversion matrix + GCP envelope (hermetic).** `qcow2`, `vmdk`,
  `vhd`, `vdi`, and the GCP `tar.gz`. Pure transforms over Phase 0; fully
  testable without a cloud account.
- **Phase 2 — direct boot (gated on x86_64/aarch64).** PVH ELF note; Firecracker
  + cloud-hypervisor smoke boots; settle the boot-drive semantics.
- **Phase 3 — real-cloud lanes (gated on x86_64/aarch64).** AWS AMI
  (snapshot→register) and Azure VHD upload behind a credentialed, opt-in CI lane;
  publish artifacts on release. Never in `preflight`.

## 4. Verification

Match the repo's philosophy — hermetic, property-style checks in the gate; real
clouds only in an opt-in lane — and reuse the existing QEMU `.wast` selftest
harness (`just selftests`) as the boot oracle.

- **Structural / round-trip (hermetic, in `preflight`).**
  - `qemu-img check` + `qemu-img info` assert the subformat and (for VHD) the
    1 MiB-aligned virtual size and `conectix`/`fixed` footer.
  - Convert each container *back* to raw and `cmp` against the source raw —
    cheap proof the conversion is lossless. Lends itself to a proptest over disk
    contents.
  - GCP `tar.gz`: assert `tar -tzf` membership is exactly one `disk.raw`.
- **Boot smoke (hermetic).** Boot the artifact under the matching VMM and require
  the selftests to reach the test-exit marker:
  - `raw`/`qcow2` → `qemu-system … -bios OVMF.fd -drive` (UEFI). Extends
    `just selftests` from `-kernel` to a real disk boot — also exercises the
    PR #730 ESP end to end.
  - PVH kernel → `qemu microvm`, `cloud-hypervisor --kernel`, and `firecracker`
    with the JSON template; assert the serial marker.
- **Real cloud (opt-in lane, credentialed).** AWS: `import-image`/snapshot →
  launch a small instance → scrape console output for the marker. Azure/GCP:
  analogous. Nightly or release-triggered, never blocking `preflight`.

## Reference

- NixOS `make-disk-image.nix` (raw → `qemu-img` → qcow2/vdi/vpc) and the
  `amazonImage` / `google-compute-image` / `azure-image` modules are the closest
  prior art for the build-once-convert-many shape.
- Firecracker `docs/rootfs-and-kernel-setup.md`; cloud-hypervisor boot docs;
  Xen PVH `XEN_ELFNOTE_PHYS32_ENTRY` ABI; AWS VM Import/Export requirements.
