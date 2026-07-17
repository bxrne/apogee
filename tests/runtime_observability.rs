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
use apogee::{kdebugln, kerrorln, kinfoln, ktraceln, kwarnln};
use apogee::task::executor::CoOpExecuter;
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

    ktraceln!("TEST", "trace boot visibility test");
    kdebugln!("TEST", "debug boot visibility test");
    kinfoln!("TEST", "info boot visibility test");
    kwarnln!("TEST", "warn boot visibility test");
    kerrorln!("TEST", "error boot visibility test");

    let after = apogee::logger::dropped_messages();
    assert!(after >= before);
}

#[test_case]
fn test_scheduler_observability_local_round_trip() {
    // Use a private scheduler so we don't perturb the global counters
    // that other tests/observability code reads.
    let scheduler = apogee::scheduler::Scheduler::new();
    assert_eq!(scheduler.tasks_alive(), 0);
    assert_eq!(scheduler.tasks_spawned(), 0);

    scheduler.note_task_spawned();
    scheduler.note_task_spawned();
    assert_eq!(scheduler.tasks_alive(), 2);
    assert_eq!(scheduler.tasks_spawned(), 2);

    scheduler.note_task_completed();
    assert_eq!(scheduler.tasks_alive(), 1);
    assert_eq!(scheduler.tasks_completed(), 1);
}

#[test_case]
fn test_executor_spawn_updates_global_scheduler_counts() {
    let before_alive = apogee::scheduler::SCHEDULER.tasks_alive();
    let before_spawned = apogee::scheduler::SCHEDULER.tasks_spawned();
    let before_completed = apogee::scheduler::SCHEDULER.tasks_completed();

    let mut exec = CoOpExecuter::new();
    exec.spawn(Task::new(async {}));
    exec.spawn(Task::new(async {}));

    assert_eq!(
        apogee::scheduler::SCHEDULER.tasks_spawned(),
        before_spawned + 2
    );
    assert_eq!(apogee::scheduler::SCHEDULER.tasks_alive(), before_alive + 2);

    exec.run_ready_tasks_for_test();
    assert_eq!(apogee::scheduler::SCHEDULER.tasks_alive(), before_alive);
    assert_eq!(
        apogee::scheduler::SCHEDULER.tasks_completed(),
        before_completed + 2
    );
}

#[test_case]
fn test_timer_interrupt_advances_tick_counter() {
    let start_ticks = apogee::scheduler::SCHEDULER.ticks();

    for _ in 0..512 {
        x86_64::instructions::hlt();
        let now = apogee::scheduler::SCHEDULER.ticks();
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
