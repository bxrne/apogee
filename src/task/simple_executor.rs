//! A bare-bones, busy-loop task executor.
//!
//! [`SimpleExecutor`] polls every queued task in round-robin order, ignoring
//! waker notifications entirely. This is wasteful for tasks that frequently
//! return `Poll::Pending`, but it is small, easy to reason about, and useful
//! for bring-up and tests where deterministic, side-effect-free polling is
//! more valuable than CPU efficiency. Use [`crate::task::executor::Executor`]
//! for the production path.

use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use alloc::collections::VecDeque;

use super::Task;

/// Minimal FIFO executor.
///
/// Tasks are polled in the order they were spawned; pending tasks are pushed
/// back to the tail of the queue and re-polled on the next iteration.
pub struct SimpleExecutor {
    task_queue: VecDeque<Task>,
}

impl Default for SimpleExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleExecutor {
    /// Creates an empty executor.
    pub fn new() -> SimpleExecutor {
        SimpleExecutor {
            task_queue: VecDeque::new(),
        }
    }

    /// Queues a task for later execution. Tasks are polled in spawn order.
    pub fn spawn(&mut self, task: Task) {
        self.task_queue.push_back(task)
    }

    /// Returns the number of tasks currently waiting to be polled. Mostly
    /// useful for tests.
    pub fn pending_tasks(&self) -> usize {
        self.task_queue.len()
    }

    /// Drains the queue, polling each task to completion. Tasks that return
    /// `Poll::Pending` are re-queued and tried again on the next iteration.
    ///
    /// This will spin until *every* spawned task has finished, so it should
    /// only be used with tasks that are known to terminate.
    pub fn run(&mut self) {
        while let Some(mut task) = self.task_queue.pop_front() {
            let waker = dummy_waker();
            let mut context = Context::from_waker(&waker);
            match task.poll(&mut context) {
                Poll::Ready(()) => {} // task done — drop it
                Poll::Pending => self.task_queue.push_back(task),
            }
        }
    }
}

/// Builds a [`Waker`] whose `wake`/`wake_by_ref` operations are no-ops.
///
/// The simple executor does not look at waker notifications, so a placeholder
/// waker that satisfies the type system is all that is needed.
fn dummy_waker() -> Waker {
    // SAFETY: the vtable functions below uphold the `RawWaker` contract — the
    // `clone` callback returns a fresh, equivalent `RawWaker`, and `wake`,
    // `wake_by_ref`, and `drop` are all no-ops over a null data pointer.
    unsafe { Waker::from_raw(dummy_raw_waker()) }
}

fn dummy_raw_waker() -> RawWaker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        dummy_raw_waker()
    }

    let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
    RawWaker::new(core::ptr::null(), vtable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test_case]
    fn test_new_executor_has_no_pending_tasks() {
        let exec = SimpleExecutor::new();
        assert_eq!(exec.pending_tasks(), 0);
    }

    #[test_case]
    fn test_spawn_increments_pending_tasks() {
        let mut exec = SimpleExecutor::new();
        exec.spawn(Task::new(async {}));
        exec.spawn(Task::new(async {}));
        assert_eq!(exec.pending_tasks(), 2);
    }

    #[test_case]
    fn test_run_drains_all_ready_tasks() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        COUNT.store(0, Ordering::SeqCst);

        let mut exec = SimpleExecutor::new();
        for _ in 0..5 {
            exec.spawn(Task::new(async {
                COUNT.fetch_add(1, Ordering::SeqCst);
            }));
        }
        exec.run();
        assert_eq!(exec.pending_tasks(), 0);
        assert_eq!(COUNT.load(Ordering::SeqCst), 5);
    }

    #[test_case]
    fn test_dummy_waker_is_constructible() {
        // Confirms the waker can be built without UB and forwarded into a
        // Context. Nothing is actually woken — the simple executor
        // doesn't observe wake notifications.
        let waker = dummy_waker();
        let _cx = Context::from_waker(&waker);
        let cloned = waker.clone();
        cloned.wake();
        waker.wake_by_ref();
    }
}
