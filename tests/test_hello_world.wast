;; Test that loads and runs our Hello World WASM program
;; This will demonstrate that stdout works with real WASM programs

(module $hello_world
    ;; Import fd_write from WASI
    (import "wasi_snapshot_preview1" "fd_write" 
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
    
    ;; Memory with at least 1 page
    (memory (export "memory") 1)
    
    ;; Data segment with our message
    (data (i32.const 8) "Hello World from WASIiiiiiiiii!\n")
    
    ;; IoVec structure at offset 0
    ;; Points to our string at offset 8, length 23
    (data (i32.const 0) "\08\00\00\00\21\00\00\00")
    
    ;; Start function
    (func $main (export "_start") (result i32)
        ;; Call fd_write(stdout=1, iovs=0, iovs_len=1, nwritten=100)
        i32.const 1      ;; fd: stdout
        i32.const 0      ;; iovs: pointer to IoVec array
        i32.const 1      ;; iovs_len: we have 1 IoVec
        i32.const 100    ;; nwritten: where to store bytes written
        call $fd_write
        ;; Return the result
    )
)

;; The module should successfully execute _start and return 0 (success)
(assert_return (invoke "_start") (i32.const 0))