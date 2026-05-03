use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;

pub const SERIAL_PORT_BASE: u16 = 0x3F8;

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(SERIAL_PORT_BASE) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        // avoid deadlock if an interrupt occurs while the buffer is locked
        SERIAL1.lock().write_fmt(args).unwrap();
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_serial_port_base_address() {
        assert_eq!(SERIAL_PORT_BASE, 0x3F8);
    }

    #[test_case]
    fn test_serial_port_base_valid_range() {
        assert!(SERIAL_PORT_BASE > 0);
    }

    #[test_case]
    fn test_serial_port_base_standard_com1() {
        assert_eq!(SERIAL_PORT_BASE, 0x3F8);
    }
}
