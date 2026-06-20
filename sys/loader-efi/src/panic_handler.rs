// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::hint;
use core::time::Duration;

use abort::abort;
use uefi::boot;

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo) -> ! {
    log::error!("[PANIC]: {}", info);

    // Give the user some time to read the message
    if crate::are_boot_services_active() {
        boot::stall(Duration::from_secs(10));
    } else {
        for _ in 0..300_000_000u32 {
            hint::spin_loop();
        }
    }

    // if the system table is available, use UEFI's standard shutdown mechanism
    if let Some(st) = uefi::table::system_table_raw() {
        // Safety: ensured by `uefi` crate
        if !unsafe { st.as_ref().runtime_services }.is_null() {
            uefi::runtime::reset(
                uefi::runtime::ResetType::SHUTDOWN,
                uefi::Status::ABORTED,
                None,
            );
        }
    }

    abort();
}
