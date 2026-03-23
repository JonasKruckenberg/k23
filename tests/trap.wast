(module
    (func $traps (unreachable))
	(func (export "outer") (call $traps))
)

(assert_trap (invoke "outer") "unreachable code executed")
