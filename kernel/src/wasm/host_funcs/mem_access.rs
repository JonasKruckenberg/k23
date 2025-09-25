// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Memory access helpers for WASI host functions

use alloc::vec::Vec;
use core::mem;
use core::slice;
use core::sync::atomic::Ordering;

use crate::wasm::func::Caller;
use crate::wasm::vm::VMMemoryDefinition;
use crate::wasm::indices::MemoryIndex;

/// Helper to access WASM memory from host functions
pub struct MemoryAccessor<'a> {
    memory_def: &'a VMMemoryDefinition,
}

impl<'a> MemoryAccessor<'a> {
    /// Create a new memory accessor from a caller context
    /// 
    /// Returns None if the instance has no memory
    pub fn new<T>(caller: &'a mut Caller<'_, T>) -> Option<Self> {
        // Get the default memory (index 0) from the instance
        // We need to get mutable access to the instance to call its methods
        let memory_ptr = unsafe {
            // Access the caller's instance memory definition pointer
            let instance = caller.caller as *const _ as *mut crate::wasm::vm::Instance;
            (*instance).defined_or_imported_memory(MemoryIndex::from_u32(0))
        };
        
        let memory_def = unsafe { memory_ptr.as_ref() };
        
        Some(Self { memory_def })
    }

    /// Get the base address of the memory
    pub fn base(&self) -> *mut u8 {
        self.memory_def.base.as_ptr()
    }

    /// Get the current size of the memory in bytes
    pub fn size(&self) -> usize {
        self.memory_def.current_length(Ordering::Relaxed)
    }

    /// Check if a memory range is valid
    pub fn is_valid_range(&self, offset: u32, len: u32) -> bool {
        let offset = offset as usize;
        let len = len as usize;
        
        // Check for overflow
        let end = match offset.checked_add(len) {
            Some(end) => end,
            None => return false,
        };
        
        end <= self.size()
    }

    /// Read a value from memory at the given offset
    /// 
    /// # Safety
    /// 
    /// The caller must ensure the offset and size are valid
    pub unsafe fn read<T: Copy>(&self, offset: u32) -> Option<T> {
        let size = mem::size_of::<T>();
        if !self.is_valid_range(offset, size as u32) {
            return None;
        }
        
        unsafe {
            let ptr = self.base().add(offset as usize) as *const T;
            Some(*ptr)
        }
    }

    /// Write a value to memory at the given offset
    /// 
    /// # Safety
    /// 
    /// The caller must ensure the offset and size are valid
    pub unsafe fn write<T: Copy>(&self, offset: u32, value: &T) -> bool {
        let size = mem::size_of::<T>();
        if !self.is_valid_range(offset, size as u32) {
            return false;
        }
        
        unsafe {
            let ptr = self.base().add(offset as usize) as *mut T;
            *ptr = *value;
        }
        true
    }

    /// Get a slice of memory
    /// 
    /// # Safety
    /// 
    /// The caller must ensure the offset and length are valid
    pub unsafe fn get_slice(&self, offset: u32, len: u32) -> Option<&'a [u8]> {
        if !self.is_valid_range(offset, len) {
            return None;
        }
        
        unsafe {
            let ptr = self.base().add(offset as usize);
            Some(slice::from_raw_parts(ptr, len as usize))
        }
    }

    /// Get a mutable slice of memory
    /// 
    /// # Safety
    /// 
    /// The caller must ensure the offset and length are valid
    pub unsafe fn get_slice_mut(&self, offset: u32, len: u32) -> Option<&'a mut [u8]> {
        if !self.is_valid_range(offset, len) {
            return None;
        }
        
        unsafe {
            let ptr = self.base().add(offset as usize);
            Some(slice::from_raw_parts_mut(ptr, len as usize))
        }
    }

    /// Write bytes to memory
    pub fn write_bytes(&self, offset: u32, data: &[u8]) -> bool {
        if !self.is_valid_range(offset, data.len() as u32) {
            return false;
        }
        
        unsafe {
            let ptr = self.base().add(offset as usize);
            ptr.copy_from_nonoverlapping(data.as_ptr(), data.len());
        }
        true
    }

    /// Read bytes from memory
    pub fn read_bytes(&self, offset: u32, len: u32) -> Option<Vec<u8>> {
        unsafe {
            self.get_slice(offset, len).map(|s| s.to_vec())
        }
    }
}