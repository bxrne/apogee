//! Integration test: heap_allocation.
//! Tests dynamic memory allocation including Box, Vec, and Rc (reference counting).
//! Requires full kernel initialization with memory mapping and heap setup.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

#[macro_use]
extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use apogee::{allocator, serial_print, serial_println};
use bootloader::BootInfo;
use core::panic::PanicInfo;
use x86_64::VirtAddr;

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    apogee::init();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { apogee::memory::init(phys_mem_offset) };
    let mut frame_allocator =
        unsafe { apogee::memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };

    allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");

    test_main();
    apogee::hlt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}

#[test_case]
fn test_box_allocation() {
    serial_print!("heap_allocation::test_box...\t");
    let boxed = Box::new(42);
    assert_eq!(*boxed, 42);
    serial_println!("[ok]");
}

#[test_case]
fn test_vec_allocation() {
    serial_print!("heap_allocation::test_vec...\t");
    let mut vec = Vec::new();
    for i in 0..100 {
        vec.push(i);
    }
    assert_eq!(vec.len(), 100);
    assert_eq!(vec[50], 50);
    serial_println!("[ok]");
}

#[test_case]
fn test_rc_single() {
    serial_print!("heap_allocation::test_rc_single...\t");
    let rc = Rc::new(100);
    assert_eq!(*rc, 100);
    assert_eq!(Rc::strong_count(&rc), 1);
    serial_println!("[ok]");
}

#[test_case]
fn test_rc_clone() {
    serial_print!("heap_allocation::test_rc_clone...\t");
    let original = Rc::new([1, 2, 3]);
    let clone1 = original.clone();
    let clone2 = original.clone();

    assert_eq!(Rc::strong_count(&original), 3);
    assert_eq!(*clone1, [1, 2, 3]);
    assert_eq!(*clone2, [1, 2, 3]);

    core::mem::drop(original);
    assert_eq!(Rc::strong_count(&clone1), 2);

    serial_println!("[ok]");
}

#[test_case]
fn test_rc_drop() {
    serial_print!("heap_allocation::test_rc_drop...\t");
    let rc = Rc::new("test");
    let clone = rc.clone();

    assert_eq!(Rc::strong_count(&clone), 2);

    core::mem::drop(rc);
    assert_eq!(Rc::strong_count(&clone), 1);
    assert_eq!(*clone, "test");

    serial_println!("[ok]");
}

#[test_case]
fn test_box_vec_combo() {
    serial_print!("heap_allocation::test_box_vec_combo...\t");
    let boxed_vec = Box::new(vec![1, 2, 3, 4, 5]);
    assert_eq!(boxed_vec.len(), 5);
    assert_eq!(boxed_vec[2], 3);
    serial_println!("[ok]");
}

#[test_case]
fn test_multiple_allocations() {
    serial_print!("heap_allocation::test_multiple_allocations...\t");
    let b1 = Box::new(1i32);
    let b2 = Box::new(2i32);
    let b3 = Box::new(3i32);

    let v1: Vec<Box<i32>> = vec![b1, b2, b3];
    let sum: i32 = v1.iter().map(|b| **b).sum();
    assert_eq!(sum, 6);
    serial_println!("[ok]");
}