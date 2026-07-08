;; Check that an invalid stack pointer triggers a trap when calling into the
;; host. This is checked by corrupting the stack limit.

(module
    (import "k23" "roundtrip_i64" (func $hostfunc (param i64) (result i64)))
    (import "k23" "corrupt_stack_limit" (func $corrupt))
    (func (export "check_stack_limit") (param $arg i64) (result i64)
		call $corrupt
        local.get $arg
        call $hostfunc
    )
)

(assert_trap (invoke "check_stack_limit" (i64.const 0)) "invalid stack pointer")
