// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

extern crate std;

use alloc::collections::BTreeMap;
use core::marker::PhantomData;
use core::num::NonZeroUsize;

use crate::address_space::{Flush, RawAddressSpace};
use crate::{AccessRules, PhysicalAddress, VirtualAddress};

pub struct TestAddressSpace<const PAGE_SIZE: usize> {
    mappings: BTreeMap<VirtualAddress, Mapping>,
}

pub struct Mapping {
    pub virt: VirtualAddress,
    pub phys: PhysicalAddress,
    pub len: NonZeroUsize,
    pub access_rules: AccessRules,
}

pub struct TestFlush {
    _priv: PhantomData<()>,
}

impl<const PAGE_SIZE: usize> TestAddressSpace<PAGE_SIZE> {
    pub const fn new() -> Self {
        Self {
            mappings: BTreeMap::new(),
        }
    }

    pub fn get_mapping_containing(&self, addr: VirtualAddress) -> Option<&Mapping> {
        let (end, mapping) = self.mappings.range(addr..).next()?;

        if addr > *end { None } else { Some(mapping) }
    }

    pub fn get_mapping_mut_containing(&mut self, addr: VirtualAddress) -> Option<&mut Mapping> {
        let (end, mapping) = self.mappings.range_mut(addr..).next()?;

        if addr > *end { None } else { Some(mapping) }
    }

    pub fn remove_mapping_containing(&mut self, addr: VirtualAddress) -> Option<Mapping> {
        let (key, _) = self.mappings.range_mut(addr..).next()?;
        let key = *key;

        Some(self.mappings.remove(&key).unwrap())
    }
}

unsafe impl<const PAGE_SIZE: usize> RawAddressSpace for TestAddressSpace<PAGE_SIZE> {
    const PAGE_SIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(PAGE_SIZE) };

    type Flush = TestFlush;

    fn flush(&self) -> Self::Flush {
        TestFlush { _priv: PhantomData }
    }

    fn lookup(&self, virt: VirtualAddress) -> Option<(PhysicalAddress, AccessRules)> {
        let mapping = self.get_mapping_containing(virt)?;

        let offset = virt.checked_sub_addr(mapping.virt).unwrap();

        Some((
            mapping.phys.checked_add(offset).unwrap(),
            mapping.access_rules,
        ))
    }

    unsafe fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
        _flush: &mut Self::Flush,
    ) -> crate::Result<()> {
        assert!(virt.is_aligned_to(Self::PAGE_SIZE.get()));
        assert!(phys.is_aligned_to(Self::PAGE_SIZE.get()));
        assert!(self.get_mapping_containing(virt).is_none());

        let end_virt = virt.checked_add(len.get() - 1).unwrap();
        assert!(end_virt.is_aligned_to(Self::PAGE_SIZE.get()));

        let prev = self.mappings.insert(
            end_virt,
            Mapping {
                virt,
                phys,
                len,
                access_rules,
            },
        );
        assert!(prev.is_none());

        Ok(())
    }

    unsafe fn unmap(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        _flush: &mut Self::Flush,
    ) {
        assert!(virt.is_aligned_to(Self::PAGE_SIZE.get()));
        assert!(
            virt.checked_add(len.get())
                .unwrap()
                .is_aligned_to(Self::PAGE_SIZE.get())
        );

        let mut bytes_remaining = len.get();

        while bytes_remaining > 0 {
            let mapping = self.remove_mapping_containing(virt).unwrap();
            assert_eq!(mapping.virt, virt);

            bytes_remaining -= mapping.len.get();
            virt = virt.checked_sub(mapping.len.get()).unwrap();
        }
    }

    unsafe fn set_access_rules(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
        _flush: &mut Self::Flush,
    ) {
        assert!(virt.is_aligned_to(Self::PAGE_SIZE.get()));
        assert!(
            virt.checked_add(len.get())
                .unwrap()
                .is_aligned_to(Self::PAGE_SIZE.get())
        );

        let mut bytes_remaining = len.get();

        while bytes_remaining > 0 {
            let mapping = self.get_mapping_mut_containing(virt).unwrap();
            assert_eq!(mapping.virt, virt);

            mapping.access_rules = access_rules;

            bytes_remaining -= mapping.len.get();
            virt = virt.checked_sub(mapping.len.get()).unwrap();
        }
    }
}

// ===== impl TestFlush =====

impl Flush for TestFlush {
    fn flush(self) -> crate::Result<()> {
        Ok(())
    }
}
