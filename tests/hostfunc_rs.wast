;; A simple smoke test to check argument and result passing through Wasm and hostfunctions
;; This test is compiled from Rust to serve as a more realistic target

(module $hostfunc_rs.wasm
  (type (;0;) (func (param i64) (result i64)))
  (import "k23" "roundtrip_i64" (func $_ZN11hostfunc_rs18host_roundtrip_i6417h4c66d31ca11529a5E (type 0)))
  (func $roundtrip_i64 (type 0) (param i64) (result i64)
    (local i64)
    local.get 0
    call $_ZN11hostfunc_rs18host_roundtrip_i6417h4c66d31ca11529a5E
    local.set 1
    local.get 1
    return)
  (memory (;0;) 16)
  (global $__stack_pointer (mut i32) (i32.const 1048576))
  (global (;1;) i32 (i32.const 1048576))
  (global (;2;) i32 (i32.const 1048576))
  (export "memory" (memory 0))
  (export "roundtrip_i64" (func $roundtrip_i64))
  (export "__data_end" (global 1))
  (export "__heap_base" (global 2))
)

(assert_return (invoke "roundtrip_i64" (i64.const 0)) (i64.const 0))
(assert_return (invoke "roundtrip_i64" (i64.const 42)) (i64.const 42))
(assert_return (invoke "roundtrip_i64" (i64.const 0x7fffffffffffffff)) (i64.const 0x7fffffffffffffff))
