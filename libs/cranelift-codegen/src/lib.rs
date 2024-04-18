//! Cranelift code generation library.
#![deny(missing_docs)]
#![cfg_attr(not(test), no_std)]
#![feature(error_in_core)]
// Various bits and pieces of this crate might only be used for one platform or
// another, but it's not really too useful to learn about that all the time. On
// CI we build at least one version of this crate with `--features all-arch`
// which means we'll always detect truly dead code, otherwise if this is only
// built for one platform we don't have to worry too much about trimming
// everything down.
#![cfg_attr(not(feature = "all-arch"), allow(dead_code))]

#[allow(unused_imports)] // #[macro_use] is required for no_std
#[macro_use]
extern crate alloc;

pub use crate::context::Context;
pub use crate::value_label::{LabelValueLoc, ValueLabelsRanges, ValueLocRange};
pub use crate::verifier::verify_function;
pub use crate::write::write_function;
use hashbrown::{hash_map, HashMap};

pub use cranelift_bforest as bforest;
pub use cranelift_entity as entity;
#[cfg(feature = "unwind")]
pub use gimli;

#[macro_use]
mod machinst;

pub mod binemit;
pub mod cfg_printer;
pub mod cursor;
pub mod data_value;
pub mod dbg;
pub mod dominator_tree;
pub mod flowgraph;
pub mod ir;
pub mod isa;
pub mod loop_analysis;
pub mod print_errors;
pub mod settings;
pub mod verifier;
pub mod write;

pub use crate::entity::packed_option;
pub use crate::machinst::buffer::{
    FinalizedMachReloc, FinalizedRelocTarget, MachCallSite, MachSrcLoc, MachStackMap,
    MachTextSectionBuilder, MachTrap, OpenPatchRegion, PatchRegion,
};
pub use crate::machinst::{
    CompiledCode, Final, MachBuffer, MachBufferFinalized, MachInst, MachInstEmit,
    MachInstEmitState, MachLabel, RealReg, Reg, RelocDistance, TextSectionBuilder,
    VCodeConstantData, VCodeConstants, Writable,
};

mod alias_analysis;
mod bitset;
mod constant_hash;
mod context;
mod ctxhash;
mod dce;
mod egraph;
mod inst_predicates;
mod isle_prelude;
mod iterators;
mod legalizer;
mod nan_canonicalization;
mod opts;
mod remove_constant_phis;
mod result;
mod scoped_hash_map;
mod unionfind;
mod unreachable_code;
mod value_label;

pub use crate::result::{CodegenError, CodegenResult, CompileError};

include!(concat!(env!("OUT_DIR"), "/version.rs"));
