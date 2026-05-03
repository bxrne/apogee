#![no_std]
#![no_main]

use core::panic::PanicInfo;

const VGA_BUFFER: *mut u8 = 0xb8000 as *mut u8;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let message = b"Hello, World!";
    for (i, &byte) in message.iter().enumerate() {
        unsafe {
            *VGA_BUFFER.offset(i as isize * 2) = byte; // Write character
            *VGA_BUFFER.offset(i as isize * 2 + 1) = 0x07; // Set color (white on black)
        }
    }

    loop {}
}



