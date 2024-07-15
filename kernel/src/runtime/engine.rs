use crate::runtime::codegen::{Compiler, TranslatedModule};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_wasm::wasmparser::WasmFeatures;

pub struct Engine {
    features: WasmFeatures,
    compiler: Compiler,
}

impl Engine {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        log::trace!("Setting up new Engine instance for ISA: {:?}", isa.name());
        Self {
            features: WasmFeatures::default(),
            compiler: Compiler::new(isa),
        }
    }
    pub fn wasm_features(&self) -> WasmFeatures {
        self.features
    }

    pub fn compiler(&self) -> &Compiler {
        &self.compiler
    }

    pub fn assert_compatible(&self, module: &TranslatedModule) {
        log::debug!(
            "required features {:?}, supported featured {:?}",
            module.required_features,
            self.features
        );
        assert!(self.features.contains(module.required_features), "module is incompatible with engine. Expected {:?} features to be enabled, but engine has {:?}.", module.required_features, self.features);
    }
}
