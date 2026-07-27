# k23 on the Milk-V Duo — hardware bringup plan

Status: **plan** (no bringup code on this branch yet). Written against `main` @ `54a1af0`.

This document is the result of a research pass over (a) the Milk-V Duo hardware /
firmware ecosystem and (b) the current k23 tree, and lays out a step-by-step plan to
get k23 booting on real Duo boards. File/line references are into this repo at the
commit above; hardware facts are sourced in [Appendix C](#appendix-c--sources).

---

## 1. TL;DR

The single most important finding: **no UEFI work is needed**. `sys/loader-flat/`
already enters via the Linux/SBI boot convention — S-mode, `a0` = hartid, `a1` = DTB
(`sys/loader-flat/src/arch/riscv64.rs:23-36`) — which is byte-for-byte what U-Boot's
`bootm` hands off with, and the Duo's boot chain (BootROM → FSBL → OpenSBI → U-Boot)
ends in exactly that. The flat loader's link address `0x80200000`
(`sys/loader-flat/linkers/riscv64-unknown-qemu.ld:4`) even happens to be correct for
the Duo's DRAM base. k23 is also Sv39-only today, and the Duo's C906 core is
Sv39-only — a happy accident.

What actually stands between k23 and first light, in order of "you will hit this
first":

| # | Blocker | Where |
|---|---------|-------|
| 1 | Loader console + abort + exit are **QEMU semihosting only** (`ebreak` magic) — on hardware the loader dies silently before printing anything | `sys/loader-flat/src/logger.rs:46-53`, `lib/abort/src/lib.rs:35`, `sys/kernel/src/main.rs:89` |
| 2 | `/chosen/rng-seed` is **mandatory**; U-Boot on the Duo won't provide one → hard boot failure before any console | `sys/loader-flat/src/machine_info.rs:45` |
| 3 | UART driver hardcodes stride-1 byte registers; the Duo's DesignWare UART needs `reg-shift=2` / `reg-io-width=4` (values are already parsed and plumbed into `UartInfo`, then dropped) plus the DW busy-detect quirk | `lib/uart-16550/src/lib.rs:44-95`, dropped at `sys/kernel/src/main.rs:131-132` |
| 4 | UART discovery requires `clock-frequency` on the UART node; Sophgo DTs use `clocks = <&clk …>` phandles → `uart = None` → kernel `unwrap()` panic | `sys/loader-common/src/fdt.rs:77`, `sys/kernel/src/main.rs:127` |
| 5 | PLIC driver only matches `sifive,plic-1.0.0` / `riscv,plic0`; the Duo's is `thead,c900-plic` | `sys/kernel/src/arch/riscv64/device/plic.rs:85` |
| 6 | Loader-built PTEs never set A/D bits; the C906 has **no hardware A/D update** → page fault on first access to loader-established mappings | `lib/mem-core/src/arch/riscv64.rs:311-324` |
| 7 | **No `fence.i` anywhere** — JIT-published WASM code will execute stale I-cache lines on real silicon (QEMU's TCG hides this) | publish chain ends at `sys/kernel/src/mem/mmap.rs:259-285` |
| 8 | T-Head **MAEE** non-standard PTE attribute bits (63:59 = SO/C/B/SH/Sec): if firmware enables MAEE, standard PTEs come up non-cacheable → crawling speed and broken AMOs | `lib/mem-core/src/arch/riscv64.rs`, `sys/kernel/src/arch/riscv64/mem.rs` (incl. PPN-mask bug at `:572-577`) |
| 9 | 32 MiB initial heap + 1 GiB physmap leaf are hostile to a 64 MB board | `sys/kernel/src/main.rs:65`, `sys/loader-common/src/lib.rs:306` + `mapping.rs:206` |
| 10 | Build system has an *arch* dimension but no *board* dimension; QEMU-virt is hardwired in the toolchain | `build/toolchains/BUCK:328-331`, `build/constraints/BUCK` |

Every item except 8 is board-agnostic hardening that can be developed and regression-
tested **on QEMU today** — most of the bringup can happen before an SD card is ever
flashed, and QEMU's `-cpu thead-c906` model lets us rehearse item 8 too.

---

## 2. Hardware background

### 2.1 The board family

| Board | SoC | Main core | RAM | Notes |
|---|---|---|---|---|
| **Duo** | Sophgo **CV1800B** | T-Head C906 @ 1 GHz | **64 MB** in-package DDR2 | the original; assumed primary target |
| Duo 256M | Sophgo SG2002 | C906 @ 1 GHz (or A53, pin-selectable) | 256 MB | same peripherals/memory map family |
| Duo S | Sophgo SG2000 | C906 @ 1 GHz (or A53) | 512 MB | bigger form factor |

All three share the cv18xx peripheral map and boot chain, so the plan applies to the
whole family; the 64 MB Duo just additionally forces the memory-budget work (item 9).
If both are on hand, doing *first* light on a 256M and then shrinking onto the 64 MB
Duo is the lower-friction order — but nothing below requires it.

Both SoCs also carry a second C906 (700 MHz, runs FreeRTOS in the vendor stack) and
an 8051 MCU. The second core is **not cache-coherent** with the main core and is not
an SMP peer — which is fine, because k23's current contract is single-CPU at handoff
(`sys/kernel/src/main.rs:186`). Ignore it for bringup; it parks in mask ROM until the
mailbox wakes it.

### 2.2 The C906 core, and how it differs from QEMU `virt` `-cpu rv64`

- **ISA: rv64imafdc** + Zicsr/Zifencei/Zicntr/Zihpm. No bit-manip (Zba/Zbb), no
  Zicbom/Zicboz, no Sstc, no Svpbmt. k23's kernel target JSON asks for exactly
  `+m,+a,+f,+d,+c` (`build/targets/riscv64gc-k23-none-kernel.json`) and Cranelift is
  left at baseline rv64gc defaults (`sys/kernel/src/wasm/engine.rs:40-47`) — **no
  codegen changes needed**. There is a vector unit but it's RVV **0.7.1** —
  unusable from a standard toolchain; must stay off.
- **Sv39 only.** k23 hardcodes Sv39 end-to-end (`sys/loader-common/src/lib.rs:60`,
  `sys/kernel/src/arch/riscv64/mem.rs:53-61`) — no work needed.
- **No hardware A/D update** (Svade behavior): an access through a PTE with A=0 (or a
  write with D=0) raises a page fault instead of setting the bit. The kernel's PTE
  path already sets A|D unconditionally (`sys/kernel/src/arch/riscv64/mem.rs:616-635`);
  the loader's does not (blocker 6).
- **XTheadMae (MAEE)**: T-Head's pre-Svpbmt memory-attribute extension. When the
  M-mode CSR `mxstatus.MAEE` is set by firmware, PTE bits 63:59 become
  SO (strong-order) / C (cacheable) / B (bufferable) / SH (shareable) / Sec. With
  MAEE on and those bits zero, memory is non-cacheable — Linux needed an errata
  (`ERRATA_THEAD_MAE`) to set C/B/SH on normal memory. Detection from S-mode: read
  CSR **`th.sxstatus` (0x5c0)** and test bit 21 (MAEE) — this is what mainline Linux
  does, and QEMU ≥ 9.0 emulates the CSR on `-cpu thead-c906`. Whether we face MAEE
  depends on the firmware in `fip.bin` (vendor OpenSBI historically enables it;
  mainline OpenSBI generic does not touch it) — the kernel should handle both
  (see Phase 5).
- **Split, non-coherent I-cache** (32 KiB I / 64 KiB D): `fence.i` after writing code
  is mandatory (blocker 7). C906 additionally has custom cache-maintenance ops
  (XTheadCmo, `th.dcache.*`) — not needed until DMA-capable drivers appear.
- **PLIC is `thead,c900-plic`**: same register layout as SiFive's, but S-mode access
  is gated behind a delegation bit at PLIC offset `0x1ffffc`. Current mainline
  OpenSBI sets that bit automatically when the DT says `thead,c900-plic`, so k23
  only needs the compatible string (blocker 5) — but if boot hangs on first PLIC
  access, this delegation bit is the suspect.
- **`time` CSR / timebase**: 25 MHz (`timebase-frequency = <25000000>` in the DT).
  k23 reads that from the FDT and uses SBI `set_timer`
  (`sys/kernel/src/arch/riscv64/device/clock.rs`) — the right mechanism for this
  core. 25 MHz divides `NANOS_PER_SEC` exactly (40 ns/tick), so no drift from the
  integer math at `clock.rs:52`.

### 2.3 SoC memory map (CV1800B / SG200x, from the mainline Linux DT)

| Peripheral | Base | Notes |
|---|---|---|
| DRAM | `0x8000_0000` | 64 MB on CV1800B (`0x0400_0000`) |
| UART0 (console) | `0x0414_0000` | `snps,dw-apb-uart`, `reg-shift=2`, `reg-io-width=4`, IRQ 28, clocked at 25 MHz |
| UART1–4 | `0x0415_0000`/`0x0416_0000`/`0x0417_0000`/`0x041c_0000` | IRQs 29–32 |
| PLIC | `0x7000_0000` | `thead,c900-plic`, 101 sources |
| CLINT/mtimer | `0x7400_0000` | M-mode only; reached via SBI |
| SDHCI0 (microSD) | `0x0431_0000` | `sophgo,cv1800b-dwcmshc` — future work |
| pinctrl / clk | `0x0300_1000` / `0x0300_2000` | firmware sets up console pinmux; not needed at first |

### 2.4 Boot chain and media

BootROM loads **`fip.bin`** from the first FAT32 partition of the microSD card.
`fip.bin` is a vendor container packing: **FSBL** (`cv180x.bin`, DDR init — vendor
blob from `sophgo/fsbl`) → **OpenSBI** (`fw_dynamic.bin`) → **U-Boot**
(`u-boot-dtb.bin`). Mainline U-Boot has a `milkv_duo_defconfig` and documents this
flow; the vendor `duo-buildroot-sdk` produces the same layout. U-Boot then loads the
OS payload from the FAT partition (vendor flow: a FIT image named `boot.sd`) and
`bootm` enters it in S-mode with `a0`=hartid, `a1`=DTB.

Consequences for k23:

- We never touch M-mode; OpenSBI is resident and provides TIME/IPI/RFENCE — the same
  SBI surface k23 already uses on QEMU (`lib/riscv/src/sbi/`).
- Our deliverable is a **FIT image** (`k23.itb`) wrapping the flat loader (with the
  kernel already embedded in it, `sys/loader-flat/src/kernel.rs:11-26`) plus **our
  own DTB**, dropped onto the FAT partition next to `fip.bin`.
- We control the DTB inside the FIT, which neutralizes blockers 2 and 4 at the data
  level (we can add `/chosen/stdout-path`, `clock-frequency`, even a static
  `rng-seed`) — though the code-level fixes are still worth doing so arbitrary
  U-Boot DTBs work.

### 2.5 Physical bench setup

- Serial console: **UART0 on GP12 (TX) / GP13 (RX)** — physical pins 16/17 on the
  standard Duo pinout (verify against the pinout diagram for the exact board rev),
  115200 8N1, **3.3 V** levels. Any USB-UART adapter works; cross TX/RX, common GND.
- Power/flash loop: the original Duo has no Ethernet jack and no USB host by
  default, so the realistic iteration loop is *SD-card swap* (or `loady` over
  serial/ymodem for small payloads — ~11 KB/s at 115200, tolerable for a trimmed
  loader, painful for full images). An **SD mux (usbsdmux / SDWire)** plus a
  USB-controlled power switch turns this into a fully scriptable loop and is the
  single best bench investment if several boards are on hand (Phase 7).

---

## 3. Boot strategy decision

**Chosen path: mainline U-Boot + FIT image, extending `loader-flat`.**

Rationale and alternatives considered:

1. **`loader-flat` + `bootm`/FIT (chosen).** Zero new boot-protocol code — the entry
   contract already matches. FIT gives us: our own DTB, a checksummed image, a load
   address (`0x80200000`) and entry point, and identical handling under vendor
   U-Boot (as `boot.sd`) and mainline U-Boot. U-Boot's `bootm` also runs its FDT
   fixups (fills in `/memory`) before handoff, which k23's FDT-driven memory
   discovery (`sys/loader-common/src/fdt.rs:129-178`) consumes as-is.
2. **`loader-efi` via U-Boot's `bootefi`** — viable in principle (U-Boot's EFI
   loader implements the protocols `loader-efi` uses, incl. `RISCV_EFI_BOOT_PROTOCOL`
   since 2022.10), but `milkv_duo_defconfig` does not enable EFI/bootstd, the EFI
   path drags in ESP layout + SimpleFS + more U-Boot config surface, and it buys
   nothing over the flat path here. Keep as a compatibility experiment only
   (Appendix A).
3. **Replace U-Boot entirely (OpenSBI `fw_payload` = loader-flat).** Attractive
   later for boot-time; bad first move — you lose U-Boot's serial recovery,
   `loady`, and FAT loading during the phase where you need them most.

---

## 4. The plan, phase by phase

Phases 1–3 are pure QEMU/desk work and can proceed in parallel with ordering
hardware. Each numbered step is meant to be a separately landable PR with its own
test. Everything must keep `just preflight` green on the existing QEMU lanes.

### Phase 0 — bench sanity (no code)

1. Wire the serial adapter, flash a stock vendor image, confirm FSBL/OpenSBI/U-Boot
   banners at 115200 and note the exact versions in the banners.
2. Interrupt U-Boot autoboot; record `printenv`, `bdinfo`, `fatls mmc 0:1`. This
   pins down: DRAM size as U-Boot sees it, load addresses, whether `bootelf`,
   `booti`, `bootefi` exist in the vendor build, and the FAT layout.
3. Build mainline U-Boot (`milkv_duo_defconfig`) + mainline OpenSBI + vendor FSBL
   into a `fip.bin` (per the mainline U-Boot Sophgo docs / `sophgo/fiptool`), and
   verify the board still reaches the U-Boot prompt with fully non-vendor firmware
   above the FSBL. Keep both `fip.bin`s; the mainline one is the supported target,
   the vendor one is the fallback.
4. From U-Boot, `md 0x04140000` and friends — confirm console UART register access
   behaves as documented (sanity for the `reg-shift=2` claim).

**Done when:** board reaches a mainline U-Boot prompt on our own `fip.bin`.

### Phase 1 — board-agnostic hardening (all testable on QEMU today)

1. **Early console via SBI DBCN with runtime fallback.** Teach the flat loader's
   logger (`sys/loader-flat/src/logger.rs`) and `lib/abort` to probe SBI extensions
   once (BASE `probe_extension`) and use **DBCN `console_write`** when present,
   falling back to the existing semihosting path (QEMU's bundled OpenSBI supports
   DBCN, so QEMU lanes keep working; so does any OpenSBI ≥ 1.2 on hardware). The
   wrapper already exists unused: `lib/riscv/src/sbi/dbcn.rs`. Same treatment for
   the exit path: prefer SBI SRST for shutdown/reset when probed, keep semihosting
   `exit()` for the QEMU test harness (`sys/kernel/src/main.rs:88-89`,
   `sys/kernel/src/tests/mod.rs:65-74`).
2. **Make `/chosen/rng-seed` optional** (`sys/loader-flat/src/machine_info.rs:45`):
   if absent, log a loud warning and derive a poor-entropy fallback (mix `rdtime`,
   hartid, FDT address) — KASLR quality degrades, boot does not. Keep the DTB-side
   seed as the recommended path (our FIT DTB can carry one; U-Boot can also be
   taught `kaslr-seed` later).
3. **UART stride + DW quirk.** Extend `uart_16550::open` to take (or a new `Config`
   struct carrying) `reg_shift`/`reg_io_width` — already parsed
   (`sys/loader-common/src/fdt.rs:86-87`), already in the boot-info ABI
   (`sys/loader-api/src/info.rs:87-91`), just dropped at the call site
   (`sys/kernel/src/main.rs:131-132`). Register access becomes
   `base + (reg << shift)` with 1- or 4-byte volatile width, per invariant 1. Add
   the DesignWare **busy quirk**: read `USR` (offset `0x1f << shift`) before LCR
   writes, and treat IIR value `0x07` (busy interrupt) as "clear via USR read".
   Property-test the register-offset math; the QEMU lane (shift=0/width=1) is the
   regression test for the default path.
4. **PLIC compatibles + hygiene.** Add `"thead,c900-plic"` (and
   `"sophgo,cv1800b-plic"`) to the match at
   `sys/kernel/src/arch/riscv64/device/plic.rs:85`. While there: replace the
   `.unwrap()`/`.expect()` fatal paths on DT lookups with proper errors so a missing
   PLIC degrades to "no external interrupts" instead of a silent hang, and map the
   PLIC MMIO as device memory once the attribute plumbing from Phase 5 exists.
5. **`fence.i` after JIT publish.** Add `fence.i` (and an SBI
   `remote_fence_i` wrapper in `lib/riscv/src/sbi/rfence.rs` for the future SMP
   case) to the code-publish path — the clean cut point is
   `Mmap::make_executable` / the `publish()` in
   `sys/kernel/src/wasm/vm/code_object.rs:68-83`. This is a latent correctness bug
   independent of the Duo; QEMU just never punishes it.
6. **Loader PTE A/D bits.** Set ACCESSED|DIRTY on leaf PTEs in
   `lib/mem-core/src/arch/riscv64.rs::new_leaf` (`:311-324`), matching what the
   kernel path already does and what the RISC-V spec recommends for kernels that
   don't use A/D. Required for C906 (no hardware A/D update); harmless everywhere.
7. **Kernel PTE upper-bit hygiene (prerequisite for MAEE/Svpbmt).** Fix
   `get_address_and_flags` to mask PPN bits properly (the `// TODO correctly mask
   out address` at `sys/kernel/src/arch/riscv64/mem.rs:572-577`) and make
   `replace_address_and_flags` preserve attribute bits it doesn't own
   (`:568-570`); reconcile the `from_bits_retain` vs `from_bits_truncate`
   inconsistency. Property-test round-tripping PTEs with bits 63:54 set.
8. **Memory budget knobs.** Shrink `INITIAL_HEAP_SIZE_PAGES`
   (`sys/kernel/src/main.rs:65`) to something 64 MB-friendly (e.g. 4 MiB) and let
   the existing `--heap-max` bootarg govern growth; size the physmap mapping to the
   actual DRAM span with 2 MiB leaves instead of one 1 GiB leaf over mostly
   nonexistent address space (`sys/loader-common/src/lib.rs:306`,
   `mapping.rs:206`) — on real silicon a speculative touch into unbacked bus
   territory can hang, and on MAEE parts a 1 GiB leaf can't express per-range
   attributes anyway.

**Done when:** `just preflight` green; QEMU flat lane boots with DBCN console (no
semihosting), with `rng-seed` deleted from the DTB, and WASM selftests still pass.

### Phase 2 — QEMU rehearsal of the C906

QEMU (≥ 9.0) models this exact core: `-cpu thead-c906` including the `th.sxstatus`
CSR and XTheadMae. Add a second QEMU flat target (e.g. `//sys:k23-flat-qemu-c906`)
running `-machine virt -cpu thead-c906`, and make it part of local selftests.

Expected fallout to fix here, cheaply: anything assuming extensions the C906 lacks;
`riscv,isa-extensions` being absent (QEMU/older DTs emit only the legacy
`riscv,isa` string — add the legacy parser fallback at
`sys/kernel/src/arch/riscv64/device/cpu.rs:77-83`); MAEE detection plumbing from
Phase 5 once it exists.

**Done when:** full selftest suite passes under `-cpu thead-c906`.

### Phase 3 — board dimension in the build + image packaging

The build system currently switches only on CPU arch; QEMU-virt is implicit
(`build/toolchains/BUCK:328-331`). Introduce the board axis the same way the
existing constraint settings work (`build/constraints/BUCK`):

1. New constraint setting `board` (values `qemu-virt` (default), `milkv-duo`) and a
   platform `//platforms:riscv64-milkv-duo`.
2. Key the flat linker script by board (`sys/loader-flat/BUCK:3-6,30-33` becomes a
   `select()`); the Duo script can start as a copy of the QEMU one — `0x80200000`
   is correct for this board — the point is the mechanism.
3. **Raw binary + FIT rule.** New action modeled on `build/split_debuginfo.bzl`
   running `llvm-objcopy -O binary` on the flat loader, then a FIT packer
   (`mkimage` from `ubootTools` + `dtc`, both nix-provided; `dtc` is already in the
   devshell) producing `k23.itb` with: loader binary (type=kernel, os=linux,
   arch=riscv, load=entry=`0x80200000`, sha256) + our DTB. New
   `K23BootInfo.protocol = "fit"` alongside `uefi`/`flat` (`build/qemu.bzl:25-48`).
4. **Board DTB in-tree.** Start from mainline Linux's `cv1800b-milkv-duo.dts`
   (dual-licensed GPL-2.0 OR MIT — take the MIT branch, keep attribution), trimmed
   to what k23 consumes, augmented with: `/chosen/stdout-path = "serial0:115200n8"`,
   `clock-frequency = <25000000>` on the UARTs, `timebase-frequency`, optional
   `rng-seed`. Build with `dtc` as part of the FIT rule.
5. `just` glue: `just duo-image` producing `fip.bin`-adjacent artifacts and a
   documented `dd`/copy recipe; keep firmware (`fip.bin`) out of the tree — document
   how to build it, or cache it as a fetched artifact.

**Done when:** `just platform=//platforms:riscv64-milkv-duo build //sys:k23-duo`
emits `k23.itb` reproducibly, and the QEMU lanes are untouched.

### Phase 4 — first light on hardware

Copy `k23.itb` next to `fip.bin` on the FAT partition; from U-Boot:

```
fatload mmc 0:1 0x81000000 k23.itb
bootm 0x81000000
```

(Then bake this into `bootcmd` / a `boot.scr`.) Note the FIT must be loaded high
enough (`0x8100_0000` leaves 14 MB for the unpacked payload at `0x8020_0000`; adjust
against actual image size — on a 64 MB board the top of DRAM is `0x8400_0000`).

Milestone ladder, with the diagnostic tool for each rung:

1. **DBCN characters from the loader** (first `log` line in
   `sys/loader-flat/src/main.rs`). Failure here = entry/ABI/load-address problem →
   use U-Boot `md`/`go` and OpenSBI's boot banner; add a debug putchar loop at
   `_start` if needed.
2. **Loader completes mapping, prints handoff log.** Failure = FDT parsing (dump
   the exact DTB U-Boot passed: `fdt addr`, `fdt print` in U-Boot) or frame-alloc
   over the 64 MB budget.
3. **`csrw satp` survives** (first instruction after the trampoline,
   `sys/loader-common/src/arch/riscv64.rs:85-109`). Failure = PTE format (A/D
   bits, MAEE — jump to Phase 5), or trampoline identity map.
4. **Kernel banner on UART0 via the 16550 driver** — first real console output
   (`sys/kernel/src/main.rs:127-135`). Failure = reg-shift/busy-quirk (Phase 1.3)
   or UART clock (should be 25 MHz from our DTB).
5. **Timer tick + PLIC init + kasync runs** (Phase 6 territory).

Practical iteration: keep two SD cards in rotation, or `loady` a fresh `k23.itb`
over serial while the FAT partition stays untouched.

**Done when:** kernel tracing banner appears on the serial console.

### Phase 5 — C906 memory attributes (MAEE) done right

Decide behavior by **runtime detection**, mirroring mainline Linux:

1. At loader entry, read `th.sxstatus` (CSR `0x5c0`; wrap in
   `lib/riscv/src/register/`) and test bit 21 (MAEE). Guard with a
   vendorid check (`mvendorid` is M-mode, but SBI `base` exposes
   `sbi_get_mvendorid` — T-Head is `0x5b7`) so we never touch a T-Head CSR on
   other cores.
2. If MAEE is **off** (mainline OpenSBI leaves it off): standard Sv39 PTEs,
   cacheability comes from the SoC's PMAs — nothing to do beyond Phase 1.6/1.7.
3. If MAEE is **on** (vendor firmware): leaf PTEs must carry attribute bits —
   normal memory gets C|B|SH, MMIO gets SO (strong-order, non-cacheable). Plumbing:
   - `lib/mem-core/src/arch/riscv64.rs`: the bitfield already reserves 61:62 for
     Svpbmt-style `PBMT` and 63 for NAPOT; add an alternate MAEE field set
     (63:59 = SO/C/B/SH/Sec) selected at runtime, and finally honor
     `MemoryAttributes::KIND` in `new_leaf` (today the UART's `MemoryKind::Device`
     is computed at `sys/loader-common/src/mapping.rs:242` and then discarded).
   - Kernel side (`sys/kernel/src/arch/riscv64/mem.rs`): after the Phase 1.7
     hygiene fixes, add the same attribute encoding, and give
     `crate::mem::Permissions`/mapping paths a device-memory notion so the PLIC
     mapping (`plic.rs:124`) stops being "normal cacheable".
4. Rehearse both branches in QEMU: `-cpu thead-c906` (MAEE on, per QEMU's model) vs
   `-cpu rv64` (no MAEE), same kernel binary.

Note even in the MAEE-off case, keep MMIO access volatile + fenced per invariants
1–2 — PMAs make the UART/PLIC windows non-cacheable on this SoC, so correctness
holds, but the attribute plumbing is still the long-term right shape (it becomes the
Svpbmt implementation on future boards).

**Done when:** same binary boots with vendor `fip.bin` (MAEE on) and mainline
`fip.bin` (MAEE off), with normal-memory performance (a trivial memcpy benchmark in
the shell distinguishes cached from uncached instantly — ~100× difference).

### Phase 6 — interrupts, timer, WASM

1. Timer: should Just Work via SBI TIME + FDT timebase (25 MHz). Validate `rdtime`
   monotonicity and wakeup latency (if `rdtime` traps-and-emulates on this core the
   cost is ~hundreds of cycles — measure, since `kasync`'s timer wheel leans on it).
2. PLIC: enable UART0 RX IRQ (28) as the first external interrupt; verify
   claim/complete against the c900 delegation quirk (§2.2). The kernel shell over
   serial is the natural test.
3. WASM: run the `.wast` selftest suite on-target. This exercises JIT publish +
   `fence.i` (Phase 1.5), trap handling (`stval`/`sepc` precision on real silicon),
   guard-page faults, and the 64 MB memory budget end to end. Expect to tune heap
   grow chunk (`sys/kernel/src/allocator.rs:33`) and possibly WASM memory
   reservation policy on the 64 MB board.

**Done when:** interactive shell over serial; WASM selftests pass on-target.

### Phase 7 — repeatable testing (turn the board into a CI lane)

1. **Serial sentinel protocol**: the selftest runner currently signals pass/fail
   via semihosting exit codes only (`sys/kernel/src/tests/mod.rs:65-74`). Add an
   unambiguous serial sentinel (`k23-selftests: PASSED/FAILED (n passed, m
   failed)`) emitted regardless of exit mechanism, plus SBI SRST reset attempt at
   the end.
2. **Host-side runner**: small harness (nix devshell already ships `socat`) that
   power-cycles the board, writes the SD via usbsdmux/SDWire, watches serial for
   sentinel or timeout. Wire as an opt-in `just duo-selftests` — hardware-attached,
   not in default CI.
3. **CI proxy lane**: the `-cpu thead-c906` QEMU target from Phase 2 goes into the
   regular selftest matrix (`justfile:107`, `.github/workflows/ci.yml` selftest
   rows) as the always-on guard for C906-specific regressions.

### Phase 8 — beyond bringup (deliberately out of scope, noted for direction)

- **SD/MMC driver** (`sophgo,cv1800b-dwcmshc`, base `0x0431_0000`) so guests/WASM
  payloads can load from storage rather than being baked into the image — first
  real DMA device, which is what forces the XTheadCmo cache-maintenance and
  DMA-API work.
- Ethernet pads / USB gadget serial; the little C906 + mailbox; PSCI-less second
  board variants (SG2002 A53 mode is a different arch lane entirely).
- Boot-time: replace U-Boot with OpenSBI `fw_payload`-embedded loader once the
  bringup crutches (serial recovery, FAT loading) are no longer needed.
- Upstream doc fixes discovered during research: `AGENTS.md`, `platforms/README.md`
  and `manual/src/building/*` still reference the pre-split `sys/loader/` and a
  nonexistent `//sys:k23-riscv64-qemu` target; `manual/src/arch/riscv/memory_layout.md`
  and `manual/src/aslr.md` describe a fixed/randomized layout the loader doesn't
  implement (actual layout: bump-allocated from `0xffffffc000000000`,
  `sys/loader-common/src/lib.rs:296-336`). Worth a standalone cleanup PR early so
  the docs stop misleading bringup work.

---

## 5. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| Vendor OpenSBI quirks (MAEE on, PLIC delegation differences, missing DBCN on old builds) | Medium | Prefer mainline `fip.bin` (Phase 0.3); runtime-detect MAEE; DBCN probe falls back gracefully |
| 64 MB is too tight for kernel + heap + WASM suite | Medium | Phase 1.8 knobs; run suite subsets; 256M variant as relief valve |
| DW UART busy quirk subtler than documented (silent LCR write drops) | Medium | U-Boot `md`-level experiments in Phase 0.4; init UART fully from scratch rather than trusting inherited state |
| `rdtime` trap-emulation cost distorts kasync timing | Low-Med | Measure in Phase 6.1; batch time reads if needed |
| Unmapped/speculative bus access hangs (1 GiB physmap leaf) | Low after Phase 1.8 | Physmap sized to DRAM |
| QEMU `thead-c906` model diverges from silicon (it models MAEE but not e.g. PMA layout, DW UART) | Certain but bounded | Treat QEMU lane as regression guard, not proof; hardware sentinel lane (Phase 7) is the truth |

## Appendix A — UEFI-via-U-Boot experiment (optional)

U-Boot's EFI loader can host `loader-efi` (`BOOTRISCV64.EFI` + `EFI\k23\kernel.elf`
on the FAT partition, `bootefi bootmgr` or `bootefi $addr $fdt`), including the
`RISCV_EFI_BOOT_PROTOCOL` boot-hartid handoff that `sys/loader-efi/src/machine_info.rs:160-183`
uses. Requires a custom U-Boot build with `EFI_LOADER`/bootstd enabled (not in
`milkv_duo_defconfig`). Worth a one-day experiment purely to validate `loader-efi`
against a second UEFI implementation besides EDK2 — not on the critical path.

## Appendix B — key constants cheat sheet

```
DRAM             0x8000_0000 + 64 MiB (CV1800B)
loader link/load 0x8020_0000  (sys/loader-flat/linkers/riscv64-unknown-qemu.ld:4)
FIT staging      0x8100_0000  (suggested; adjust to image size)
UART0            0x0414_0000  DW-APB, reg-shift 2, io-width 4, IRQ 28, clk 25 MHz
PLIC             0x7000_0000  thead,c900-plic, 101 sources, S-mode delegate @ +0x1ffffc
timebase         25_000_000 Hz (40 ns/tick)
th.sxstatus      CSR 0x5c0, MAEE = bit 21 (S-mode readable)
MAEE PTE bits    63=SO 62=C 61=B 60=SH 59=Sec  (vs Svpbmt PBMT = bits 62:61)
T-Head mvendorid 0x5b7
console          115200 8N1, 3.3 V, GP12/GP13 (UART0 TX/RX)
```

## Appendix C — sources

- [Mainline U-Boot: Milk-V Duo board docs](https://docs.u-boot.org/en/latest/board/sophgo/milkv_duo.html) ([source](https://github.com/u-boot/u-boot/blob/master/doc/board/sophgo/milkv_duo.rst)) — boot chain, `milkv_duo_defconfig`, fip flow
- [sophgo/fiptool](https://github.com/sophgo/fiptool) — `fip.bin` packing
- [Milk-V duo-buildroot-sdk](https://github.com/milkv-duo/duo-buildroot-sdk) — vendor firmware stack
- Mainline Linux DTs: `arch/riscv/boot/dts/sophgo/{cv1800b.dtsi,cv180x.dtsi,cv180x-cpus.dtsi}` — memory map, UART/PLIC properties, `riscv,sv39`, 25 MHz timebase
- [OpenSBI: automatic T-Head PLIC S-mode delegation](https://github.com/riscv-software-src/opensbi/commit/78c2b19218bd62653b9fb31623a42ced45f38ea6) — `0x1ffffc` delegation bit
- [Linux: "Test th.sxstatus.MAEE bit before enabling MAEE"](https://git.zx2c4.com/linux-rng/commit/arch?id=6beb6bc5a81e1433a1534e75173f67d42a6f225a) — CSR 0x5c0 / bit 21 detection
- [QEMU: th.sxstatus CSR emulation](https://www.mail-archive.com/qemu-devel/msg1038679.html) and [xtheadmaee for thead-c906](https://www.mail-archive.com/qemu-devel@nongnu.org/msg1033672.html) — `-cpu thead-c906` rehearsal viability
- [Milk-V Duo docs](https://milkv.io/docs/duo/overview) / [community: UART pinmux](https://community.milkv.io/t/use-all-uart-pinmux-uart-and-serial/791) — console wiring, board variants
- [Linux mainline CV1800B clk](https://lwn.net/Articles/956178/), [MMC support](https://patchew.org/linux/20240217144202.3808-1-jszhang@kernel.org/) — ecosystem maturity for Phase 8
