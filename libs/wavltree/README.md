<div align="center">
  <h1>
    <code>wavltree</code>
  </h1>
  <p>
    <strong>An intrusive Weak AVL Tree.</strong>
  </p>
  <p>

[![MIT licensed][mit-badge]][mit-url]

  </p>
</div>

[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: LICENSE

A Rust implementation of Weak AVL Trees, primarily for use in the [k23 operating system][k23].

Weak AVL trees are *self-balancing binary search trees* introduced by [Haeupler, Sen & Tarjan (2015)][paper] that are
similar to red-black trees but better in several ways.
In particular, their worst-case height is that of AVL trees (~1.44log2(n) as opposed to 2log2(n) for red-black trees),
while tree restructuring operations after deletions are even more efficient than red-black trees.
Additionally, this implementation is *intrusive* meaning node data (pointers to other nodes etc.) are stored _within_
participating values, rather than being allocated and owned by the tree itself.

**This crate is self-contained, (somewhat) fuzzed, and fully `no_std`.**

## Example

The following example shows an implementation of a simple intrusive WAVL tree node (`MyNode`) and
how it can be used with `WAVLTree`, notice how - due to the intrusive nature of the data structure -
there is quite a lot more setup required, compared to e.g. a `BTreeMap` or `HashMap`.

```rust
use alloc::boxed::Box;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;

#[derive(Default)]
struct MyNode {
    links: wavltree::Links<Self>,
    value: usize,
}

impl MyNode {
    pub fn new(value: usize) -> Self {
        let mut this = Self::default();
        this.value = value;
        this
    }
}

// Participation in an intrusive collection requires a bit more effort
// on the values's part.
unsafe impl wavltree::Linked for MyNode {
    /// The owning handle type, must ensure participating values are pinned in memory.
    type Handle = Pin<Box<Self>>;
    /// The key type by which entries are identified.
    type Key = usize;

    /// Convert a `Handle` into a raw pointer to `Self`,
    /// taking ownership of it in the process.
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }

    /// Return the links of the node pointed to by ptr.
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    /// Retrieve the key identifying this node within the collection.
    fn get_key(&self) -> &Self::Key {
        &self.value
   }
}

fn main() {
    let mut tree = wavltree::WAVLTree::new();
    tree.insert(Box::pin(MyNode::new(42)));
    tree.insert(Box::pin(MyNode::new(17)));
    tree.insert(Box::pin(MyNode::new(9)));

    tree.remove(&9);

    let _entry = tree.entry(&42);
}
```

## When To Use This

- **want binary search** - WAVL trees are *sorted* collections that are efficient to search.
- **search more than you edit** - WAVL trees offer better search complexity than red-black trees at the cost of being
  slightly more complex.
- **want to avoid hidden allocations** - Because node data is stored _inside_ participating values, an element can be
  added without requiring additional heap allocations.
- **have to allocator at all** - When elements have fixed memory locations (such as pages in a page allocator, `static`
  s), they can be added without *any allocations at all*.
- **want flexibility** - Intrusive data structures allow elements to participate in many different collections at the
  same time, e.g. a node might both be linked to a `WAVLTree` and an intrusive doubly-linked list at the same time.

In short, `WAVLTree`s are a good choice for `no_std` binary search trees such as inside page allocators.

## When Not To Use This

- **need to store primitives** - Intrusive collections require elements to store the node data, which excludes
  primitives such as strings or numbers, since they can't hold this metadata.
- **can't use unsafe** - Both this implementation and code consuming it require `unsafe`, the `Linked` trait is unsafe
  to implement since it requires implementors uphold special invariants.
- **you are unsure if you need this** - Search trees and especially intrusive ones like this are niche data structures,
  only use them if you are sure you need them. Very likely doing binary search on a sorted `Vec` or using a `HashMap`
  works better for your use case.

## Cargo Features

The following features are available:

| Feature | Default | Explanation                                                                               |
|:--------|:--------|:------------------------------------------------------------------------------------------|
| `dot`   | `false` | Enables the `WAVLTree::dot` method, which allows display of the tree in [graphviz format] |

## References

This paper implements the Weak AVL tree algorithm in Rust as described in [Haeupler, Sen & Tarjan (2015)][paper], with
additional
references taken from [Phil Vachons WAVL tree C implementation][pvachon] as well
as [the implementation in the Fuchsia base library][fuchsia].

Inspiration for the design of intrusive APIs in Rust has been taken from [cordyceps] and [intrusive-collections]

[cordyceps]: https://docs.rs/intrusive-collections/latest/intrusive_collections/index.html

[intrusive-collections]: https://docs.rs/cordyceps/latest/cordyceps/index.html

[intrusive]: https://www.boost.org/doc/libs/1_45_0/doc/html/intrusive/intrusive_vs_nontrusive.html

[paper]: https://sidsen.azurewebsites.net/papers/rb-trees-talg.pdf

[k23]: https://github.com/JonasKruckenberg/k23

[pvachon]: https://github.com/pvachon/wavl_tree/blob/main/wavltree.c

[fuchsia]: https://fuchsia.googlesource.com/fuchsia/+/master/zircon/system/ulib/fbl/include/fbl/intrusive_wavl_tree.h

[graphviz format]: https://graphviz.org
