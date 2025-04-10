;; A simple smoke test to check argument and result passing through Wasm and hostfunctions

(module
    (import "k23" "roundtrip_i64" (func $hostfunc (param i64) (result i64)))
    (func (export "roundtrip_i64") (param $arg i64) (result i64)
        local.get $arg
        call $hostfunc
    )
)

(assert_return (invoke "roundtrip_i64" (i64.const 0)) (i64.const 0))
(assert_return (invoke "roundtrip_i64" (i64.const 42)) (i64.const 42))
(assert_return (invoke "roundtrip_i64" (i64.const 0x7fffffffffffffff)) (i64.const 0x7fffffffffffffff))
