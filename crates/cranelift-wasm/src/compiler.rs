use cranelift_codegen::dominator_tree::DominatorTree;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::settings::Configurable;
use cranelift_codegen::{ir, CompiledCode};

pub struct Compiler {
    pub(crate) isa: OwnedTargetIsa,
}

impl Compiler {
    pub fn new_for_host() -> crate::Result<Self> {
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST)?;
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size")?;

        let isa = isa_builder.finish(cranelift_codegen::settings::Flags::new(b))?;

        Ok(Self { isa })
    }

    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    pub fn compile_function(&self, function: &ir::Function) -> crate::Result<CompiledCode> {
        let cfg = ControlFlowGraph::with_function(function);
        let domtree = DominatorTree::with_function(function, &cfg);

        let compiled = self.isa.compile_function(function, &domtree, false)?;

        Ok(compiled.apply_params(&function.params))
    }
}
