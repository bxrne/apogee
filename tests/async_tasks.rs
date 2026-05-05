//! Integration test: async_tasks.
//!
//! Drives both the [`SimpleExecutor`] and the production [`Executor`] with a
//! handful of async functions and verifies that:
//!
//! * Multiple `async` tasks can be spawned and run to completion.
//! * `.await` chains across multiple async functions yield the correct value.
//! * Tasks correctly observe shared state across await points.
//! * Pending tasks woken via [`futures_util::task::AtomicWaker`] are
//!   actually re-polled by the production executor.
//!
//! These tests require the heap to be initialised, so the entry point sets
//! up paging + the global allocator before invoking `test_main`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(apogee::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::sync::Arc;
use apogee::allocator;
use apogee::task::executor::Executor;
use apogee::task::simple_executor::SimpleExecutor;
use apogee::task::Task;
use bootloader::BootInfo;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll};
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;
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
fn panic(info: &core::panic::PanicInfo) -> ! {
    apogee::test_panic_handler(info)
}

// ---- counters used to share state between the test and async tasks --------

static SIMPLE_COUNTER: AtomicUsize = AtomicUsize::new(0);
static EXECUTOR_COUNTER: AtomicUsize = AtomicUsize::new(0);
static AWAITED_VALUE: AtomicUsize = AtomicUsize::new(0);

async fn produce_number() -> u32 {
    42
}

async fn awaiting_task() {
    let n = produce_number().await;
    AWAITED_VALUE.store(n as usize, Ordering::SeqCst);
}

async fn bump_simple() {
    SIMPLE_COUNTER.fetch_add(1, Ordering::SeqCst);
}

async fn bump_executor() {
    EXECUTOR_COUNTER.fetch_add(1, Ordering::SeqCst);
}

// ---- tests ----------------------------------------------------------------

#[test_case]
fn simple_executor_runs_multiple_tasks() {
    SIMPLE_COUNTER.store(0, Ordering::SeqCst);
    let mut exec = SimpleExecutor::new();
    for _ in 0..4 {
        exec.spawn(Task::new(bump_simple()));
    }
    exec.run();
    assert_eq!(SIMPLE_COUNTER.load(Ordering::SeqCst), 4);
}

#[test_case]
fn executor_runs_and_drains_ready_tasks() {
    EXECUTOR_COUNTER.store(0, Ordering::SeqCst);
    let mut exec = Executor::new();
    for _ in 0..3 {
        exec.spawn(Task::new(bump_executor()));
    }
    // We can't call `exec.run()` because it's `-> !`. The internal helper
    // would do, but it's private; instead, exercise the public surface by
    // spawning + relying on the queue draining via `run_ready_tasks` inside
    // the spawn->loop path of a tiny custom driver.
    drive_until_idle(&mut exec, 8);
    assert_eq!(EXECUTOR_COUNTER.load(Ordering::SeqCst), 3);
    assert_eq!(exec.task_count(), 0);
}

#[test_case]
fn await_chain_propagates_value() {
    AWAITED_VALUE.store(0, Ordering::SeqCst);
    let mut exec = SimpleExecutor::new();
    exec.spawn(Task::new(awaiting_task()));
    exec.run();
    assert_eq!(AWAITED_VALUE.load(Ordering::SeqCst), 42);
}

#[test_case]
fn pending_task_is_repolled_after_wake() {
    // Build a future that returns Pending the first time it is polled and
    // Ready the second. A waker registered via `AtomicWaker` is fired from
    // the test driver to demonstrate the wake -> repoll round-trip.
    static WAKER: AtomicWaker = AtomicWaker::new();
    static POLLS: AtomicUsize = AtomicUsize::new(0);
    static DONE: AtomicUsize = AtomicUsize::new(0);

    POLLS.store(0, Ordering::SeqCst);
    DONE.store(0, Ordering::SeqCst);

    struct WakeOnSecondPoll;
    impl Future for WakeOnSecondPoll {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let n = POLLS.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                WAKER.register(cx.waker());
                Poll::Pending
            } else {
                DONE.store(1, Ordering::SeqCst);
                Poll::Ready(())
            }
        }
    }

    let mut exec = Executor::new();
    exec.spawn(Task::new(async {
        WakeOnSecondPoll.await;
    }));

    // First drain: future returns Pending, registers the waker.
    drive_until_idle(&mut exec, 1);
    assert_eq!(POLLS.load(Ordering::SeqCst), 1);
    assert_eq!(DONE.load(Ordering::SeqCst), 0);
    assert_eq!(exec.task_count(), 1, "task should still be alive");

    // Fire the waker — this enqueues the task id back onto the ready queue.
    WAKER.wake();

    // Second drain: future is repolled and completes.
    drive_until_idle(&mut exec, 4);
    assert_eq!(DONE.load(Ordering::SeqCst), 1);
    assert_eq!(exec.task_count(), 0);
}

#[test_case]
fn arc_array_queue_round_trip() {
    // The executor relies on `Arc<ArrayQueue<TaskId>>` for the ready queue;
    // make sure pushing/popping across Arc clones works as expected with the
    // global slab allocator.
    let q: Arc<ArrayQueue<u8>> = Arc::new(ArrayQueue::new(4));
    let producer = q.clone();
    producer.push(7).unwrap();
    producer.push(11).unwrap();
    assert_eq!(q.pop(), Some(7));
    assert_eq!(q.pop(), Some(11));
    assert_eq!(q.pop(), None);
}

/// Helper that mimics what `Executor::run` does, minus the `hlt` step, so
/// tests can observe state between drains without halting the CPU.
///
/// We reach into the executor by spawning a sentinel task that asserts the
/// queue has drained, but a simpler approach is to expose the private
/// behaviour transitively: the only way to drive the executor without
/// `run()` is via spawn -> waker -> repeat, and `run_ready_tasks` is
/// `pub(super)`. Since we can't call it directly from this integration test,
/// we instead manually loop `run_once`-style by spawning a trivial yield
/// task that re-polls the queue.
///
/// In practice, after `spawn` enqueues all task ids the executor is already
/// runnable; we just need to give it a chance to poll. To do that without
/// `run()`, we call into the executor through an inherent method we exposed
/// on the `Executor` type for tests. This wrapper centralises the call so
/// the rest of the test file stays readable.
fn drive_until_idle(exec: &mut Executor, max_iterations: usize) {
    for _ in 0..max_iterations {
        let before = exec.task_count();
        exec.run_ready_tasks_for_test();
        if exec.task_count() == before && exec.task_count() != 0 {
            // No progress and tasks remain pending — they're presumably
            // waiting on a waker, so further polling won't help.
            return;
        }
        if exec.task_count() == 0 {
            return;
        }
    }
}
