use crate::board_info::Serial;
use spin::Mutex;
use uart_16550::SerialPort;

pub static UART: Mutex<Option<SerialPort>> = Mutex::new(None);

pub fn init(serial_info: &Serial) {
    unsafe {
        *UART.lock() = Some(SerialPort::new(
            serial_info.mmio_regs.start,
            serial_info.clock_frequency,
            38400, // TODO maybe make this configurable?
        ));
    }
}
