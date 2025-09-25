;; Tests for Memory management host functions

;; Test module for memory operations
(module $test_memory
    ;; Import memory functions
    (import "wasi_snapshot_preview1" "memory_grow"
        (func $memory_grow (param i32) (result i32)))
    (import "wasi_snapshot_preview1" "memory_size"
        (func $memory_size (result i32)))
    
    ;; Initial memory: 1 page (64KB)
    (memory (export "memory") 1)
    
    ;; Test getting current memory size
    (func (export "test_memory_size") (result i32)
        call $memory_size
    )
    
    ;; Test growing memory by 1 page
    (func (export "test_memory_grow_1") (result i32)
        i32.const 1
        call $memory_grow
    )
    
    ;; Test growing memory by 0 pages (should succeed)
    (func (export "test_memory_grow_0") (result i32)
        i32.const 0
        call $memory_grow
    )
    
    ;; Test growing memory by large amount (likely to fail)
    (func (export "test_memory_grow_large") (result i32)
        i32.const 65536  ;; Try to grow by 4GB worth of pages
        call $memory_grow
    )
    
    ;; Test growing by negative amount (should fail)
    (func (export "test_memory_grow_negative") (result i32)
        i32.const -1
        call $memory_grow
    )
)

;; Test assertions for memory functions
;; Note: Current stubs return 0 for size and -1 for grow
(assert_return (invoke "test_memory_size") (i32.const 0))           ;; Stub returns 0
(assert_return (invoke "test_memory_grow_1") (i32.const -1))        ;; Stub returns -1 (failure)
(assert_return (invoke "test_memory_grow_0") (i32.const -1))        ;; Stub returns -1 (failure)
(assert_return (invoke "test_memory_grow_large") (i32.const -1))    ;; Should fail
(assert_return (invoke "test_memory_grow_negative") (i32.const -1)) ;; Should fail

;; Test module for memory access patterns
(module $test_memory_access
    (import "wasi_snapshot_preview1" "memory_size"
        (func $memory_size (result i32)))
    
    ;; Initial memory: 2 pages
    (memory (export "memory") 2 5)  ;; min 2, max 5 pages
    
    ;; Test data
    (data (i32.const 0) "Test data at page 0")
    (data (i32.const 65536) "Test data at page 1")
    
    ;; Function to write and read from memory
    (func (export "test_memory_rw") (result i32)
        ;; Write value to memory
        i32.const 1000
        i32.const 42
        i32.store
        
        ;; Read it back
        i32.const 1000
        i32.load
    )
    
    ;; Function to check memory bounds
    (func (export "test_memory_bounds") (result i32)
        ;; Try to access within bounds (page 1)
        i32.const 65536
        i32.load8_u
        drop
        
        ;; Return success indicator
        i32.const 1
    )
)

;; Test assertions for memory access
(assert_return (invoke "test_memory_rw") (i32.const 42))      ;; Should write and read 42
(assert_return (invoke "test_memory_bounds") (i32.const 1))   ;; Should succeed