#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

use apogee::println;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    apogee::init();

    println!("apogee kernel starting...");

    #[allow(unconditional_recursion)]
    fn stack_overflow() {
        stack_overflow();
        volatile::Volatile::new(0u64).read();
    }
    stack_overflow();

    #[cfg(test)]
    test_main();

    loop {}
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}
