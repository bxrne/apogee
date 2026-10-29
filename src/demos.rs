//! Boot-time demo programs and smoke checks.
//!
//! These exercises drive the kernel subsystems end to end: the global heap,
//! the preemptible thread scheduler, the async task executor and the ring-3
//! user-land process boundary. They form the "hello world" of the microkernel:
//! just enough wiring to watch every ring work before real drivers land.

use crate::kdebugln;
use crate::kerrorln;
use crate::kinfoln;
use crate::thread;
use crate::userland;
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use x86_64::structures::paging::OffsetPageTable;

/// Smoke test the global allocator: `Box`, `Vec` and `Rc` across the
/// fixed-size-block and linked-list fallback layers.
pub fn sample_heap_allocations() {
    kdebugln!("HEAP", "smoke test: allocating Box, Vec, and Rc");

    let heap_value = Box::new(41);
    kdebugln!("HEAP", "heap_value at {:p}", heap_value);

    let mut vec = Vec::new();
    for i in 0..500 {
        vec.push(i);
    }
    kdebugln!("HEAP", "vec backing slice at {:p}", vec.as_slice());

    let reference_counted = Rc::new([1, 2, 3]);
    let cloned_reference = reference_counted.clone();
    kdebugln!(
        "HEAP",
        "rc strong_count={} before drop",
        Rc::strong_count(&cloned_reference)
    );
    core::mem::drop(reference_counted);
    kdebugln!(
        "HEAP",
        "rc strong_count={} after drop",
        Rc::strong_count(&cloned_reference)
    );
}

/// Spin up two kernel threads that exercise the real context-switch path:
/// they each print, call [`thread::yield_now`] to round-robin with the
/// bootstrap thread and each other, and finally call [`thread::exit_thread`]
/// to tear themselves down. Once both have exited, the bootstrap thread (this
/// function's caller) regains the CPU and execution falls through to the
/// async executor.
pub fn threading_demo() {
    kinfoln!("THRD", "spawning two kernel threads");
    thread::spawn_kernel(worker_a);
    thread::spawn_kernel(worker_b);

    // Drive the scheduler from the bootstrap thread until every spawned
    // thread has called `exit_thread` and the ready queue is empty again.
    while thread::ready_count() > 0 {
        thread::yield_now();
    }

    kinfoln!("THRD", "all kernel threads exited; resuming bootstrap");
}

extern "C" fn worker_a() -> ! {
    let id = thread::current_id().as_u64();
    kinfoln!("THRD", "thread {}: A running, about to yield", id);
    thread::yield_now();
    kinfoln!("THRD", "thread {}: A resumed, exiting", id);
    thread::exit_thread();
}

extern "C" fn worker_b() -> ! {
    let id = thread::current_id().as_u64();
    kinfoln!("THRD", "thread {}: B running, about to yield", id);
    thread::yield_now();
    kinfoln!("THRD", "thread {}: B resumed, exiting", id);
    thread::exit_thread();
}

/// Trivial async function used to demonstrate the executor end-to-end.
async fn async_number() -> u32 {
    42
}

/// Example async task: awaits an async value and prints it. Verifies the
/// state-machine + executor pipeline works for a non-trivial future.
pub async fn example_task() {
    let number = async_number().await;
    kdebugln!("EXEC", "example async task completed with {}", number);
}

/// Bring up a single user-mode process from the embedded demo program, then
/// drive the kernel-thread scheduler until the user process has called
/// `sys_exit` and its host kernel thread has been reaped. After this returns,
/// control is back on the bootstrap thread and execution falls through to the
/// async executor.
pub fn userland_demo(
    mapper: &mut OffsetPageTable<'static>,
    frame_allocator: &mut crate::memory::MonotonicFrameAllocator,
) {
    kinfoln!("USER", "bringing up demo ring-3 process");
    let pid = match userland::create_demo_process(mapper, frame_allocator) {
        Ok(pid) => pid,
        Err(e) => {
            kerrorln!("USER", "failed to create demo process: {}", e);
            return;
        }
    };

    userland::spawn_user_thread(pid);

    // Round-robin between the bootstrap thread and the host kernel
    // thread until the user process has exited and its host thread is
    // gone (drained out of the reaper slot).
    while thread::ready_count() > 0 {
        thread::yield_now();
    }
    // One more yield to drain the reaper if the user thread was the
    // last to exit.
    thread::yield_now();

    kinfoln!("USER", "demo process complete; resuming bootstrap");
}
