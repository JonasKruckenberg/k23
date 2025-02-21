// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod symbolize;

use arrayvec::ArrayVec;
use core::fmt::Formatter;
use core::{fmt, slice};
use fallible_iterator::FallibleIterator;
use loader_api::BootInfo;
use symbolize::SymbolizeContext;
use sync::{LazyLock, OnceLock};
use unwind2::FrameIter;

static ELF_INFO: OnceLock<ElfInfo> = OnceLock::new();
static SYMBOLIZE_CONTEXT: LazyLock<Option<SymbolizeContext>> = LazyLock::new(|| {
    tracing::debug!("Setting up symbolize context...");
    let state = ELF_INFO.get()?;

    let elf = xmas_elf::ElfFile::new(state.elf).unwrap();
    Some(SymbolizeContext::new(elf, state.kernel_virt_base).unwrap())
});

struct ElfInfo {
    kernel_virt_base: u64,
    elf: &'static [u8],
}

#[cold]
pub fn init(boot_info: &BootInfo) {
    ELF_INFO.get_or_init(|| ElfInfo {
        kernel_virt_base: boot_info.kernel_virt.start as u64,
        // Safety: we have to trust the loaders BootInfo here
        elf: unsafe {
            let base = boot_info
                .physical_address_offset
                .checked_add(boot_info.kernel_phys.start)
                .unwrap() as *const u8;

            slice::from_raw_parts(
                base,
                boot_info
                    .kernel_phys
                    .end
                    .checked_sub(boot_info.kernel_phys.start)
                    .unwrap(),
            )
        },
    });
}

#[derive(Clone)]
pub struct Backtrace<'a, const MAX_FRAMES: usize> {
    symbolize_ctx: Option<&'a SymbolizeContext<'static>>,
    pub frames: ArrayVec<usize, MAX_FRAMES>,
    pub frames_omitted: usize,
}

impl<'a, const MAX_FRAMES: usize> Backtrace<'a, MAX_FRAMES> {
    /// Captures a backtrace at the callsite of this function, returning an owned representation.
    ///
    /// The returned object is almost entirely self-contained. It can be cloned, or send to other threads.
    ///
    /// Note that this step is quite cheap, contrary to the `Backtrace` implementation in the standard
    /// library this resolves the symbols (the expensive step) lazily, so this struct can be constructed
    /// in performance sensitive codepaths and only later resolved.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`unwind2::Error`] if walking the stack fails.
    #[inline]
    pub fn capture() -> Result<Self, unwind2::Error> {
        Self::new_inner(FrameIter::new())
    }

    /// Constructs a backtrace from the provided register context, returning an owned representation.
    ///
    /// The returned object is almost entirely self-contained. It can be cloned, or send to other threads.
    ///
    /// Note that this step is quite cheap, contrary to the `Backtrace` implementation in the standard
    /// library this resolves the symbols (the expensive step) lazily, so this struct can be constructed
    /// in performance sensitive codepaths and only later resolved.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`unwind2::Error`] if walking the stack fails.
    #[inline]
    pub fn from_registers(regs: unwind2::Registers, ip: usize) -> Result<Self, unwind2::Error> {
        let iter = FrameIter::from_registers(regs, ip);
        Self::new_inner(iter)
    }

    fn new_inner(mut iter: FrameIter) -> Result<Self, unwind2::Error> {
        let mut frames = ArrayVec::new();
        let mut frames_omitted: usize = 0;

        while let Some(frame) = iter.next()? {
            if frames.try_push(frame.ip()).is_err() {
                frames_omitted += 1;
            }
        }

        Ok(Self {
            symbolize_ctx: SYMBOLIZE_CONTEXT.as_ref(),
            frames,
            frames_omitted,
        })
    }
}

impl<const MAX_FRAMES: usize> fmt::Display for Backtrace<'_, MAX_FRAMES> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "stack backtrace:")?;

        let mut frame_idx: i32 = 0;
        for ip in &self.frames {
            // if the symbolication state isn't setup, yet we can't print symbols the addresses will have
            // to suffice...
            if let Some(symbolize_ctx) = self.symbolize_ctx {
                let mut syms = symbolize_ctx.resolve_unsynchronized(*ip as u64).unwrap();

                write!(f, "{frame_idx}: {address:#x}    -", address = ip)?;
                while let Some(sym) = syms.next().unwrap() {
                    if let Some(name) = sym.name() {
                        writeln!(f, "      {name}")?;
                    } else {
                        writeln!(f, "      <unknown>")?;
                    }
                    if let Some(filename) = sym.filename() {
                        write!(f, "      at {filename}")?;
                        if let Some(lineno) = sym.lineno() {
                            write!(f, ":{lineno}")?;
                        } else {
                            write!(f, "??")?;
                        }
                        if let Some(colno) = sym.colno() {
                            writeln!(f, ":{colno}")?;
                        } else {
                            writeln!(f, "??")?;
                        }
                    }
                }
            } else {
                writeln!(f, "{frame_idx}: {address:#x}", address = ip)?;
            }

            frame_idx += 1i32;
        }

        if self.symbolize_ctx.is_none() {
            let _ = writeln!(f, "note: backtrace subsystem wasn't initialized, no symbols were printed.");
        }

        Ok(())
    }
}

/// Fixed frame used to clean the backtrace with `RUST_BACKTRACE=1`. Note that
/// this is only inline(never) when backtraces in std are enabled, otherwise
/// it's fine to optimize away.
#[inline(never)]
pub fn __rust_begin_short_backtrace<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let result = f();

    // prevent this frame from being tail-call optimised away
    core::hint::black_box(());

    result
}

/// Fixed frame used to clean the backtrace with `RUST_BACKTRACE=1`. Note that
/// this is only inline(never) when backtraces in std are enabled, otherwise
/// it's fine to optimize away.
#[inline(never)]
pub fn __rust_end_short_backtrace<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let result = f();

    // prevent this frame from being tail-call optimised away
    core::hint::black_box(());

    result
}
