// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Throwaway allocation tracer.
//!
//! Captures `(size, align, callstack)` for every successful allocation through
//! the global allocator into a fixed in-BSS ring buffer. Used to build the
//! per-allocation-type arena inventory; not intended to ship.
//!
//! Output is written one record per line in the form
//! `ALLOC,<size>,<align>,<pc0>[,<pc1>...]` to be symbolized host-side.

use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::backtrace::Backtrace;

const MAX_FRAMES: usize = 8;
const RING_CAP: usize = 32 * 1024;

#[derive(Copy, Clone)]
struct Record {
    size: u32,
    align: u16,
    nframes: u8,
    _pad: u8,
    frames: [u64; MAX_FRAMES],
}

const EMPTY: Record = Record {
    size: 0,
    align: 0,
    nframes: 0,
    _pad: 0,
    frames: [0; MAX_FRAMES],
};

#[repr(transparent)]
struct Ring(UnsafeCell<[Record; RING_CAP]>);
// Safety: all reads/writes are gated through `IN_TRACE`, which acts as an
// exclusive lock across CPUs and serves as a re-entrancy guard.
unsafe impl Sync for Ring {}

static RECORDS: Ring = Ring(UnsafeCell::new([EMPTY; RING_CAP]));
static HEAD: AtomicUsize = AtomicUsize::new(0);
static WRAPPED: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicUsize = AtomicUsize::new(0);

/// Becomes the only critical-section primitive: a global try-lock. Losers
/// drop their sample rather than block. Also serves as the re-entrancy guard
/// when `Backtrace::capture` (or anything we call) ends up allocating.
static IN_TRACE: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

pub fn disable() {
    ENABLED.store(false, Ordering::Release);
}

#[inline]
pub fn record(layout: Layout) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if IN_TRACE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }

    do_record(layout);

    IN_TRACE.store(false, Ordering::Release);
}

#[inline(never)]
fn do_record(layout: Layout) {
    let mut frames = [0u64; MAX_FRAMES];
    let mut nframes: u8 = 0;
    if let Ok(bt) = Backtrace::<MAX_FRAMES>::capture() {
        for &ip in &bt.frames {
            if (nframes as usize) >= MAX_FRAMES {
                break;
            }
            frames[nframes as usize] = ip as u64;
            nframes += 1;
        }
    }

    let rec = Record {
        size: u32::try_from(layout.size()).unwrap_or(u32::MAX),
        align: u16::try_from(layout.align()).unwrap_or(u16::MAX),
        nframes,
        _pad: 0,
        frames,
    };

    let idx = HEAD.load(Ordering::Relaxed);
    let next = idx + 1;
    let (next, wrap) = if next >= RING_CAP {
        (0, true)
    } else {
        (next, false)
    };

    // Safety: IN_TRACE is currently held, so no other writer (and no concurrent
    // reader from `dump`) can be in the records array.
    unsafe {
        (*RECORDS.0.get())[idx] = rec;
    }
    HEAD.store(next, Ordering::Relaxed);
    if wrap {
        WRAPPED.store(true, Ordering::Relaxed);
    }
}

/// Dump the ring to the log. Call from a single quiescent context; concurrent
/// allocations that race the dump will simply be dropped via `IN_TRACE`.
pub fn dump() {
    // Acquire the global guard. Spin briefly if a sample is mid-flight; the
    // critical section is bounded.
    while IN_TRACE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }

    let head = HEAD.load(Ordering::Relaxed);
    let wrapped = WRAPPED.load(Ordering::Relaxed);
    let dropped = DROPPED.load(Ordering::Relaxed);
    let total = if wrapped { RING_CAP } else { head };

    log::info!(
        "alloc_trace BEGIN total={} wrapped={} dropped={}",
        total,
        wrapped,
        dropped
    );

    // Safety: IN_TRACE is held; concurrent writers bail and drop their sample.
    let recs: &[Record; RING_CAP] = unsafe { &*RECORDS.0.get() };

    if wrapped {
        for i in head..RING_CAP {
            print_rec(&recs[i]);
        }
        for i in 0..head {
            print_rec(&recs[i]);
        }
    } else {
        for i in 0..head {
            print_rec(&recs[i]);
        }
    }

    log::info!("alloc_trace END");

    IN_TRACE.store(false, Ordering::Release);
}

fn print_rec(r: &Record) {
    let mut buf = [0u8; 512];
    let mut w = StackWriter { buf: &mut buf, len: 0 };
    let _ = write!(&mut w, "ALLOC,{},{}", r.size, r.align);
    for i in 0..r.nframes as usize {
        let _ = write!(&mut w, ",{:#x}", r.frames[i]);
    }
    let len = w.len;
    let s = core::str::from_utf8(&buf[..len]).unwrap_or("<bad>");
    log::info!("{}", s);
}

struct StackWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Write for StackWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let avail = self.buf.len() - self.len;
        let n = bytes.len().min(avail);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        if n < bytes.len() { Err(fmt::Error) } else { Ok(()) }
    }
}
