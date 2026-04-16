use std::fmt;
use std::mem::offset_of;
use std::pin::Pin;
use std::ptr::NonNull;

use criterion::{Criterion, criterion_group, criterion_main};
use rand::prelude::SliceRandom;
use rand::thread_rng;
use wavltree::{Linked, Links, WAVLTree};

#[derive(Default)]
struct WAVLEntry {
    value: usize,
    links: Links<Self>,
}
impl WAVLEntry {
    pub fn new(value: usize) -> Self {
        let mut this = Self::default();
        this.value = value;
        this
    }
}
impl fmt::Debug for WAVLEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaceHolderEntry")
            .field("value", &self.value)
            .finish()
    }
}
unsafe impl Linked for WAVLEntry {
    type Handle = Pin<Box<Self>>;
    type Key = usize;
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }
    unsafe fn links(target: NonNull<Self>) -> NonNull<Links<WAVLEntry>> {
        target
            .map_addr(|addr| {
                let offset = offset_of!(Self, links);
                addr.checked_add(offset).unwrap()
            })
            .cast()
    }
    fn get_key(&self) -> &Self::Key {
        &self.value
    }
}

fn wavl(inserts: &[usize], deletes: &[usize]) {
    let mut tree: WAVLTree<WAVLEntry> = WAVLTree::new();

    for i in inserts {
        tree.insert(Box::pin(WAVLEntry::new(*i)));
    }

    for i in deletes {
        tree.remove(i);
    }
}

fn bench_fibs(c: &mut Criterion) {
    let mut rng = thread_rng();

    let mut nums = (0..700).collect::<Vec<_>>();
    nums.shuffle(&mut rng);
    let inserts = nums.clone();
    nums.shuffle(&mut rng);
    let deletes = nums;

    c.bench_function("Insertions & Deletions", |b| {
        b.iter(|| wavl(&inserts, &deletes))
    });
}

criterion_group!(benches, bench_fibs);
criterion_main!(benches);
