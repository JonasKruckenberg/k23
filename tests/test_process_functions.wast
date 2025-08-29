;; Tests for Process management host functions

;; Test module for proc_exit
(module $test_proc_exit
    (import "wasi_snapshot_preview1" "proc_exit"
        (func $proc_exit (param i32) (result i32)))
    
    ;; Test exiting with code 0
    (func (export "test_exit_0") (result i32)
        i32.const 0
        call $proc_exit
    )
    
    ;; Test exiting with code 42
    (func (export "test_exit_42") (result i32)
        i32.const 42
        call $proc_exit
    )
    
    ;; Test exiting with negative code
    (func (export "test_exit_neg") (result i32)
        i32.const -1
        call $proc_exit
    )
)

;; Test assertions for proc_exit
;; Note: proc_exit currently just returns the exit code instead of terminating
(assert_return (invoke "test_exit_0") (i32.const 0))
(assert_return (invoke "test_exit_42") (i32.const 42))
(assert_return (invoke "test_exit_neg") (i32.const -1))  ;; -1

;; Test module for environment variables
(module $test_environ
    (import "wasi_snapshot_preview1" "environ_sizes_get"
        (func $environ_sizes_get (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "environ_get"
        (func $environ_get (param i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; Test getting environment sizes
    (func (export "test_environ_sizes") (result i32)
        i32.const 100    ;; environ_count pointer
        i32.const 104    ;; environ_buf_size pointer
        call $environ_sizes_get
    )
    
    ;; Test getting environment variables
    (func (export "test_environ_get") (result i32)
        i32.const 200    ;; environ pointer
        i32.const 300    ;; environ_buf pointer
        call $environ_get
    )
)

;; Test assertions for environment functions
(assert_return (invoke "test_environ_sizes") (i32.const 0))  ;; Should succeed
(assert_return (invoke "test_environ_get") (i32.const 0))    ;; Should succeed

;; Test module for command-line arguments
(module $test_args
    (import "wasi_snapshot_preview1" "args_sizes_get"
        (func $args_sizes_get (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "args_get"
        (func $args_get (param i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; Test getting argument sizes
    (func (export "test_args_sizes") (result i32)
        i32.const 100    ;; argc pointer
        i32.const 104    ;; argv_buf_size pointer
        call $args_sizes_get
    )
    
    ;; Test getting arguments
    (func (export "test_args_get") (result i32)
        i32.const 200    ;; argv pointer
        i32.const 300    ;; argv_buf pointer
        call $args_get
    )
)

;; Test assertions for argument functions
(assert_return (invoke "test_args_sizes") (i32.const 0))  ;; Should succeed
(assert_return (invoke "test_args_get") (i32.const 0))    ;; Should succeed

;; Test module for random_get
(module $test_random
    (import "wasi_snapshot_preview1" "random_get"
        (func $random_get (param i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; Test getting 16 random bytes
    (func (export "test_random_16") (result i32)
        i32.const 100    ;; buffer pointer
        i32.const 16     ;; length
        call $random_get
    )
    
    ;; Test getting 0 random bytes
    (func (export "test_random_0") (result i32)
        i32.const 100    ;; buffer pointer
        i32.const 0      ;; length
        call $random_get
    )
    
    ;; Test getting large amount of random bytes
    (func (export "test_random_large") (result i32)
        i32.const 100    ;; buffer pointer
        i32.const 1024   ;; length
        call $random_get
    )
)

;; Test assertions for random_get
(assert_return (invoke "test_random_16") (i32.const 0))     ;; Should succeed
(assert_return (invoke "test_random_0") (i32.const 0))      ;; Should succeed
(assert_return (invoke "test_random_large") (i32.const 0))  ;; Should succeed