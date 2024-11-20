# An intrusive Weak AVL Tree.

A Rust implementation of Weak AVL Trees, primarily for use in the [k23 operating system][k23].

Weak AVL trees are *self-balancing binary search trees* introduced by [Haeupler, Sen & Tarjan (2015)][paper] that are
similar to red-black trees but better in several ways.
In particular, their worst-case height is that of AVL trees (~1.44log2(n) as opposed to 2log2(n) for red-black trees),
while tree restructuring operations after deletions are even more efficient than red-black trees.
Additionally, this implementation is *intrusive* meaning node data (pointers to other nodes etc.) are stored _within_
participating values, rather than being allocated and owned by the tree itself.

This crate is self-contained, (somewhat) fuzzed, and fully `no_std`.

## when to use this

- **want binary search** - WAVL trees are *sorted* collections that are efficient to search.
- **search more than you edit** - WAVL trees offer better search complexity than red-black trees at the cost of being
  slightly more complex.
- **want to avoid hidden allocations** - Because node data is stored _inside_ participating values, an element can be
  added without
  requiring additional heap allocations.
- **have to allocator at all** - When elements have fixed memory locations (such as pages in a page allocator, `static`
  s),
  they can be added without *any allocations at all*.
- **want flexibility** - Intrusive data structures allow elements to participate in many different collections at the
  same time,
  e.g. a node might both be linked to a `WAVLTree` and an intrusive doubly-linked list at the same time.

In short, `WAVLTree`s are a good choice for `no_std` binary search trees such as inside page allocators.

## when not to use this

- **need to store primitives** - Intrusive collections require elements to store the node data, which excludes
  primitives
  such
  as strings or numbers, since they can't hold this metadata.
- **can't use unsafe** - Both this implementation and code consuming it require `unsafe`, the `Linked` trait is unsafe
  to
  implement since it requires implementors uphold special invariants.

## features

The following features are available:

| Feature | Default | Explanation                                                                               |
|:--------|:--------|:------------------------------------------------------------------------------------------|
| `dot`   | `false` | Enables the `WAVLTree::dot` method, which allows display of the tree in [graphviz format] |

## references

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
