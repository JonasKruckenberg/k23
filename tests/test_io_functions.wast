;; Tests for I/O host functions

;; Test module for fd_write
(module $test_fd_write
    ;; Import WASI I/O functions
    (import "wasi_snapshot_preview1" "fd_write" 
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
    
    ;; Memory for test data
    (memory (export "memory") 1)
    
    ;; Test data: "Hello stdout!\n"
    (data (i32.const 0) "Hello stdout!\n")
    
    ;; Test data: "Hello stderr!\n"  
    (data (i32.const 20) "Hello stderr!\n")
    
    ;; IoVec structure for stdout at offset 100
    ;; buf_ptr = 0, buf_len = 14
    (data (i32.const 100) "\00\00\00\00\0e\00\00\00")
    
    ;; IoVec structure for stderr at offset 108
    ;; buf_ptr = 20, buf_len = 14
    (data (i32.const 108) "\14\00\00\00\0e\00\00\00")
    
    ;; Test writing to stdout
    (func (export "test_stdout") (result i32)
        i32.const 1      ;; fd (stdout)
        i32.const 100    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 200    ;; nwritten pointer
        call $fd_write
    )
    
    ;; Test writing to stderr
    (func (export "test_stderr") (result i32)
        i32.const 2      ;; fd (stderr)
        i32.const 108    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 200    ;; nwritten pointer
        call $fd_write
    )
    
    ;; Test writing to invalid fd
    (func (export "test_invalid_fd") (result i32)
        i32.const 99     ;; invalid fd
        i32.const 100    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 200    ;; nwritten pointer
        call $fd_write
    )
)

;; Test assertions for fd_write
(assert_return (invoke "test_stdout") (i32.const 0))     ;; Should succeed
(assert_return (invoke "test_stderr") (i32.const 0))     ;; Should succeed
(assert_return (invoke "test_invalid_fd") (i32.const 8)) ;; Should return ERRNO_BADF

;; Test module for fd_read
(module $test_fd_read
    (import "wasi_snapshot_preview1" "fd_read"
        (func $fd_read (param i32 i32 i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; IoVec structure at offset 100
    ;; buf_ptr = 200, buf_len = 100
    (data (i32.const 100) "\c8\00\00\00\64\00\00\00")
    
    ;; Test reading from stdin
    (func (export "test_stdin") (result i32)
        i32.const 0      ;; fd (stdin)
        i32.const 100    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 300    ;; nread pointer
        call $fd_read
    )
    
    ;; Test reading from stdout (should fail)
    (func (export "test_read_stdout") (result i32)
        i32.const 1      ;; fd (stdout - invalid for reading)
        i32.const 100    ;; iovs pointer
        i32.const 1      ;; iovs_len
        i32.const 300    ;; nread pointer
        call $fd_read
    )
)

;; Test assertions for fd_read
(assert_return (invoke "test_stdin") (i32.const 0))         ;; Should succeed (returns EOF)
(assert_return (invoke "test_read_stdout") (i32.const 8))   ;; Should return ERRNO_BADF

;; Test module for fd_close
(module $test_fd_close
    (import "wasi_snapshot_preview1" "fd_close"
        (func $fd_close (param i32) (result i32)))
    
    ;; Test closing stdin (should fail - can't close standard streams)
    (func (export "test_close_stdin") (result i32)
        i32.const 0      ;; fd (stdin)
        call $fd_close
    )
    
    ;; Test closing a regular fd (should succeed with stub filesystem)
    (func (export "test_close_regular") (result i32)
        i32.const 10     ;; some fd
        call $fd_close
    )
)

;; Test assertions for fd_close
(assert_return (invoke "test_close_stdin") (i32.const 8))    ;; Should return ERRNO_BADF
(assert_return (invoke "test_close_regular") (i32.const 0))  ;; Should succeed with stub filesystem

;; Test module for fd_seek
(module $test_fd_seek
    (import "wasi_snapshot_preview1" "fd_seek"
        (func $fd_seek (param i32 i64 i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; Test seeking (not implemented yet)
    (func (export "test_seek") (result i32)
        i32.const 3           ;; fd
        i64.const 100         ;; offset
        i32.const 0           ;; whence (SET)
        i32.const 200         ;; newoffset pointer
        call $fd_seek
    )
)

;; Test assertion for fd_seek
(assert_return (invoke "test_seek") (i32.const 0))  ;; Should succeed with stub filesystem