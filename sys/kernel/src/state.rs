// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::OnceCell;

use cpu_local::cpu_local;
use kasync::executor::Executor;
use kasync::time::{Instant, Timer};
use loader_api::BootInfo;
use spin::OnceLock;

use crate::arch;
use crate::device_tree::DeviceTree;

static GLOBAL: OnceLock<Global> = OnceLock::new();

cpu_local! {
    static CPU_LOCAL: OnceCell<CpuLocal> = OnceCell::new();
}

#[derive(Debug)]
pub struct Global {
    pub executor: Executor,
    pub timer: Timer,
    pub device_tree: DeviceTree,
    pub boot_info: &'static BootInfo,
    pub time_origin: Instant,
    pub arch: arch::state::Global,
}

#[derive(Debug)]
pub struct CpuLocal {
    pub id: usize,
    pub arch: arch::state::CpuLocal,
}

pub fn try_init_global<F>(f: F) -> crate::Result<&'static Global>
where
    F: FnOnce() -> crate::Result<Global>,
{
    GLOBAL.get_or_try_init(f)
}

pub fn init_cpu_local(state: CpuLocal) {
    CPU_LOCAL
        .set(state)
        .expect("CPU local state already initialized");
}

pub fn global() -> &'static Global {
    GLOBAL.get().expect("Global state not initialized")
}

pub fn try_global() -> Option<&'static Global> {
    GLOBAL.get()
}

pub fn cpu_local() -> &'static CpuLocal {
    CPU_LOCAL.get().expect("Cpu local state not initialized")
}

pub fn try_cpu_local() -> Option<&'static CpuLocal> {
    CPU_LOCAL.get()
}
