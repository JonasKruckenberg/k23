(module
   (import "k23" "roundtrip_i64" (func $hostfunc (param i64) (result i64)))
   (func (export "roundtrip_i64") (param $arg i64) (result i64)
     local.get $arg
     call $hostfunc
   )
 )