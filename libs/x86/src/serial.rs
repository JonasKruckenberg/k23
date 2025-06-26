// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Serial port I/O for early boot debugging

use core::fmt;

/// COM1 serial port base address
pub const COM1_BASE: u16 = 0x3F8;

/// A simple serial port writer for COM1
pub struct SerialPort {
    port: u16,
}

impl SerialPort {
    /// Creates a new serial port writer for the given port
    pub const fn new(port: u16) -> Self {
        Self { port }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            unsafe {
                // Write byte to serial port
                core::arch::asm!(
                    "out dx, al",
                    in("dx") self.port,
                    in("al") byte,
                );
            }
        }
        Ok(())
    }
}

/// Print to the COM1 serial port
///
/// # Panics
///
/// Panics if writing to the serial port fails.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    
    let mut serial = SerialPort::new(COM1_BASE);
    serial.write_fmt(args).expect("failed to write to serial port");
}