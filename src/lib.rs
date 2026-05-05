//! Apogee - A minimal x86-64 bare-metal kernel.
//!
//! This library provides the core kernel functionality including:
//! - VGA text buffer output
//! - Serial port communication
//! - Interrupt handling (IDT, PIC)
//! - Global Descriptor Table (GDT) and TSS
//! - Memory paging and frame allocation
//! - Dynamic memory allocation (heap) with reference counting
//! - Test framework for QEMU-based testing

#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(abi_x86_interrupt)]
use core::panic::PanicInfo;

extern crate alloc;

#[cfg(test)]
use bootloader::{BootInfo, entry_point};

pub mod allocator;
pub mod gdt;
pub mod interrupts;
pub mod memory;
pub mod scheduler;
pub mod serial;
pub mod task;
pub mod vga_buffer;

pub trait Testable {
    fn run(&self);
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("");
    serial_println!("==============================================");
    serial_println!("  Running {} tests", tests.len());
    serial_println!("==============================================");
    serial_println!("");
    for test in tests {
        test.run();
    }
    serial_println!("");
    serial_println!("----------------------------------------------");
    serial_println!("  All tests passed!");
    serial_println!("----------------------------------------------");
    serial_println!("");
    exit_qemu(QemuExitCode::Success);
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failed);
    hlt_loop()
}

#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(boot_info: &'static BootInfo) -> ! {
    use x86_64::VirtAddr;
    init();

    // Initialise paging + the global heap so unit tests that touch the
    // allocator (Arc, BTreeMap, ArrayQueue, ...) don't fail at runtime.
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");

    test_main();
    hlt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    test_panic_handler(info)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    unsafe { interrupts::PICS.lock().initialize() };
    x86_64::instructions::interrupts::enable();
}
