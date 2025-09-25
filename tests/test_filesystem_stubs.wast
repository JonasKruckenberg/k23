;; Comprehensive tests for all filesystem stub functions

;; ============================================================================
;; 1. PREOPENED DIRECTORY FUNCTIONS
;; ============================================================================

(module $test_preopen
    (import "wasi_snapshot_preview1" "fd_prestat_get"
        (func $fd_prestat_get (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_prestat_dir_name"
        (func $fd_prestat_dir_name (param i32 i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; fd_prestat_get tests
    (func (export "test_prestat_get_valid") (result i32)
        i32.const 3      ;; fd=3 (preopened root)
        i32.const 100    ;; buffer
        call $fd_prestat_get
    )
    
    (func (export "test_prestat_get_invalid") (result i32)
        i32.const 99     ;; invalid fd
        i32.const 100    
        call $fd_prestat_get
    )
    
    (func (export "test_prestat_get_stdio") (result i32)
        i32.const 0      ;; stdin (not preopened)
        i32.const 100    
        call $fd_prestat_get
    )
    
    ;; fd_prestat_dir_name tests
    (func (export "test_prestat_dir_name_valid") (result i32)
        i32.const 3      ;; fd=3
        i32.const 200    ;; buffer
        i32.const 10     ;; sufficient length
        call $fd_prestat_dir_name
    )
    
    (func (export "test_prestat_dir_name_invalid_fd") (result i32)
        i32.const 99     ;; invalid fd
        i32.const 200    
        i32.const 10     
        call $fd_prestat_dir_name
    )
    
    (func (export "test_prestat_dir_name_small_buffer") (result i32)
        i32.const 3      ;; fd=3
        i32.const 200    
        i32.const 0      ;; buffer too small
        call $fd_prestat_dir_name
    )
)

;; Assertions for preopened directory functions
(assert_return (invoke "test_prestat_get_valid") (i32.const 0))        ;; SUCCESS
(assert_return (invoke "test_prestat_get_invalid") (i32.const 8))      ;; ERRNO_BADF
(assert_return (invoke "test_prestat_get_stdio") (i32.const 8))        ;; ERRNO_BADF
(assert_return (invoke "test_prestat_dir_name_valid") (i32.const 0))   ;; SUCCESS
(assert_return (invoke "test_prestat_dir_name_invalid_fd") (i32.const 8)) ;; ERRNO_BADF
(assert_return (invoke "test_prestat_dir_name_small_buffer") (i32.const 28)) ;; ERRNO_INVAL

;; ============================================================================
;; 2. PATH OPERATIONS
;; ============================================================================

(module $test_path_ops
    (import "wasi_snapshot_preview1" "path_open"
        (func $path_open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_create_directory"
        (func $path_create_directory (param i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_remove_directory"
        (func $path_remove_directory (param i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_unlink_file"
        (func $path_unlink_file (param i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_rename"
        (func $path_rename (param i32 i32 i32 i32 i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    (data (i32.const 0) "test.txt")
    (data (i32.const 20) "newdir")
    (data (i32.const 40) "oldname")
    (data (i32.const 60) "newname")
    
    ;; path_open tests
    (func (export "test_path_open_valid") (result i32)
        i32.const 3      ;; dirfd (preopened)
        i32.const 0      ;; dirflags
        i32.const 0      ;; path ptr
        i32.const 8      ;; path len
        i32.const 0      ;; oflags
        i64.const -1     ;; rights_base
        i64.const -1     ;; rights_inheriting
        i32.const 0      ;; fdflags
        i32.const 100    ;; fd_ptr
        call $path_open
    )
    
    ;; path_create_directory test
    (func (export "test_path_create_dir") (result i32)
        i32.const 3      ;; dirfd
        i32.const 20     ;; path ptr ("newdir")
        i32.const 6      ;; path len
        call $path_create_directory
    )
    
    ;; path_remove_directory test
    (func (export "test_path_remove_dir") (result i32)
        i32.const 3      ;; dirfd
        i32.const 20     ;; path ptr
        i32.const 6      ;; path len
        call $path_remove_directory
    )
    
    ;; path_unlink_file test
    (func (export "test_path_unlink") (result i32)
        i32.const 3      ;; dirfd
        i32.const 0      ;; path ptr
        i32.const 8      ;; path len
        call $path_unlink_file
    )
    
    ;; path_rename test
    (func (export "test_path_rename") (result i32)
        i32.const 3      ;; old_dirfd
        i32.const 40     ;; old_path ptr
        i32.const 7      ;; old_path len
        i32.const 3      ;; new_dirfd
        i32.const 60     ;; new_path ptr
        i32.const 7      ;; new_path len
        call $path_rename
    )
)

;; Assertions for path operations
(assert_return (invoke "test_path_open_valid") (i32.const 0))    ;; SUCCESS
(assert_return (invoke "test_path_create_dir") (i32.const 0))    ;; SUCCESS
(assert_return (invoke "test_path_remove_dir") (i32.const 0))    ;; SUCCESS
(assert_return (invoke "test_path_unlink") (i32.const 0))        ;; SUCCESS
(assert_return (invoke "test_path_rename") (i32.const 0))        ;; SUCCESS

;; ============================================================================
;; 3. FILE DESCRIPTOR I/O OPERATIONS (from io.rs)
;; ============================================================================

(module $test_fd_io
    (import "wasi_snapshot_preview1" "fd_read"
        (func $fd_read (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_write"
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_close"
        (func $fd_close (param i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_seek"
        (func $fd_seek (param i32 i64 i32 i32) (result i32)))
    
    (memory (export "memory") 1)
    
    ;; IoVec structure at offset 100 (buf_ptr=200, buf_len=10)
    (data (i32.const 100) "\c8\00\00\00\0a\00\00\00")
    
    ;; fd_read tests
    (func (export "test_fd_read_stdin") (result i32)
        i32.const 0      ;; stdin
        i32.const 100    ;; iovs ptr
        i32.const 1      ;; iovs len
        i32.const 300    ;; nread ptr
        call $fd_read
    )
    
    (func (export "test_fd_read_stdout") (result i32)
        i32.const 1      ;; stdout (should fail)
        i32.const 100    
        i32.const 1      
        i32.const 300    
        call $fd_read
    )
    
    (func (export "test_fd_read_file") (result i32)
        i32.const 4      ;; file fd
        i32.const 100    
        i32.const 1      
        i32.const 300    
        call $fd_read
    )
    
    ;; fd_write tests
    (func (export "test_fd_write_stdout") (result i32)
        i32.const 1      ;; stdout
        i32.const 100    ;; iovs ptr
        i32.const 1      ;; iovs len
        i32.const 300    ;; nwritten ptr
        call $fd_write
    )
    
    (func (export "test_fd_write_stderr") (result i32)
        i32.const 2      ;; stderr
        i32.const 100    
        i32.const 1      
        i32.const 300    
        call $fd_write
    )
    
    (func (export "test_fd_write_stdin") (result i32)
        i32.const 0      ;; stdin (should fail)
        i32.const 100    
        i32.const 1      
        i32.const 300    
        call $fd_write
    )
    
    ;; fd_close tests
    (func (export "test_fd_close_stdin") (result i32)
        i32.const 0      ;; stdin (should fail)
        call $fd_close
    )
    
    (func (export "test_fd_close_file") (result i32)
        i32.const 4      ;; file fd
        call $fd_close
    )
    
    ;; fd_seek test
    (func (export "test_fd_seek") (result i32)
        i32.const 4      ;; fd
        i64.const 100    ;; offset
        i32.const 0      ;; whence (SEEK_SET)
        i32.const 400    ;; newoffset ptr
        call $fd_seek
    )
)

;; Assertions for FD I/O operations
(assert_return (invoke "test_fd_read_stdin") (i32.const 0))       ;; SUCCESS (returns EOF)
(assert_return (invoke "test_fd_read_stdout") (i32.const 8))      ;; ERRNO_BADF
(assert_return (invoke "test_fd_read_file") (i32.const 0))        ;; SUCCESS
(assert_return (invoke "test_fd_write_stdout") (i32.const 0))     ;; SUCCESS
(assert_return (invoke "test_fd_write_stderr") (i32.const 0))     ;; SUCCESS
(assert_return (invoke "test_fd_write_stdin") (i32.const 8))      ;; ERRNO_BADF
(assert_return (invoke "test_fd_close_stdin") (i32.const 8))      ;; ERRNO_BADF
(assert_return (invoke "test_fd_close_file") (i32.const 0))       ;; SUCCESS
(assert_return (invoke "test_fd_seek") (i32.const 0))             ;; SUCCESS

;; ============================================================================
;; 4. FILE STATISTICS
;; ============================================================================

(module $test_stats
    (import "wasi_snapshot_preview1" "fd_filestat_get"
        (func $fd_filestat_get (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_filestat_get"
        (func $path_filestat_get (param i32 i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_fdstat_get"
        (func $fd_fdstat_get (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_readdir"
        (func $fd_readdir (param i32 i32 i32 i64 i32) (result i32)))
    
    (memory (export "memory") 1)
    (data (i32.const 0) "test.txt")
    
    ;; fd_filestat_get tests
    (func (export "test_fd_filestat_stdin") (result i32)
        i32.const 0      ;; stdin
        i32.const 100    ;; buffer
        call $fd_filestat_get
    )
    
    (func (export "test_fd_filestat_preopen") (result i32)
        i32.const 3      ;; preopen
        i32.const 100    
        call $fd_filestat_get
    )
    
    (func (export "test_fd_filestat_file") (result i32)
        i32.const 4      ;; file fd
        i32.const 100    
        call $fd_filestat_get
    )
    
    (func (export "test_fd_filestat_invalid") (result i32)
        i32.const -1     ;; invalid fd
        i32.const 100    
        call $fd_filestat_get
    )
    
    ;; path_filestat_get test
    (func (export "test_path_filestat") (result i32)
        i32.const 3      ;; dirfd
        i32.const 0      ;; flags
        i32.const 0      ;; path ptr
        i32.const 8      ;; path len
        i32.const 200    ;; buffer
        call $path_filestat_get
    )
    
    ;; fd_fdstat_get tests
    (func (export "test_fd_fdstat_stdin") (result i32)
        i32.const 0      ;; stdin
        i32.const 300    ;; buffer
        call $fd_fdstat_get
    )
    
    (func (export "test_fd_fdstat_preopen") (result i32)
        i32.const 3      ;; preopen
        i32.const 300    
        call $fd_fdstat_get
    )
    
    (func (export "test_fd_fdstat_invalid") (result i32)
        i32.const -1     ;; invalid
        i32.const 300    
        call $fd_fdstat_get
    )
    
    ;; fd_readdir test
    (func (export "test_fd_readdir") (result i32)
        i32.const 3      ;; directory fd
        i32.const 400    ;; buffer
        i32.const 256    ;; buffer len
        i64.const 0      ;; cookie
        i32.const 500    ;; bufused ptr
        call $fd_readdir
    )
)

;; Assertions for file statistics
(assert_return (invoke "test_fd_filestat_stdin") (i32.const 0))     ;; SUCCESS
(assert_return (invoke "test_fd_filestat_preopen") (i32.const 0))   ;; SUCCESS
(assert_return (invoke "test_fd_filestat_file") (i32.const 0))      ;; SUCCESS
(assert_return (invoke "test_fd_filestat_invalid") (i32.const 8))   ;; ERRNO_BADF
(assert_return (invoke "test_path_filestat") (i32.const 0))         ;; SUCCESS
(assert_return (invoke "test_fd_fdstat_stdin") (i32.const 0))       ;; SUCCESS
(assert_return (invoke "test_fd_fdstat_preopen") (i32.const 0))     ;; SUCCESS
(assert_return (invoke "test_fd_fdstat_invalid") (i32.const 8))     ;; ERRNO_BADF
(assert_return (invoke "test_fd_readdir") (i32.const 0))            ;; SUCCESS

;; ============================================================================
;; 5. TIMESTAMP OPERATIONS
;; ============================================================================

(module $test_times
    (import "wasi_snapshot_preview1" "fd_filestat_set_times"
        (func $fd_filestat_set_times (param i32 i64 i64 i32) (result i32)))
    (import "wasi_snapshot_preview1" "path_filestat_set_times"
        (func $path_filestat_set_times (param i32 i32 i32 i32 i64 i64 i32) (result i32)))
    
    (memory (export "memory") 1)
    (data (i32.const 0) "test.txt")
    
    ;; fd_filestat_set_times test
    (func (export "test_fd_set_times") (result i32)
        i32.const 4              ;; fd
        i64.const 1000000000     ;; atime
        i64.const 2000000000     ;; mtime
        i32.const 3              ;; fst_flags (SET_ATIME | SET_MTIME)
        call $fd_filestat_set_times
    )
    
    ;; path_filestat_set_times test
    (func (export "test_path_set_times") (result i32)
        i32.const 3              ;; dirfd
        i32.const 0              ;; flags
        i32.const 0              ;; path ptr
        i32.const 8              ;; path len
        i64.const 1000000000     ;; atime
        i64.const 2000000000     ;; mtime
        i32.const 3              ;; fst_flags
        call $path_filestat_set_times
    )
)

;; Assertions for timestamp operations
(assert_return (invoke "test_fd_set_times") (i32.const 0))    ;; SUCCESS
(assert_return (invoke "test_path_set_times") (i32.const 0))  ;; SUCCESS

;; ============================================================================
;; 6. SYNC AND ALLOCATION OPERATIONS
;; ============================================================================

(module $test_sync
    (import "wasi_snapshot_preview1" "fd_fdstat_set_flags"
        (func $fd_fdstat_set_flags (param i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_allocate"
        (func $fd_allocate (param i32 i64 i64) (result i32)))
    (import "wasi_snapshot_preview1" "fd_datasync"
        (func $fd_datasync (param i32) (result i32)))
    (import "wasi_snapshot_preview1" "fd_sync"
        (func $fd_sync (param i32) (result i32)))
    
    ;; fd_fdstat_set_flags test
    (func (export "test_fdstat_set_flags") (result i32)
        i32.const 4      ;; fd
        i32.const 1      ;; flags (FDFLAGS_APPEND)
        call $fd_fdstat_set_flags
    )
    
    ;; fd_allocate test
    (func (export "test_fd_allocate") (result i32)
        i32.const 4      ;; fd
        i64.const 0      ;; offset
        i64.const 1024   ;; len
        call $fd_allocate
    )
    
    ;; fd_datasync test
    (func (export "test_fd_datasync") (result i32)
        i32.const 4      ;; fd
        call $fd_datasync
    )
    
    ;; fd_sync test
    (func (export "test_fd_sync") (result i32)
        i32.const 4      ;; fd
        call $fd_sync
    )
)

;; Assertions for sync operations
(assert_return (invoke "test_fdstat_set_flags") (i32.const 0))  ;; SUCCESS
(assert_return (invoke "test_fd_allocate") (i32.const 0))       ;; SUCCESS
(assert_return (invoke "test_fd_datasync") (i32.const 0))       ;; SUCCESS
(assert_return (invoke "test_fd_sync") (i32.const 0))           ;; SUCCESS