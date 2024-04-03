use ewr::{Compiler, ModuleEnvironment};

#[test]
fn main() {
    env_logger::init();

    let bytes = include_bytes!("file_icons.wasm");

    let compiler = Compiler::new_for_host().unwrap();

    let parsed = ewr::parse_module(bytes).unwrap();

    let mut env = ModuleEnvironment::new(&compiler);
    let translated = env.translate(parsed).unwrap();

    translated.debug_print();
}
