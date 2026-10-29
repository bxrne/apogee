//! Main kernel binary entry point.
//!
//! This is the primary kernel executable that boots via the bootloader
//! and initializes all kernel subsystems: GDT, IDT, memory paging, and heap.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

use apogee::demos;
use apogee::memory;
use apogee::task::Task;
use apogee::task::executor::CoopExecutor;
use apogee::task::keyboard;
use apogee::vga_buffer::{self, Color};
use apogee::{allocator, kdebugln, kinfoln, println};
use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use x86_64::{VirtAddr, structures::paging::Page};

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
    kinfoln!("BOOT", "entered kernel_main");
    apogee::init();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    kdebugln!("MEM ", "offset page table initialized");
    let mut frame_allocator =
        unsafe { memory::MonotonicFrameAllocator::init(&boot_info.memory_map) };
    kdebugln!("MEM ", "boot frame allocator initialized");

    let page = Page::containing_address(VirtAddr::new(0));
    memory::create_example_mapping(page, &mut mapper, &mut frame_allocator);
    kdebugln!("MEM ", "example mapping established");

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");
    kinfoln!(
        "MEM ",
        "heap initialized: {} KiB @ {:#x}",
        allocator::HEAP_SIZE / 1024,
        allocator::HEAP_START
    );

    print_banner(boot_info);

    demos::sample_heap_allocations();

    #[cfg(test)]
    test_main();

    demos::threading_demo();

    demos::userland_demo(&mut mapper, &mut frame_allocator);

    // Spin up the cooperative executor with a sample async task and the
    // asynchronous keyboard handler. `Executor::run` halts the CPU between
    // wakeups and never returns.
    let mut executor = CoopExecutor::new();
    kinfoln!("EXEC", "spawning startup tasks");
    executor.spawn(Task::new(demos::example_task()));
    executor.spawn(Task::new(keyboard::print_keypresses()));
    kinfoln!("BOOT", "startup complete: entering executor loop");
    executor.run();
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
