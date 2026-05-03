//! Integration test: basic boot.
//! Verifies the kernel boots and that `println!` works without panicking.
//! Uses the shared test infrastructure from the `apogee` library.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

use apogee::println;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    test_main();
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}

// Tests

#[test_case]
fn test_println() {
    println!("test_println output");
}
