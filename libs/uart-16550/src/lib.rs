#![cfg_attr(not(test), no_std)]

use bitflags::bitflags;
use core::fmt;
use core::sync::atomic::{AtomicPtr, Ordering};
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

pub struct SerialPort {
    data: AtomicPtr<u8>,
    line_sts: AtomicPtr<u8>,
}

impl SerialPort {
    /// Constructs a new UART 16550 compatible driver
    ///
    /// # Panics
    ///
    /// panics is the `clock_frequency` is too large to fit into a u16 if divided by 16.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the given `base` address is valid and points to the correct MMIO region for the UART device.
    pub unsafe fn new(base: usize, clock_frequency: u32, baud_rate: u32) -> Self {
        let base_pointer = base as *mut u8;

        // Safety: ensured by caller
        unsafe {
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
            #[expect(clippy::cast_possible_truncation, reason = "")]
            let div_least = div as u8;
            let div_most = (div >> 8) as u8;

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

            Self {
                data: AtomicPtr::new(base_pointer),
                line_sts: AtomicPtr::new(base_pointer.add(5)),
            }
        }
    }

    fn line_sts(&mut self) -> LineStsFlags {
        // Safety: it is always safe to read the line status
        unsafe { LineStsFlags::from_bits_truncate(*self.line_sts.load(Ordering::Relaxed)) }
    }

    pub fn send(&mut self, data: u8) {
        let self_data = self.data.load(Ordering::Relaxed);
        let mut boff = Backoff::new();

        // Safety: it is always safe to send to the channel
        unsafe {
            match data {
                // special uart handling for backspace
                8 | 0x7F => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write(8);
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write(b' ');
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write(8);
                }
                _ => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY), boff);
                    self_data.write(data);
                }
            }
        }
    }

    pub fn recv(&mut self) -> u8 {
        let self_data = self.data.load(Ordering::Relaxed);
        let mut boff = Backoff::new();

        // Safety: it is always safe to read from the channel
        unsafe {
            wait_for!(self.line_sts().contains(LineStsFlags::INPUT_FULL), boff);
            self_data.read()
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            self.send(c);
        }
        Ok(())
    }
}
