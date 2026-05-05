//! Main kernel binary entry point.
//!
//! This is the primary kernel executable that boots via the bootloader
//! and initializes all kernel subsystems: GDT, IDT, memory paging, and heap.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]
extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use apogee::task::Task;
use apogee::task::executor::CoOpExecuter;
use apogee::task::keyboard;
use apogee::vga_buffer::{self, Color};
use apogee::{allocator, println};
use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use x86_64::{VirtAddr, structures::paging::Page};

mod memory;

entry_point!(kernel_main);
pub const BANNER_ART: &str = r#"
 ________  ________  ________  ________  _______   _______      
|\   __  \|\   __  \|\   __  \|\   ____\|\  ___ \ |\  ___ \     
\ \  \|\  \ \  \|\  \ \  \|\  \ \  \___|\ \   __/|\ \   __/|    
 \ \   __  \ \   ____\ \  \\\  \ \  \  __\ \  \_|/_\ \  \_|/__  
  \ \  \ \  \ \  \___|\ \  \\\  \ \  \|\  \ \  \_|\ \ \  \_|\ \ 
   \ \__\ \__\ \__\    \ \_______\ \_______\ \_______\ \_______\
    \|__|\|__|\|__|     \|_______|\|_______|\|_______|\|_______|
"#;

fn print_banner(boot_info: &BootInfo) {
    const PAGE_SIZE: usize = 4096;

    vga_buffer::set_color(Color::Brown, Color::Black);
    println!("{}", BANNER_ART);
    println!("apogee x86_64 kernel");
    vga_buffer::reset_color();
    println!();

    let heap_kib = allocator::HEAP_SIZE / 1024;
    let heap_pages = allocator::HEAP_SIZE / PAGE_SIZE;
    let memory_regions = boot_info.memory_map.iter().count();

    println!("> heap storage: {} KiB ({} pages)", heap_kib, heap_pages);
    println!(
        "> heap start: {:#x}  phys offset: {:#x}",
        allocator::HEAP_START,
        boot_info.physical_memory_offset
    );
    println!("> memory map: {} regions  paging: enabled", memory_regions);
    println!("> allocator: fixed-size blocks  async executor: ready");
    println!();
}

/// Main entry point for the kernel.
/// Called by the bootloader after it sets up initial memory and boot info.
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    apogee::kinfoln!("boot: entered kernel_main");
    apogee::init();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    apogee::kdebugln!("memory: offset page table initialized");
    let mut frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };
    apogee::kdebugln!("memory: boot frame allocator initialized");

    let page = Page::containing_address(VirtAddr::new(0));
    memory::create_example_mapping(page, &mut mapper, &mut frame_allocator);
    apogee::kdebugln!("memory: example mapping established");

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");
    apogee::kinfoln!(
        "heap initialized: {} KiB @ {:#x}",
        allocator::HEAP_SIZE / 1024,
        allocator::HEAP_START
    );

    print_banner(boot_info);

    sample_heap_allocations();

    #[cfg(test)]
    test_main();

    // Spin up the cooperative executor with a sample async task and the
    // asynchronous keyboard handler. `Executor::run` halts the CPU between
    // wakeups and never returns.
    let mut executor = CoOpExecuter::new();
    apogee::kinfoln!("executor: spawning startup tasks");
    executor.spawn(Task::new(example_task()));
    executor.spawn(Task::new(keyboard::print_keypresses()));
    apogee::kinfoln!("startup complete: entering executor loop");
    executor.run();
}

fn sample_heap_allocations() {
    apogee::kdebugln!("heap smoke test: allocating Box, Vec, and Rc");

    let heap_value = Box::new(41);
    apogee::kdebugln!("heap_value at {:p}", heap_value);

    let mut vec = Vec::new();
    for i in 0..500 {
        vec.push(i);
    }
    apogee::kdebugln!("vec backing slice at {:p}", vec.as_slice());

    let reference_counted = Rc::new([1, 2, 3]);
    let cloned_reference = reference_counted.clone();
    apogee::kdebugln!(
        "rc strong_count={} before drop",
        Rc::strong_count(&cloned_reference)
    );
    core::mem::drop(reference_counted);
    apogee::kdebugln!(
        "rc strong_count={} after drop",
        Rc::strong_count(&cloned_reference)
    );
}

/// Trivial async function used to demonstrate the executor end-to-end.
async fn async_number() -> u32 {
    42
}

/// Example async task: awaits an async value and prints it. Verifies the
/// state-machine + executor pipeline works for a non-trivial future.
async fn example_task() {
    let number = async_number().await;
    apogee::kdebugln!("example async task completed with {}", number);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    apogee::hlt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_banner_art_is_non_empty() {
        assert!(!BANNER_ART.is_empty());
    }

    #[test_case]
    fn test_banner_art_contains_apogee() {
        assert!(BANNER_ART.contains("apogee"));
    }

    #[test_case]
    fn test_banner_art_contains_kernel_marker() {
        assert!(BANNER_ART.contains("kernel"));
    }
}
