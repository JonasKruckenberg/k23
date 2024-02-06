use cranelift_codegen::ir;
use cranelift_entity::entity_impl;

/// An opaque reference to a [`HeapData`][crate::HeapData].
///
/// While the order is stable, it is arbitrary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Heap(u32);
entity_impl!(Heap, "heap");

/// WebAssembly operates on a small set of bounded memory areas.
/// These are represented as `Heap`s following the design of Wasmtime.
///
///
/// ## Heaps and virtual memory
///
/// In traditional WASM engines, heaps are allocated through the host OS and therefore need additional
/// protections such as guard pages. In our case however, where WASM instances *are* userspace programs
/// we can use virtual memory for memory isolation.
///
/// In the case that there is only one linear memory configured, the heap starts at a
/// dynamically determined start address (after the VMcontext and data) and runs until the kernel memory space starts.
/// In case that are multiple linear memories the address space is equally split among the heaps
/// (excluding heaps that have a configured max size).
///
/// Heaps are made up of two address ranges: *mapped pages* and *unmapped pages*:
/// When first initializing **min_size** pages are automatically mapped to the processes' address space,
/// followed by unmapped pages until the heap ends.
///
/// A heap starts out with all the address space it will ever need, so
/// it never moves to a different address. At the base address is a number of
/// mapped pages corresponding to the heap's current size. Then follows a number
/// of unmapped pages where the heap can grow up to its maximum size.
///
/// Heaps therefore correspond 1:1 to the processes' memory.
pub struct HeapData {
    /// The address of the start of the heap's storage.
    pub base: ir::GlobalValue,

    /// Guaranteed minimum heap size in **pages**. Heap accesses before `min_size`
    /// don't need bounds checking.
    pub min_size: u64,

    /// The maximum heap size in **pages**.
    ///
    /// Heap accesses larger than this will always trap.
    pub max_size: u64,

    /// Whether this is a 64-bit memory
    pub memory64: bool,

    /// The index type for the heap.
    pub index_type: ir::Type,

    /// The memory type for the pointed-to memory, if using proof-carrying code.
    pub memory_type: Option<ir::MemoryType>,
}
