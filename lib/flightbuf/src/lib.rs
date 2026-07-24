// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! A flight recorder: a ring of fixed-size chunks, written in place, that keeps the *most recent*
//! records rather than the oldest.
//!
//! The producer never waits on a reader — out of chunks, it overwrites the oldest. That is what
//! makes a hung kernel worth dumping; a queue is nearly empty in healthy operation. A [`Reader`] is
//! a cursor, not a consumer: it holds nothing back and finds out *afterwards* that it was lapped.
//!
//! [`Reader::read`] is a [seqlock]: copy the chunk out, re-read `head`, discard if the producer took
//! that slot mid-copy. A torn chunk is never delivered — but the copy itself races the producer,
//! which is inherent to an overwrite ring and why it uses
//! [`read_volatile`](core::ptr::read_volatile). A stopped target has no producer, so nothing races.
//!
//! [seqlock]: https://en.wikipedia.org/wiki/Seqlock

#![cfg_attr(not(test), no_std)]

mod loom;
#[cfg(test)]
mod tests;

use core::marker::PhantomData;

use util::CachePadded;

use crate::loom::cell::{ConstPtr, MutPtr, UnsafeCell};
use crate::loom::sync::atomic::{AtomicU32, Ordering, fence};

/// A ring of `CHUNKS` chunks of `CHUNK_SIZE` bytes.
///
/// `CHUNKS` is a power of two of at least two — `head` is free-running, so the mapping onto chunks
/// must survive its wrap — and one chunk is always open, leaving `CHUNKS - 1` of history.
/// `CHUNK_SIZE` is a multiple of 8, the granularity a reader copies at.
///
/// Plain data: all-zeros is a valid empty ring, and an agent with the image's DWARF can read the
/// surviving chunks straight out of a dump.
pub struct FlightBuf<const CHUNKS: usize, const CHUNK_SIZE: usize> {
    head: CachePadded<AtomicU32>,
    chunks: [CachePadded<UnsafeCell<Chunk<CHUNK_SIZE>>>; CHUNKS],
}

// Safety: the producer touches chunk `head` only; a reader touches the chunks behind it and checks
// that it was not lapped — the seqlock argument in the crate docs.
unsafe impl<const CHUNKS: usize, const CHUNK_SIZE: usize> Send for FlightBuf<CHUNKS, CHUNK_SIZE> {}
// Safety: as above; sharing across CPUs is the point.
unsafe impl<const CHUNKS: usize, const CHUNK_SIZE: usize> Sync for FlightBuf<CHUNKS, CHUNK_SIZE> {}

impl<const CHUNKS: usize, const CHUNK_SIZE: usize> FlightBuf<CHUNKS, CHUNK_SIZE> {
    const ASSERT_PARAMS: () = assert!(
        CHUNKS >= 2
            && CHUNKS.is_power_of_two()
            && CHUNKS <= u32::MAX as usize
            && CHUNK_SIZE % 8 == 0,
        "FlightBuf needs a power-of-two chunk count within 2..=u32::MAX and a chunk size that is a multiple of 8"
    );

    /// Chunks a reader can trust: every chunk but the open one.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "ASSERT_PARAMS, evaluated in `new`, bounds CHUNKS by u32::MAX"
    )]
    const HISTORY: u32 = (CHUNKS - 1) as u32;

    /// Creates an empty ring.
    #[must_use]
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        let () = Self::ASSERT_PARAMS;

        Self {
            head: CachePadded(AtomicU32::new(0)),
            chunks: [const { CachePadded(UnsafeCell::new(Chunk::new())) }; CHUNKS],
        }
    }

    /// Creates an empty ring. Not `const`: loom's atomics are not.
    #[must_use]
    #[cfg(loom)]
    pub fn new() -> Self {
        let () = Self::ASSERT_PARAMS;

        Self {
            head: CachePadded(AtomicU32::new(0)),
            chunks: core::array::from_fn(|_| CachePadded(UnsafeCell::new(Chunk::new()))),
        }
    }

    /// The appending handle.
    ///
    /// # Safety
    ///
    /// At most one `Producer` may exist at a time, and it must not be driven from two contexts at
    /// once. Where a trap handler emits too, that means masking interrupts around
    /// [`Producer::reserve`].
    #[must_use]
    pub unsafe fn producer(&self) -> Producer<'_, CHUNKS, CHUNK_SIZE> {
        Producer {
            // Relaxed: this handle is the only writer of `head`.
            head: self.head.0.load(Ordering::Relaxed),
            ring: self,
            _not_sync: PhantomData,
        }
    }

    /// A cursor over the chunks sealed from now on. Unrestricted: a reader that falls behind loses
    /// history, not correctness.
    #[must_use]
    pub fn reader(&self) -> Reader<'_, CHUNKS, CHUNK_SIZE> {
        Reader {
            // Acquire: pairs with the producer's release store in `seal`.
            next: self.head.0.load(Ordering::Acquire),
            ring: self,
            lost: 0,
        }
    }

    // Under loom the returned guard registers the access for as long as it lives, which is why
    // these hand one out rather than a bare pointer.
    #[inline(always)]
    fn chunk(&self, index: u32) -> ConstPtr<Chunk<CHUNK_SIZE>> {
        self.chunks[index as usize & (CHUNKS - 1)].get()
    }

    #[inline(always)]
    fn chunk_mut(&self, index: u32) -> MutPtr<Chunk<CHUNK_SIZE>> {
        self.chunks[index as usize & (CHUNKS - 1)].get_mut()
    }
}

// `align(8)` with `buf` first so its base is 8-aligned: aligned field stores for the producer, a
// word-at-a-time copy for the reader.
#[repr(C, align(8))]
struct Chunk<const SIZE: usize> {
    buf: [u8; SIZE],
    len: u32,
}

impl<const SIZE: usize> Chunk<SIZE> {
    const fn new() -> Self {
        Self {
            buf: [0u8; SIZE],
            len: 0,
        }
    }
}

/// The appending half of a [`FlightBuf`]. Never `Clone`, never `Sync`, so there is provably one.
pub struct Producer<'r, const CHUNKS: usize, const CHUNK_SIZE: usize> {
    ring: &'r FlightBuf<CHUNKS, CHUNK_SIZE>,
    /// Mirrors `ring.head`, which only this handle stores to.
    head: u32,
    _not_sync: PhantomData<*const ()>,
}

// Safety: movable between CPUs, but `_not_sync` keeps it from being shared, so there is still one
// writer.
unsafe impl<const CHUNKS: usize, const CHUNK_SIZE: usize> Send
    for Producer<'_, CHUNKS, CHUNK_SIZE>
{
}

impl<const CHUNKS: usize, const CHUNK_SIZE: usize> Producer<'_, CHUNKS, CHUNK_SIZE> {
    /// Reserves `bytes`, calls `f` to write them, and returns whether a chunk was sealed — i.e.
    /// whether a reader has something new to come for.
    ///
    /// The bytes hold whatever the chunk's last occupant left. A record never straddles a chunk
    /// boundary, so one that does not fit seals the open chunk first, overwriting the oldest if that
    /// is what it takes.
    ///
    /// Never allocates, blocks, panics, or waits on a reader. The one refusal is a record larger
    /// than a whole chunk: nothing is reserved and `f` never runs. Callers with compile-time sizes
    /// should assert against `CHUNK_SIZE` there rather than let a record go missing here.
    #[inline]
    pub fn reserve(&mut self, bytes: usize, f: impl FnOnce(&mut [u8])) -> bool {
        // Scoped: write access has to end before any `seal`, since publishing `head` is what hands
        // a chunk to readers.
        {
            let open = self.ring.chunk_mut(self.head);
            // Safety: `head` names the open chunk, which is this handle's alone — a reader only
            // ever touches the chunks behind it.
            let chunk = unsafe { open.deref() };
            let len = chunk.len as usize;

            // Subtracting from the chunk size keeps an absurd `bytes` from overflowing the sum, and
            // clamping `len` keeps a corrupt length from panicking (invariant 8).
            if CHUNK_SIZE - len.min(CHUNK_SIZE) >= bytes {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the sum is bounded by CHUNK_SIZE, itself bounded by u32::MAX"
                )]
                {
                    chunk.len = (len + bytes) as u32;
                }
                f(&mut chunk.buf[len..len + bytes]);
                return false;
            }
        }

        self.reserve_from_next_chunk(bytes, f)
    }

    #[cold]
    fn reserve_from_next_chunk(&mut self, bytes: usize, f: impl FnOnce(&mut [u8])) -> bool {
        if bytes > CHUNK_SIZE {
            return false;
        }
        self.seal();

        let open = self.ring.chunk_mut(self.head);
        // Safety: `seal` moved `head` onto a fresh chunk, which is this handle's alone.
        let chunk = unsafe { open.deref() };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "`bytes <= CHUNK_SIZE`, itself bounded by u32::MAX"
        )]
        {
            chunk.len = bytes as u32;
        }
        f(&mut chunk.buf[..bytes]);
        true
    }

    /// Seals the open chunk early so a low-rate CPU's records do not sit in a half-filled one.
    /// Returns whether one became readable.
    ///
    /// Runs on the producer's own CPU, in a quiescent state — no record half-written.
    #[inline]
    pub fn flush(&mut self) -> bool {
        let filled = self.open_len() != 0;
        if filled {
            self.seal();
        }
        filled
    }

    /// A function so the write guard ends before any `seal`.
    #[inline]
    fn open_len(&self) -> usize {
        let open = self.ring.chunk_mut(self.head);
        // Safety: as in `reserve`.
        unsafe { open.deref() }.len as usize
    }

    /// Publishes the open chunk and opens the next, overwriting whatever was in it.
    fn seal(&mut self) {
        let next = self.head.wrapping_add(1);

        // Release: publishes the sealed chunk *and* claims the one about to be reused — a reader
        // copying that slot learns from this store that it changed hands. Both edges are why it
        // comes before the writes below.
        self.ring.head.0.store(next, Ordering::Release);
        self.head = next;

        // Release fence: orders that claim before the reuse, so a reader that picked up any byte of
        // the reused chunk is guaranteed to see this `head` when it checks.
        fence(Ordering::Release);

        let open = self.ring.chunk_mut(next);
        // Safety: `head` now names this chunk, so it is the open one and this handle's alone.
        unsafe { open.deref() }.len = 0;
    }
}

/// A cursor over a [`FlightBuf`]'s sealed chunks. Any number may run alongside the producer.
pub struct Reader<'r, const CHUNKS: usize, const CHUNK_SIZE: usize> {
    ring: &'r FlightBuf<CHUNKS, CHUNK_SIZE>,
    next: u32,
    lost: u32,
}

impl<const CHUNKS: usize, const CHUNK_SIZE: usize> Reader<'_, CHUNKS, CHUNK_SIZE> {
    /// Chunks the producer overwrote before this reader reached them, and clears the count.
    #[must_use]
    pub fn take_lost(&mut self) -> u32 {
        core::mem::take(&mut self.lost)
    }

    /// Whether the producer has sealed nothing this reader has not delivered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        // Acquire: pairs with the producer's release store in `seal`.
        self.ring.head.0.load(Ordering::Acquire) == self.next
    }

    /// Copies the oldest undelivered chunk into `out`, returning how many of its bytes are records.
    ///
    /// `None` means nothing new, or that the producer took the chunk mid-copy — either way the next
    /// call resumes on surviving history. Skipping overwritten chunks is silent;
    /// [`take_lost`](Self::take_lost) is how many went by. Never blocks, allocates, panics, or holds
    /// the producer up.
    pub fn read(&mut self, out: &mut [u8; CHUNK_SIZE]) -> Option<usize> {
        let history = FlightBuf::<CHUNKS, CHUNK_SIZE>::HISTORY;

        // Acquire: pairs with the producer's release store in `seal`, so a chunk this reveals is
        // one whose bytes are complete.
        let head = self.ring.head.0.load(Ordering::Acquire);
        if head == self.next {
            return None;
        }
        // Lapped: resume at the oldest chunk the producer has not reused, counting what went.
        if head.wrapping_sub(self.next) > history {
            let oldest = head.wrapping_sub(history);
            self.lost = self.lost.saturating_add(oldest.wrapping_sub(self.next));
            self.next = oldest;
        }

        let len = self.ring.chunk(self.next).with(|chunk| {
            // Safety: the guard keeps `chunk` addressing a live chunk. Its bytes may be changing
            // under us — that is what the check below is for — but `Chunk` is `align(8)` with `buf`
            // first and `CHUNK_SIZE` is a multiple of 8, so every word read is aligned and in
            // bounds.
            unsafe {
                let words = (&raw const (*chunk).buf).cast::<u64>();
                for (i, out) in out.chunks_exact_mut(8).enumerate() {
                    out.copy_from_slice(&words.add(i).read_volatile().to_ne_bytes());
                }
                (&raw const (*chunk).len).read_volatile() as usize
            }
        });

        // Acquire fence: pairs with the producer's release fence, so any byte picked up from a slot
        // already handed on forces this reload to see the `head` that claimed it. A fence rather
        // than an acquire load, because it also keeps the copy from sinking past the reload.
        fence(Ordering::Acquire);
        let head = self.ring.head.0.load(Ordering::Relaxed);
        if head.wrapping_sub(self.next) > history {
            return None;
        }

        self.next = self.next.wrapping_add(1);
        // Clamped, not trusted: a corrupt length must not panic a drainer (invariant 8).
        Some(len.min(CHUNK_SIZE))
    }
}
