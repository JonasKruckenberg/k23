;; Test WASI host functions
(module
    ;; Import WASI functions
    (import "wasi_snapshot_preview1" "fd_write" 
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "proc_exit" 
        (func $proc_exit (param i32) (result i32)))
    
    ;; Memory for data
    (memory (export "memory") 1)
    
    ;; Data segment with "Hello WASI!\n"
    (data (i32.const 0) "Hello WASI!\n")
    
    ;; IoVec structure at offset 100
    ;; buf_ptr = 0, buf_len = 12
    (data (i32.const 100) "\00\00\00\00\0c\00\00\00")
    
    ;; Function to print hello
    (func (export "hello") (result i32)
        ;; Call fd_write(stdout=1, iovs=100, iovs_len=1, nwritten=200)
        i32.const 1      ;; fd (stdout)
        i32.const 100    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 200    ;; nwritten pointer
        call $fd_write
    )
    
    ;; Function to exit (returns the exit code for testing)
    (func (export "exit") (param i32) (result i32)
        local.get 0
        call $proc_exit
    )
)

;; Test assertions
(assert_return (invoke "hello") (i32.const 0))  ;; Should return success
;; Test exit function (but don't actually exit)
(assert_return (invoke "exit" (i32.const 42)) (i32.const 42))  ;; Should return the exit code