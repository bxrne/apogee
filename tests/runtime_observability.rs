//! Integration test: runtime_observability.
//! Verifies logger and scheduler observability primitives.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use apogee::allocator;
use apogee::task::Task;
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

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");

    test_main();
    apogee::hlt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}

#[test_case]
fn test_logger_macros_are_callable() {
    let before = apogee::logger::dropped_messages();

    apogee::ktraceln!("trace boot visibility test");
    apogee::kdebugln!("debug boot visibility test");
    apogee::kinfoln!("info boot visibility test");
    apogee::kwarnln!("warn boot visibility test");
    apogee::kerrorln!("error boot visibility test");

    let after = apogee::logger::dropped_messages();
    assert!(after >= before);
}

#[test_case]
fn test_scheduler_local_round_trip() {
    let mut scheduler = apogee::scheduler::Scheduler::new();
    let task = Task::new(async {});
    let id = task.id();

    scheduler.add_task(&task);
    assert_eq!(scheduler.queue_len(), 1);
    assert_eq!(scheduler.pop_next_task(), Some(id));
    assert_eq!(scheduler.pop_next_task(), None);
}

#[test_case]
fn test_timer_interrupt_advances_tick_counter() {
    let start_ticks = { apogee::scheduler::SCHEDULER.lock().ticks() };

    for _ in 0..512 {
        x86_64::instructions::hlt();
        let now = { apogee::scheduler::SCHEDULER.lock().ticks() };
        if now > start_ticks {
            return;
        }
    }

    panic!("timer tick counter did not advance");
}

#[test_case]
fn test_syscall_vector_constant() {
    assert_eq!(apogee::interrupts::SYSCALL_VECTOR, 0x80);
}
