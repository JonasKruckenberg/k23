// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use abort::abort;

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo) -> ! {
    log::error!("[PANIC]: {}", info);

    abort();
}
