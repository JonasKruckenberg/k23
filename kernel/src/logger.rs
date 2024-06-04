use crate::arch;
use log::{LevelFilter, Metadata, Record};

pub fn init() {
    static LOGGER: Logger = Logger;

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Trace);
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            arch::HARTID.with(|hartid| {
                riscv::hprintln!(
                    "[{:<5} HART {} {}] {}",
                    record.level(),
                    hartid,
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                )
            });
        }
    }

    fn flush(&self) {}
}


// use crate::{arch, kconfig};
// use core::cell::UnsafeCell;
// use core::fmt::Write;
// use core::mem::MaybeUninit;
// use log::{Metadata, Record};
// use sync::ReentrantMutex;
// use uart_16550::SerialPort;
// use vmm::VirtualAddress;
// 
// /// ## Reentrancy
// ///
// /// The logger uses a `ReentrantMutex`, i.e. one that can be locked from *the same hart* multiple
// /// times without deadlocking, since the logger is also used from within trap handlers.
// /// Upon entering the trap handler, the harts execution state gets "frozen" in place, which also means
// /// any locks will continue to be held. Attempting to take out another lock for the UART SerialPort would
// /// therefore result in a deadlock.
// ///
// /// But because a hart can ever only execute one thing at a time, and we know that mutating the serial port from
// /// within a trap handler is fine (its just writing text to the screen if messages get interleaved that's fine)
// /// we can use a ReentrantMutex to just take out another lock on the same thread.
// ///
// /// Note that a ReentrantMutex only gives out immutable access to its data (because we can take out
// /// multiple locks at the same time, which would give us multiple mutable references at the same time)
// /// we have to use *Interior Mutability* which we just use an `UnsafeCell` for because - again - this
// /// use-case is special, and we know it's safe.
// static LOGGER: Logger = Logger(ReentrantMutex::new(UnsafeCell::new(MaybeUninit::uninit())));
// 
// struct Logger(ReentrantMutex<UnsafeCell<MaybeUninit<SerialPort>>>);
// 
// /// # Safety
// ///
// /// The caller has to ensure the `base` address points to the start of a UART devices MMIO-range
// pub unsafe fn init(base: VirtualAddress, clock_freq: u32) {
//     log::set_logger(&LOGGER).unwrap();
//     log::set_max_level(kconfig::LOG_LEVEL.to_level_filter());
// 
//     let serial_port = SerialPort::new(base.as_raw(), clock_freq, 38400);
// 
//     (&mut *LOGGER.0.lock().get()).write(serial_port);
// }
// 
// impl log::Log for Logger {
//     fn enabled(&self, _metadata: &Metadata) -> bool {
//         true
//     }
// 
//     fn log(&self, record: &Record) {
//         if self.enabled(record.metadata()) {
//             let uart = self.0.lock();
//             // Safety: We only ever lock the mutex here, so potential double-locking only occurs within trap handlers
//             // were this is fine. Writing bytes to the uart doesn't meaningfully impact the original lock holder.
//             let uart = unsafe { (&mut *uart.get()).assume_init_mut() };
// 
//             let _ = arch::HARTID.with(|hartid| {
//                 writeln!(
//                     uart,
//                     "[{:<5} HART {hartid} {}] {}",
//                     record.level(),
//                     record.module_path_static().unwrap_or_default(),
//                     record.args()
//                 )
//             });
//         }
//     }
// 
//     fn flush(&self) {}
// }
