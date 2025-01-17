(module
  (import "fib_cpp" "fib" (func $fib (param i32) (result i32)))
  (func $fib_test (export "fib_test")
    ;; the 8th fibonacci number is 21
    i32.const 7
    i32.const 21
    call $check
    ;; the 9th fibonacci number is 34
    i32.const 8
    i32.const 34
    call $check
    ;; the 10th fibonacci number is 55
    i32.const 9
    i32.const 55
    call $check
  )

  (func $check (param $n i32) (param $expected i32)
      local.get $n
      call $fib
      local.get $expected
      i32.ne
      if
          unreachable
      end
  )
)