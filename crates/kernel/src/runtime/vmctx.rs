#[repr(C)]
pub struct VMContext {
    // size(builtin_functions) = ret.pointer_size(),
    // size(defined_globals)
    // = cmul(ret.num_defined_globals, ret.ptr.size_of_vmglobal_definition()),
}
