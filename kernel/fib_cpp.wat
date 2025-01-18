(module
  (type (;0;) (func (param i32) (result i32)))
  (func (;0;) (type 0) (param i32) (result i32)
    (local i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
    global.get 0
    local.set 1
    i32.const 32
    local.set 2
    local.get 1
    local.get 2
    i32.sub
    local.set 3
    i32.const 0
    local.set 4
    i32.const 1
    local.set 5
    local.get 3
    local.get 0
    i32.store offset=28
    local.get 3
    local.get 4
    i32.store offset=20
    local.get 3
    local.get 5
    i32.store offset=16
    local.get 3
    local.get 4
    i32.store offset=12
    block  ;; label = @1
      loop  ;; label = @2
        local.get 3
        i32.load offset=12
        local.set 6
        local.get 3
        i32.load offset=28
        local.set 7
        local.get 6
        local.set 8
        local.get 7
        local.set 9
        local.get 8
        local.get 9
        i32.lt_s
        local.set 10
        i32.const 1
        local.set 11
        local.get 10
        local.get 11
        i32.and
        local.set 12
        local.get 12
        i32.eqz
        br_if 1 (;@1;)
        local.get 3
        i32.load offset=20
        local.set 13
        local.get 3
        local.get 13
        i32.store offset=24
        local.get 3
        i32.load offset=16
        local.set 14
        local.get 3
        local.get 14
        i32.store offset=20
        local.get 3
        i32.load offset=24
        local.set 15
        local.get 3
        i32.load offset=16
        local.set 16
        local.get 16
        local.get 15
        i32.add
        local.set 17
        local.get 3
        local.get 17
        i32.store offset=16
        local.get 3
        i32.load offset=12
        local.set 18
        i32.const 1
        local.set 19
        local.get 18
        local.get 19
        i32.add
        local.set 20
        local.get 3
        local.get 20
        i32.store offset=12
        br 0 (;@2;)
      end
    end
    local.get 3
    i32.load offset=16
    local.set 21
    local.get 21
    return)
  (table (;0;) 1 1 funcref)
  (memory (;0;) 1)
  (global (;0;) (mut i32) (i32.const 66560))
  (export "memory" (memory 0))
  (export "fib" (func 0))
)