//! Non-blocking serial kernel logger.
//!
//! Writes best-effort logs to COM1 serial. If the serial port is currently
//! locked, the message is dropped to avoid blocking in interrupt context.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

use spin::MutexGuard;
use uart_16550::SerialPort;

use crate::serial::SERIAL1;

static DROPPED_MESSAGES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn tag(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRC",
            LogLevel::Debug => "DBG",
            LogLevel::Info => "INF",
            LogLevel::Warn => "WRN",
            LogLevel::Error => "ERR",
        }
    }
}

struct SerialWriter<'a> {
    serial: Option<MutexGuard<'a, SerialPort>>,
}

impl Write for SerialWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(serial) = self.serial.as_mut() {
            let _ = serial.write_str(s);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let serial = SERIAL1.try_lock();

    if serial.is_none() {
        DROPPED_MESSAGES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let mut writer = SerialWriter { serial };
    let _ = writer.write_fmt(args);
}

#[doc(hidden)]
pub fn _log(level: LogLevel, source: &str, args: fmt::Arguments) {
    let serial = SERIAL1.try_lock();

    if serial.is_none() {
        DROPPED_MESSAGES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let mut writer = SerialWriter { serial };

    let short: [u8; 4] = {
        let mut buf = [b' '; 4];
        let bytes = source.as_bytes();
        let copy = bytes.len().min(4);
        for (dst, src) in buf[..copy].iter_mut().zip(&bytes[..copy]) {
            *dst = src.to_ascii_uppercase();
        }
        buf
    };

    let short = unsafe { core::str::from_utf8_unchecked(&short) };
    let _ = writer.write_fmt(format_args!("{};{} [{}]\n", level.tag(), short, args));
}

/// Number of log messages dropped because the serial port was busy.
pub fn dropped_messages() -> u64 {
    DROPPED_MESSAGES.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => {
        $crate::logger::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! klogln {
    () => ($crate::klog!("\n"));
    ($($arg:tt)*) => ($crate::klog!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! ktrace {
    ($source:expr, $($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Trace, $source, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! ktraceln {
    ($source:expr) => ($crate::ktrace!($source, ""));
    ($source:expr, $($arg:tt)*) => ($crate::ktrace!($source, $($arg)*));
}

#[macro_export]
macro_rules! kdebug {
    ($source:expr, $($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Debug, $source, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kdebugln {
    ($source:expr) => ($crate::kdebug!($source, ""));
    ($source:expr, $($arg:tt)*) => ($crate::kdebug!($source, $($arg)*));
}

#[macro_export]
macro_rules! kinfo {
    ($source:expr, $($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Info, $source, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kinfoln {
    ($source:expr) => ($crate::kinfo!($source, ""));
    ($source:expr, $($arg:tt)*) => ($crate::kinfo!($source, $($arg)*));
}

#[macro_export]
macro_rules! kwarn {
    ($source:expr, $($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Warn, $source, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kwarnln {
    ($source:expr) => ($crate::kwarn!($source, ""));
    ($source:expr, $($arg:tt)*) => ($crate::kwarn!($source, $($arg)*));
}

#[macro_export]
macro_rules! kerror {
    ($source:expr, $($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Error, $source, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kerrorln {
    ($source:expr) => ($crate::kerror!($source, ""));
    ($source:expr, $($arg:tt)*) => ($crate::kerror!($source, $($arg)*));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_log_level_tags() {
        assert_eq!(LogLevel::Trace.tag(), "TRC");
        assert_eq!(LogLevel::Debug.tag(), "DBG");
        assert_eq!(LogLevel::Info.tag(), "INF");
        assert_eq!(LogLevel::Warn.tag(), "WRN");
        assert_eq!(LogLevel::Error.tag(), "ERR");
    }

    #[test_case]
    fn test_dropped_messages_is_monotonic() {
        let before = dropped_messages();
        crate::klogln!("logger monotonic dropped counter test");
        let after = dropped_messages();
        assert!(after >= before);
    }
}
