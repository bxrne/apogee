//! Non-blocking dual-target kernel logger.
//!
//! Writes best-effort logs to both VGA text mode and COM1 serial.
//! If either output is currently locked, that sink is skipped. If both are
//! locked, the message is dropped to avoid blocking in interrupt context.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

use spin::MutexGuard;
use uart_16550::SerialPort;

use crate::serial::SERIAL1;
use crate::vga_buffer::{
    Color, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, WRITER, Writer as VgaWriter,
};

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
    fn prefix(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE>",
            LogLevel::Debug => "DEBUG>",
            LogLevel::Info => "INFO>",
            LogLevel::Warn => "WARN>",
            LogLevel::Error => "ERROR>",
        }
    }

    fn vga_color(self) -> Color {
        match self {
            LogLevel::Trace => Color::DarkGray,
            LogLevel::Debug => Color::LightCyan,
            LogLevel::Info => Color::LightGreen,
            LogLevel::Warn => Color::Yellow,
            LogLevel::Error => Color::LightRed,
        }
    }
}

struct DualWriter<'a> {
    vga: Option<MutexGuard<'a, VgaWriter>>,
    serial: Option<MutexGuard<'a, SerialPort>>,
}

impl Write for DualWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(vga) = self.vga.as_mut() {
            let _ = vga.write_str(s);
        }
        if let Some(serial) = self.serial.as_mut() {
            let _ = serial.write_str(s);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let vga = WRITER.try_lock();
    let serial = SERIAL1.try_lock();

    if vga.is_none() && serial.is_none() {
        DROPPED_MESSAGES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let mut writer = DualWriter { vga, serial };
    let _ = writer.write_fmt(args);
}

#[doc(hidden)]
pub fn _log(level: LogLevel, args: fmt::Arguments) {
    let vga = WRITER.try_lock();
    let serial = SERIAL1.try_lock();

    if vga.is_none() && serial.is_none() {
        DROPPED_MESSAGES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let mut writer = DualWriter { vga, serial };

    if let Some(vga) = writer.vga.as_mut() {
        vga.set_color(level.vga_color(), DEFAULT_BACKGROUND);
    }

    let _ = writer.write_fmt(format_args!("{}", level.prefix()));
    let _ = writer.write_fmt(args);

    if let Some(vga) = writer.vga.as_mut() {
        vga.set_color(DEFAULT_FOREGROUND, DEFAULT_BACKGROUND);
    }
}

/// Number of log messages dropped because both sinks were busy.
pub fn dropped_messages() -> u64 {
    DROPPED_MESSAGES.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => {
        $crate::logger::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! klogln {
    () => ($crate::klog!("\n"));
    ($($arg:tt)*) => ($crate::klog!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! ktrace {
    ($($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Trace, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! ktraceln {
    () => ($crate::ktrace!("\n"));
    ($($arg:tt)*) => ($crate::ktrace!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kdebug {
    ($($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Debug, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! kdebugln {
    () => ($crate::kdebug!("\n"));
    ($($arg:tt)*) => ($crate::kdebug!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kinfo {
    ($($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Info, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! kinfoln {
    () => ($crate::kinfo!("\n"));
    ($($arg:tt)*) => ($crate::kinfo!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kwarn {
    ($($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Warn, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! kwarnln {
    () => ($crate::kwarn!("\n"));
    ($($arg:tt)*) => ($crate::kwarn!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kerror {
    ($($arg:tt)*) => {
        $crate::logger::_log($crate::logger::LogLevel::Error, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! kerrorln {
    () => ($crate::kerror!("\n"));
    ($($arg:tt)*) => ($crate::kerror!("{}\n", format_args!($($arg)*)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_log_level_prefixes() {
        assert_eq!(LogLevel::Trace.prefix(), "TRACE>");
        assert_eq!(LogLevel::Debug.prefix(), "DEBUG>");
        assert_eq!(LogLevel::Info.prefix(), "INFO>");
        assert_eq!(LogLevel::Warn.prefix(), "WARN>");
        assert_eq!(LogLevel::Error.prefix(), "ERROR>");
    }

    #[test_case]
    fn test_log_level_colors() {
        assert_eq!(LogLevel::Trace.vga_color(), Color::DarkGray);
        assert_eq!(LogLevel::Debug.vga_color(), Color::LightCyan);
        assert_eq!(LogLevel::Info.vga_color(), Color::LightGreen);
        assert_eq!(LogLevel::Warn.vga_color(), Color::Yellow);
        assert_eq!(LogLevel::Error.vga_color(), Color::LightRed);
    }

    #[test_case]
    fn test_dropped_messages_is_monotonic() {
        let before = dropped_messages();
        crate::klogln!("logger monotonic dropped counter test");
        let after = dropped_messages();
        assert!(after >= before);
    }
}
