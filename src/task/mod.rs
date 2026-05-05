//! Cooperative multitasking primitives built on top of Rust's `async`/`await`.
//!
//! This module provides the building blocks for running asynchronous tasks in
//! the kernel:
//!
//! * [`Task`]: a heap-allocated, pinned, dynamically-dispatched future that
//!   produces no value (it is run for its side effects only).
//! * [`TaskId`]: a process-wide unique identifier for a task, used by the
//!   waker machinery to enqueue specific tasks for re-polling.
//! * [`simple_executor`]: a basic, busy-loop executor that ignores wakers —
//!   useful for bring-up and tests.
//! * [`executor`]: the production executor that uses waker notifications to
//!   only poll tasks that have signalled progress.
//! * [`keyboard`]: the asynchronous keyboard input task and its supporting
//!   scancode stream / queue.

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll};

use alloc::boxed::Box;

pub mod executor;
pub mod keyboard;
pub mod simple_executor;

/// A unique identifier assigned to every spawned [`Task`].
///
/// Used by [`executor::Executor`] as a key into its task map and by
/// [`executor::TaskWaker`] to push specific tasks back onto the ready queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

impl TaskId {
    /// Generates a fresh, process-wide unique [`TaskId`].
    ///
    /// Uses a single `AtomicU64` counter incremented with `Relaxed` ordering;
    /// the only requirement on the counter is uniqueness, not any particular
    /// happens-before relationship.
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the underlying numeric value of this id (mostly for debugging
    /// and tests).
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// A cooperative, heap-allocated unit of asynchronous work.
///
/// A `Task` owns a pinned, boxed future. The future yields `()` because tasks
/// run for their side effects only — return values are not surfaced through
/// this abstraction.
pub struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    /// Wraps a future in a `Task`, allocating it on the heap and pinning it.
    ///
    /// The `'static` bound ensures the task can outlive the call site, which
    /// is required because the executor may run it at any later point.
    pub fn new(future: impl Future<Output = ()> + 'static) -> Task {
        Task {
            id: TaskId::new(),
            future: Box::pin(future),
        }
    }

    /// Returns this task's unique id.
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Polls the wrapped future once.
    ///
    /// Visible to the rest of the crate so executors can drive the future,
    /// but kept out of the public surface so user code can only interact via
    /// the executor.
    pub(crate) fn poll(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    #[test_case]
    fn test_task_ids_are_unique_and_monotonic() {
        let a = TaskId::new();
        let b = TaskId::new();
        let c = TaskId::new();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(a.as_u64() < b.as_u64());
        assert!(b.as_u64() < c.as_u64());
    }

    #[test_case]
    fn test_task_carries_its_id() {
        let task = Task::new(async {});
        let reported = task.id();
        // Calling `.id()` again must yield the exact same value.
        assert_eq!(reported, task.id());
    }

    #[test_case]
    fn test_task_runs_future_side_effects() {
        // Drive a trivial future to completion using a no-op waker to verify
        // that `Task::poll` actually advances the underlying state machine.
        use core::task::{Context, RawWaker, RawWakerVTable, Waker};

        static RAN: AtomicBool = AtomicBool::new(false);
        RAN.store(false, Ordering::SeqCst);

        let mut task = Task::new(async {
            RAN.store(true, Ordering::SeqCst);
        });

        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(core::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
        let raw = RawWaker::new(core::ptr::null(), &VTABLE);
        // SAFETY: the vtable functions are no-ops and uphold the contract.
        let waker = unsafe { Waker::from_raw(raw) };
        let mut cx = Context::from_waker(&waker);

        assert_eq!(task.poll(&mut cx), Poll::Ready(()));
        assert!(RAN.load(Ordering::SeqCst));
    }
}
