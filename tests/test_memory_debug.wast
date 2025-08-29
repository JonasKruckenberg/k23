;; Debug version of memory tests - testing each function individually

;; Test 1: Just memory_size
(module $test_memory_size_only
    (import "wasi_snapshot_preview1" "memory_size"
        (func $memory_size (result i32)))
    
    (memory (export "memory") 1)
    
    (func (export "test_size") (result i32)
        call $memory_size
    )
)

(assert_return (invoke "test_size") (i32.const 0))

;; Test 2: Just memory_grow with positive value
(module $test_memory_grow_positive
    (import "wasi_snapshot_preview1" "memory_grow"
        (func $memory_grow (param i32) (result i32)))
    
    (memory (export "memory") 1)
    
    (func (export "test_grow") (result i32)
        i32.const 1
        call $memory_grow
    )
)

(assert_return (invoke "test_grow") (i32.const -1))

;; Test 3: Just memory_grow with zero
(module $test_memory_grow_zero
    (import "wasi_snapshot_preview1" "memory_grow"
        (func $memory_grow (param i32) (result i32)))
    
    (memory (export "memory") 1)
    
    (func (export "test_grow_zero") (result i32)
        i32.const 0
        call $memory_grow
    )
)

(assert_return (invoke "test_grow_zero") (i32.const -1))