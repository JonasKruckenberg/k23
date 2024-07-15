use crate::runtime::instance::Instance;
use crate::runtime::module::Module;
use crate::runtime::store::Store;

pub struct Linker {}

impl Linker {
    pub fn new() -> Self {
        Self {}
    }

    #[allow(clippy::unused_self)]
    pub fn instantiate<'wasm>(&self, store: &mut Store<'wasm>, module: &Module<'wasm>) -> Instance {
        Instance::new(store, module).unwrap()
    }
}
