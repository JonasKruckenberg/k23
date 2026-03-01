#[derive(Debug, Clone, Copy)]
pub enum Error {
    /// Failed to allocate the memory mapping metadata
    AllocError,
}

impl From<alloc::alloc::AllocError> for Error {
    fn from(_: alloc::alloc::AllocError) -> Self {
        Self::AllocError
    }
}
