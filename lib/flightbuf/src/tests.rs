// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Chunk arithmetic and the overwrite policy first, then two loom models.
//!
//! The models cover the two cross-CPU edges a reader depends on — a sealed chunk becoming visible,
//! and a seal elsewhere in the ring *not* being mistaken for a lap — each checked to fail with its
//! ordering weakened, on rings sized so the producer cannot lap the reader within the modelled
//! writes. Lapping itself is deliberately outside them: it is a data race by construction (see the
//! crate docs), so loom would report the very interleaving the seqlock exists to survive. What it
//! does to the cursor needs no concurrency to check, and is checked above.

use crate::loom::{self, thread};
use crate::{FlightBuf, Producer, Reader};

/// Writes a record. Returns whether a chunk became readable.
fn append<const CHUNKS: usize, const CHUNK_SIZE: usize>(
    producer: &mut Producer<'_, CHUNKS, CHUNK_SIZE>,
    record: &[u8],
) -> bool {
    producer.reserve(record.len(), |bytes| bytes.copy_from_slice(record))
}

/// Reads the oldest undelivered chunk out as a `Vec`, or `None` if there is none.
fn read_one<const CHUNKS: usize, const CHUNK_SIZE: usize>(
    reader: &mut Reader<'_, CHUNKS, CHUNK_SIZE>,
) -> Option<Vec<u8>> {
    let mut out = [0u8; CHUNK_SIZE];
    reader.read(&mut out).map(|len| out[..len].to_vec())
}

/// Runs `body` against a ring, freed once both handles are gone.
///
/// The handles have to be `'static` — loom has no scoped threads — but leaking the ring would trip
/// miri, and loom would leak one per interleaving.
fn with_ring<const CHUNKS: usize, const CHUNK_SIZE: usize>(
    body: impl FnOnce(Producer<'static, CHUNKS, CHUNK_SIZE>, Reader<'static, CHUNKS, CHUNK_SIZE>),
) {
    let ring = Box::into_raw(Box::new(FlightBuf::<CHUNKS, CHUNK_SIZE>::new()));

    // Safety: the pointer comes straight out of `Box::into_raw` and nothing else refers to it, so
    // the reference is unaliased and lives until the box is reclaimed below; the producer taken
    // from it is the only one.
    let handles = unsafe { ((*ring).producer(), (*ring).reader()) };
    body(handles.0, handles.1);

    // Safety: `body` has returned, so both handles are gone and the allocation is unreferenced.
    drop(unsafe { Box::from_raw(ring) });
}

// -- chunk arithmetic ----------------------------------------------------------------------------

/// Records accumulate in the open chunk and stay invisible to a reader until it is sealed.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn appends_stay_invisible_until_sealed() {
    with_ring::<4, 16>(|mut producer, mut reader| {
        for record in [b"aaaa", b"bbbb", b"cccc"] {
            assert!(!append(&mut producer, record), "the chunk has room");
            assert!(reader.is_empty(), "nothing is sealed yet");
        }

        assert!(producer.flush(), "a partial chunk seals on flush");
        assert_eq!(read_one(&mut reader).as_deref(), Some(&b"aaaabbbbcccc"[..]));
    });
}

/// A record that does not fit seals the open chunk and lands in the next one, whole.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn a_record_that_does_not_fit_starts_a_new_chunk() {
    with_ring::<4, 16>(|mut producer, mut reader| {
        assert!(!append(&mut producer, b"aaaaaa"));
        assert!(!append(&mut producer, b"bbbbbb"));
        // Only four bytes left, so this one starts a new chunk rather than being split across the
        // boundary.
        assert!(append(&mut producer, b"cccccc"), "should have sealed");

        assert_eq!(
            read_one(&mut reader).as_deref(),
            Some(&b"aaaaaabbbbbb"[..]),
            "the sealed chunk holds whole records and nothing else"
        );
        assert!(reader.is_empty(), "the third record is still open");

        assert!(producer.flush());
        assert_eq!(read_one(&mut reader).as_deref(), Some(&b"cccccc"[..]));
    });
}

/// The producer never waits on a reader: it takes the oldest chunk back, and the reader finds out
/// afterwards by resuming at the oldest one that survived.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn a_lagging_reader_is_lapped_not_obeyed() {
    with_ring::<2, 8>(|mut producer, mut reader| {
        // Each record fills a chunk exactly, so every write after the first seals one. A two-chunk
        // ring keeps one chunk of history, so only the last sealed chunk survives.
        for record in [b"aaaaaaaa", b"bbbbbbbb", b"cccccccc", b"dddddddd"] {
            append(&mut producer, record);
        }

        assert_eq!(read_one(&mut reader).as_deref(), Some(&b"cccccccc"[..]));
        assert_eq!(reader.take_lost(), 2, "chunks 0 and 1 were overwritten");
        assert!(reader.is_empty(), "the fourth record is still open");
        assert_eq!(reader.take_lost(), 0, "the count is taken, not accumulated");
    });
}

/// A record larger than a whole chunk is the one thing no ring can hold, and refusing it must not
/// disturb the one it has.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn an_oversized_record_is_refused() {
    with_ring::<4, 16>(|mut producer, mut reader| {
        assert!(!append(&mut producer, b"aaaa"));
        assert!(!producer.reserve(17, |_| unreachable!("nothing was reserved")));
        assert!(!producer.reserve(usize::MAX, |_| unreachable!()));

        assert!(reader.is_empty(), "a refusal must not seal anything");
        assert!(producer.flush());
        assert_eq!(read_one(&mut reader).as_deref(), Some(&b"aaaa"[..]));
    });
}

/// Flushing an empty chunk publishes nothing — a reader must not be woken for it.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn flushing_an_empty_chunk_is_a_noop() {
    with_ring::<4, 16>(|mut producer, reader| {
        assert!(!producer.flush());
        assert!(reader.is_empty());

        assert!(!append(&mut producer, b"aaaa"));
        assert!(producer.flush());
        assert!(!producer.flush(), "the chunk it opened is empty");
    });
}

/// The ring keeps working after `head` has gone round the array many times.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn wraparound() {
    const ROUNDS: u8 = 200;

    with_ring::<4, 8>(|mut producer, mut reader| {
        for round in 0..ROUNDS {
            let record = [round; 8];
            // Fills the open chunk exactly, so the *next* write is what seals it.
            assert!(!append(&mut producer, &record));
            assert!(producer.flush());

            assert_eq!(
                read_one(&mut reader).as_deref(),
                Some(&record[..]),
                "round {round}"
            );
            assert!(reader.is_empty());
        }
    });
}

#[cfg(not(loom))]
mod props {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// A reader that keeps up is never lapped, so whatever the record sizes, the bytes out are
        /// exactly the bytes in — whole, in order, nothing lost or duplicated.
        #[test]
        // proptest persists failures to a file; `getcwd` is denied under miri isolation. Same
        // unsafe paths as the tests above.
        #[cfg_attr(miri, ignore)]
        fn a_reader_that_keeps_up_loses_nothing(lens in prop::collection::vec(0usize..=12, 1..256)) {
            let mut written: Vec<u8> = Vec::new();
            let mut read: Vec<u8> = Vec::new();
            let mut next: u8 = 0;

            with_ring::<4, 32>(|mut producer, mut reader| {
                for len in lens {
                    let record: Vec<u8> =
                        (0..len).map(|_| { next = next.wrapping_add(1); next }).collect();
                    append(&mut producer, &record);
                    written.extend_from_slice(&record);
                    while let Some(bytes) = read_one(&mut reader) {
                        read.extend_from_slice(&bytes);
                    }
                }

                producer.flush();
                while let Some(bytes) = read_one(&mut reader) {
                    read.extend_from_slice(&bytes);
                }
                assert_eq!(reader.take_lost(), 0);
            });

            prop_assert_eq!(read, written);
        }
    }
}

// -- loom: the two cross-CPU edges ---------------------------------------------------------------
//
// Four chunks of eight bytes: a record fills a chunk exactly, so every write after the first seals
// one, and the modelled writes stay short of the four seals it would take to lap the reader.

/// A chunk a reader can see is one whose contents are complete, in every interleaving.
#[test]
fn seal_publishes_the_chunk() {
    loom::model(|| {
        with_ring::<4, 8>(|mut producer, mut reader| {
            let writer = thread::spawn(move || {
                append(&mut producer, b"aaaaaaaa");
                // Fills the chunk exactly, so this one seals chunk 0 and lands in chunk 1.
                append(&mut producer, b"bbbbbbbb");
            });

            // Bounded: an unbounded poll loop would blow loom's branch limit. Anything missed is
            // picked up after the join.
            let mut seen = None;
            for _ in 0..2 {
                if let Some(bytes) = read_one(&mut reader) {
                    seen = Some(bytes);
                    break;
                }
            }

            writer.join().unwrap();
            let seen = seen.or_else(|| read_one(&mut reader));

            assert_eq!(seen.as_deref(), Some(&b"aaaaaaaa"[..]));
            assert!(read_one(&mut reader).is_none(), "chunk 1 was never sealed");
        });
    });
}

/// A seal elsewhere in the ring is not a lap: a reader checking against a `head` the producer moved
/// on must still accept the chunk it copied, and report no loss.
#[test]
fn a_seal_behind_the_reader_is_not_a_lap() {
    loom::model(|| {
        with_ring::<4, 8>(|mut producer, mut reader| {
            append(&mut producer, b"aaaaaaaa");

            let writer = thread::spawn(move || {
                // Two more seals, so `head` runs ahead of the reader without reaching chunk 0's
                // slot again — the four-chunk ring is what keeps it short of a lap.
                append(&mut producer, b"bbbbbbbb");
                append(&mut producer, b"cccccccc");
            });

            let mut seen = Vec::new();
            for _ in 0..2 {
                if let Some(bytes) = read_one(&mut reader) {
                    seen.push(bytes);
                }
            }

            writer.join().unwrap();
            while let Some(bytes) = read_one(&mut reader) {
                seen.push(bytes);
            }

            assert_eq!(seen, [b"aaaaaaaa".to_vec(), b"bbbbbbbb".to_vec()]);
            assert_eq!(reader.take_lost(), 0, "nothing was overwritten");
        });
    });
}
