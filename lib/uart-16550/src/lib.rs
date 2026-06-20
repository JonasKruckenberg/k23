// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(test), no_std)]

use core::fmt;
use core::sync::atomic::{AtomicPtr, Ordering};

use bitflags::bitflags;
use spin::Backoff;

macro_rules! wait_for {
    ($cond:expr, $boff:expr) => {
        while !$cond {
            $boff.spin()
        }
    };
}

bitflags! {
    /// Line status flags
    struct LineStsFlags: u8 {
        const INPUT_FULL = 1;
        // 1 to 4 unknown
        const OUTPUT_EMPTY = 1 << 5;
        // 6 and 7 unknown
    }
}

/// Initializes a 16550 UART at `base` returning a [`Sender`] /
/// [`Receiver`] pair.
///
/// # Panics
///
/// Panics if `clock_frequency` divided by 16 is too large to fit into a `u16`.
///
/// # Safety
///
/// `base` must be a valid pointer to the MMIO register block of a 16550 UART.
pub unsafe fn open(base: usize, clock_frequency: u32, baud_rate: u32) -> (Sender, Receiver) {
    let base_pointer = base as *mut u8;

    // Safety: ensured by caller
    let (data, line_sts) = unsafe {
        let data = base_pointer;
        let int_en = base_pointer.add(1);
        let fifo_ctrl = base_pointer.add(2);
        let line_ctrl = base_pointer.add(3);
        let modem_ctrl = base_pointer.add(4);

        // Disable interrupts
        int_en.write_volatile(0x00);

        // Enable DLAB
        line_ctrl.write_volatile(0x80);

        let div = u16::try_from(clock_frequency.div_ceil(baud_rate * 16)).unwrap();
        let [div_least, div_most] = div.to_le_bytes();

        // Set maximum speed to 38400 bps by configuring DLL and DLM
        data.write_volatile(div_least); // divisor low
        int_en.write_volatile(div_most); // divisor high

        // Disable DLAB and set data word length to 8 bits
        line_ctrl.write_volatile(0x03);

        // Enable FIFO, clear TX/RX queues and
        // set interrupt watermark at 14 bytes
        fifo_ctrl.write_volatile(0xC7);

        // Mark data terminal ready, signal request to send
        // and enable auxiliary output #2 (used as interrupt line for CPU)
        modem_ctrl.write_volatile(0x0B);

        // Enable interrupts
        int_en.write_volatile(0x01);

        (base_pointer, base_pointer.add(5))
    };

    (
        Sender {
            data: AtomicPtr::new(data),
            line_sts: AtomicPtr::new(line_sts),
        },
        Receiver {
            data: AtomicPtr::new(data),
            line_sts: AtomicPtr::new(line_sts),
        },
    )
}

/// The sending (transmit) half of a UART opened with [`open`].
pub struct Sender {
    data: AtomicPtr<u8>,
    line_sts: AtomicPtr<u8>,
}

impl Sender {
    fn line_sts(&self) -> LineStsFlags {
        // Safety: it is always safe to read the line status
        unsafe {
            LineStsFlags::from_bits_truncate(self.line_sts.load(Ordering::Relaxed).read_volatile())
        }
    }

    pub fn send(&self, data: u8) {
        let self_data = self.data.load(Ordering::Relaxed);
        let mut boff = Backoff::new();

        // Safety: it is always safe to send to the channel
        unsafe {
            match data {
                // special uart handling for backspace
                8 | 0x7F => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write_volatile(8);
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write_volatile(b' ');
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write_volatile(8);
                }
                _ => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write_volatile(data);
                }
            }
        }
    }
}

impl fmt::Write for &Sender {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            self.send(c);
        }
        Ok(())
    }
}

/// The receiving half of a UART opened with [`open`].
pub struct Receiver {
    data: AtomicPtr<u8>,
    line_sts: AtomicPtr<u8>,
}

impl Receiver {
    fn line_sts(&self) -> LineStsFlags {
        // Safety: it is always safe to read the line status
        unsafe {
            LineStsFlags::from_bits_truncate(self.line_sts.load(Ordering::Relaxed).read_volatile())
        }
    }

    pub fn recv(&self) -> u8 {
        let self_data = self.data.load(Ordering::Relaxed);
        let mut boff = Backoff::new();

        // Safety: it is always safe to read from the channel
        unsafe {
            wait_for!(self.line_sts().contains(LineStsFlags::INPUT_FULL), boff);
            self_data.read_volatile()
        }
    }
}
