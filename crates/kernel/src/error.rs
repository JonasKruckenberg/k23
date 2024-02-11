use kmem::PhysicalAddress;

#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("failed to parse Device Tree Blob")]
    DTB(#[from] dtb_parser::Error),
    #[error("missing board info property: {0}")]
    MissingBordInfo(&'static str),
    #[error("SBI call failed: {0}")]
    SBI(#[from] sbicall::Error),
    #[error("kernel memory management error: {0}")]
    Kmem(#[from] kmem::Error),
    #[error("WASM parsing failed with error: {0}")]
    WasmParser(#[from] wasmparser::Error),
    #[error("WASM translation failed with error: {0}")]
    CraneliftWasm(#[from] cranelift_wasm::Error),
    #[error("failed to init global logger")]
    InitLogger(log::SetLoggerError),
    #[error("out of memory")]
    OutOfMemory,
    #[error("tried to free already free memory {0:?}")]
    DoubleFree(PhysicalAddress),
}
