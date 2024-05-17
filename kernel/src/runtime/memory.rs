use crate::runtime::guest_memory::GuestVec;
use crate::runtime::vmcontext::VMMemoryDefinition;

#[derive(Debug)]
pub struct Memory {
    pub inner: GuestVec<u8>,
    pub current_length: usize,
    pub maximum: Option<usize>,
    pub asid: usize,
}

impl Memory {
    pub fn as_vmmemory(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: self.inner.as_mut_ptr(),
            current_length: self.current_length.into(),
            asid: 0,
        }
    }
}
