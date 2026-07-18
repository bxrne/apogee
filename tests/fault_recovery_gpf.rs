//! Integration test: fault_recovery_gpf.
//! Verifies Phase 1.2 of PLAN.md: a general protection fault raised
//! from ring 3 kills the offending process and returns control to the
//! kernel instead of panicking the whole machine.
//!
//! Runs without the `#[test_case]` harness because the ring-3 crash
//! demo exercises the real boot-time mapper/frame-allocator, which
//! `test_main` doesn't hand to individual test functions.

#![no_std]
#![no_main]

use apogee::process::PROCESSES;
use apogee::scheduler::SCHEDULER;
use apogee::userland;
use apogee::{QemuExitCode, exit_qemu, memory, serial_print, serial_println, thread};
use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use x86_64::VirtAddr;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    apogee::init();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };

    apogee::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");

    serial_print!("fault_recovery_gpf::general_protection_fault_kills_ring3_process...\t");

    let before_alive = SCHEDULER.processes_alive();
    let pid = userland::create_process_from_program(
        &mut mapper,
        &mut frame_allocator,
        userland::CRASH_GPF_DEMO,
    )
    .expect("failed to create crash-demo process");
    userland::spawn_user_thread(pid);

    // Drive the scheduler until the crashed process's host thread has
    // been reaped. If the GPF handler panicked instead of killing the
    // process, we never get here at all — the panic handler below
    // reports the test as failed.
    while thread::THREADS.lock().ready_count() > 0 {
        thread::yield_now();
    }
    thread::yield_now();

    assert_eq!(
        SCHEDULER.processes_alive(),
        before_alive,
        "crashed process should have been reaped from the scheduler"
    );
    assert!(
        PROCESSES.lock().get(&pid).is_none(),
        "crashed process should have been removed from the registry"
    );

    serial_println!("[ok]");

    exit_qemu(QemuExitCode::Success);
    apogee::hlt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}
