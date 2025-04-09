// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]

// (import "k23" "roundtrip_i64" (func $hostfunc (param i64) (result i64)))
#[link(wasm_import_module = "k23")]
extern "C" {
    #[link_name = "roundtrip_i64"]
    fn host_roundtrip_i64(params: i64) -> i64;
}

// (func (export "roundtrip_i64") (param $arg i64) (result i64)
#[no_mangle]
extern "C" fn roundtrip_i64(param: i64) -> i64 {
    unsafe { host_roundtrip_i64(param) }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}