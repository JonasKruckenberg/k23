use crate::rt::instance::{Instance};
use crate::rt::module::Module;
use crate::rt::store::Store;

pub struct Linker {}

impl Linker {
    pub fn new() -> Self {
        Self {}
    }

    pub fn instantiate<'wasm>(&self, store: &mut Store<'wasm>, module: &Module<'wasm>) -> Instance {
        Instance::new(store, module).unwrap()
    }
}
