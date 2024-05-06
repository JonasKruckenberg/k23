use crate::runtime::instance::Instance;
use crate::runtime::instantiate::Store;
use crate::runtime::module::Module;
use crate::runtime::Engine;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub struct Linker {
    string2idx: BTreeMap<Arc<str>, usize>,
    strings: Vec<Arc<str>>,
    map: BTreeMap<ImportKey, ()>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Ord, PartialOrd)]
struct ImportKey {
    module: usize,
    field: usize,
}

impl Linker {
    pub fn new() -> Self {
        Self {
            string2idx: Default::default(),
            strings: vec![],
            map: Default::default(),
        }
    }

    pub fn instantiate<'wasm>(
        &self,
        store: &mut Store<'wasm>,
        engine: &Engine,
        module: Module<'wasm>,
    ) -> Instance {
        unsafe { Instance::new_raw(store, module) }
    }

    // pub fn instantiate_pre<'wasm>(&self, module: Module<'wasm>) -> InstancePre<'wasm> {
    //     // let mut imports = module
    //     //     .imports()
    //     //     .map(|import| self._get_by_import(&import))
    //     //     .collect::<Vec<_>>();
    //
    //     InstancePre::new(module)
    // }

    // fn _get_by_import(&self, import: &ImportType) -> &() {
    //     let key = self.get_import_key(import.module, import.field);
    //     let ext = self.map.get(&key).unwrap();
    //     // TODO assert type
    // }

    // fn insert(&mut self, key: ImportKey, item: Func) {
    //     match self.map.entry(key) {
    //         Entry::Occupied(_) => {
    //             let module = &self.strings[key.module];
    //             panic!(
    //                 "import of `{}::{}` defined_twice",
    //                 module,
    //                 self.strings.get(key.name)
    //             );
    //         }
    //         Entry::Vacant(v) => {
    //             v.insert(item);
    //         }
    //     }
    // }

    fn def_import_key(&mut self, module: impl AsRef<str>, field: impl AsRef<str>) -> ImportKey {
        ImportKey {
            module: self.intern_str(module.as_ref()),
            field: self.intern_str(field.as_ref()),
        }
    }

    fn get_import_key(&self, module: &str, field: &str) -> ImportKey {
        let module = *self.string2idx.get(module).unwrap();

        let field = *self.string2idx.get(field).unwrap();

        ImportKey { module, field }
    }

    fn intern_str(&mut self, string: &str) -> usize {
        if let Some(idx) = self.string2idx.get(string) {
            return *idx;
        }
        let string: Arc<str> = string.into();
        let idx = self.strings.len();
        self.strings.push(string.clone());
        self.string2idx.insert(string, idx);
        idx
    }
}
