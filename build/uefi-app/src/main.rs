#![no_main]
#![no_std]

use core::time::Duration;

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::info!("Hello world!");
    boot::stall(Duration::from_secs(10));
    Status::SUCCESS
}
