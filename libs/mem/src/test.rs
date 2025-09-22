// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::RangeBounds;
use crate::{AccessRules, VirtualAddress};
use crate::address_space::Batch;

struct AddressSpaceRegion<A> {
    _aspace: A,
}

impl<A> AddressSpaceRegion<A> {
    /// Map physical memory to back the given `range`
    ///
    /// After this call succeeds, accesses that align with the given `access` are guaranteed to
    /// not page fault. The provided `access_rules` MUST be a subset or equal to this regions access rules.
    ///
    /// # Errors
    ///
    /// - `range` is out of bounds
    /// - `access_rules` is NOT a subset of self.access_rules
    pub fn commit(
        &mut self,
        range: impl RangeBounds<VirtualAddress>,
        access_rules: AccessRules,
        batch: &mut Batch,
        raw_aspace: &mut A,
    ) -> crate::Result<()> {





        todo!()
    }

    /// Release physical memory frames backing the given `range`.
    ///
    /// After this call succeeds, accesses will page fault.
    ///
    /// # Errors
    ///
    /// - `range` is out of bounds for this address space region
    pub fn decommit(
        &mut self,
        range: impl RangeBounds<VirtualAddress>,
        batch: &mut Batch,
        raw_aspace: &mut A,
    ) -> crate::Result<()> {
        todo!()
    }
}