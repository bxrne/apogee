//! Production-quality cooperative executor with waker support.
//!
//! Unlike [`super::simple_executor::SimpleExecutor`], this executor only
//! polls tasks that have been signalled as ready via a [`Waker`]. Tasks are
//! identified by [`TaskId`] and woken by pushing their id onto a shared
//! ready queue ([`task_queue`](Executor::task_queue)). When the queue is
//! empty the executor halts the CPU with `hlt` until the next interrupt
//! delivers more work.

use core::task::{Context, Poll, Waker};

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::task::Wake;
use crossbeam_queue::ArrayQueue;

use super::{Task, TaskId};

/// Capacity of the ready-task queue. Must comfortably exceed the highest
/// expected number of concurrently-runnable tasks; pushes from interrupt
/// handlers panic if the queue is full.
const TASK_QUEUE_CAPACITY: usize = 100;

/// Cooperative task executor with waker support.
pub struct CoOpExecuter {
    /// All currently spawned tasks, indexed by id for O(log n) lookup when a
    /// waker fires.
    tasks: BTreeMap<TaskId, Task>,
    /// Shared ready queue. Cloned wakers push task ids back onto it; the
    /// executor pops them in [`Self::run_ready_tasks`].
    task_queue: Arc<ArrayQueue<TaskId>>,
    /// Cached `Waker` instances, one per task, to avoid the cost of
    /// constructing a new `Arc<TaskWaker>` on every poll. Also keeps the
    /// reference count from being decremented inside the interrupt handler,
    /// which would otherwise risk dropping into the allocator there.
    waker_cache: BTreeMap<TaskId, Waker>,
    /// Tracks tasks that have already been started (first poll happened), so
    /// we can emit a startup log exactly once per task.
    started_tasks: BTreeSet<TaskId>,
}

impl Default for CoOpExecuter {
    fn default() -> Self {
        Self::new()
    }
}

impl CoOpExecuter {
    /// Builds a new executor with empty task and waker collections.
    pub fn new() -> Self {
        CoOpExecuter {
            tasks: BTreeMap::new(),
            task_queue: Arc::new(ArrayQueue::new(TASK_QUEUE_CAPACITY)),
            waker_cache: BTreeMap::new(),
            started_tasks: BTreeSet::new(),
        }
    }

    /// Spawns a task: inserts it into [`Self::tasks`] and immediately marks
    /// it ready by pushing its id onto [`Self::task_queue`].
    ///
    /// Panics if a task with the same id is already present (which would
    /// indicate a [`TaskId`] generation bug) or if the ready queue is full.
    pub fn spawn(&mut self, task: Task) {
        let task_id = task.id();
        if self.tasks.insert(task_id, task).is_some() {
            panic!("task with same ID already in tasks");
        }
        self.task_queue.push(task_id).expect("queue full");
    }

    /// Number of tasks currently owned by the executor (whether ready or
    /// waiting). Mostly used in tests.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Polls every currently-ready task once.
    ///
    /// Tasks that return `Poll::Pending` will only be re-polled after their
    /// waker fires; tasks that return `Poll::Ready` are removed entirely
    /// (along with their cached waker).
    fn run_ready_tasks(&mut self) {
        // Destructure `self` so the closure passed to `or_insert_with`
        // doesn't try to borrow it whole. (See rust-lang RFC 2229.)
        let Self {
            tasks,
            task_queue,
            waker_cache,
            started_tasks,
        } = self;

        while let Some(task_id) = task_queue.pop() {
            let total_tasks = tasks.len();
            let task = match tasks.get_mut(&task_id) {
                Some(task) => task,
                // Wake fired for a task that has already finished and been
                // removed — harmless, just skip it.
                None => continue,
            };
            let waker = waker_cache
                .entry(task_id)
                .or_insert_with(|| TaskWaker::new(task_id, task_queue.clone()));

            if started_tasks.insert(task_id) {
                crate::println!(
                    "[task {}] started (total tasks: {})",
                    task_id.as_u64(),
                    total_tasks
                );
            }

            let mut context = Context::from_waker(waker);
            match task.poll(&mut context) {
                Poll::Ready(()) => {
                    tasks.remove(&task_id);
                    waker_cache.remove(&task_id);
                    started_tasks.remove(&task_id);
                }
                Poll::Pending => {}
            }
        }
    }

    /// Runs the executor forever: poll all ready tasks, then sleep until an
    /// interrupt wakes us with more work.
    ///
    /// Returns `!` because, in our kernel, the keyboard task is intended to
    /// run for the lifetime of the system.
    pub fn run(&mut self) -> ! {
        loop {
            self.run_ready_tasks();
            self.sleep_if_idle();
        }
    }

    /// Test-only entry point that performs a single ready-tasks drain
    /// without halting the CPU. Allows integration tests to advance the
    /// executor a controlled amount and inspect state in between.
    #[doc(hidden)]
    pub fn run_ready_tasks_for_test(&mut self) {
        self.run_ready_tasks();
    }

    /// If no tasks are ready, halt the CPU until the next interrupt.
    ///
    /// Interrupts are disabled around the `is_empty` check to avoid the
    /// classic race where an interrupt fires *between* the check and the
    /// `hlt` instruction, leaving us asleep with work pending.
    fn sleep_if_idle(&self) {
        use x86_64::instructions::interrupts::{self, enable_and_hlt};

        interrupts::disable();
        if self.task_queue.is_empty() {
            // `enable_and_hlt` is atomic with respect to interrupts: any
            // interrupt that became pending after `disable()` will be
            // delivered immediately *after* the `hlt` returns.
            enable_and_hlt();
        } else {
            interrupts::enable();
        }
    }
}

/// Per-task waker. Holds a reference to the shared ready queue and the id of
/// the task it should wake.
struct TaskWaker {
    task_id: TaskId,
    task_queue: Arc<ArrayQueue<TaskId>>,
}

impl TaskWaker {
    /// Wraps a fresh `TaskWaker` in an `Arc` and converts it into a
    /// [`Waker`] via the standard `Wake` -> `Waker` conversion.
    ///
    /// The factory pattern (returning `Waker` rather than `Self`) is what
    /// the executor needs at the call site, so the clippy lint about `new`
    /// returning non-`Self` is silenced here.
    #[allow(clippy::new_ret_no_self)]
    fn new(task_id: TaskId, task_queue: Arc<ArrayQueue<TaskId>>) -> Waker {
        Waker::from(Arc::new(TaskWaker {
            task_id,
            task_queue,
        }))
    }

    /// Marks this waker's task as ready by pushing its id onto the queue.
    fn wake_task(&self) {
        self.task_queue.push(self.task_id).expect("task_queue full");
    }
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_task();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_task();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test_case]
    fn test_new_executor_has_no_tasks() {
        let exec = CoOpExecuter::new();
        assert_eq!(exec.task_count(), 0);
    }

    #[test_case]
    fn test_spawn_inserts_task_and_enqueues_id() {
        let mut exec = CoOpExecuter::new();
        let task = Task::new(async {});
        let id = task.id();
        exec.spawn(task);
        assert_eq!(exec.task_count(), 1);
        // Spawning enqueues the id so the task is immediately runnable.
        assert_eq!(exec.task_queue.pop(), Some(id));
    }

    #[test_case]
    fn test_run_ready_tasks_completes_simple_futures() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        COUNT.store(0, Ordering::SeqCst);

        let mut exec = CoOpExecuter::new();
        for _ in 0..3 {
            exec.spawn(Task::new(async {
                COUNT.fetch_add(1, Ordering::SeqCst);
            }));
        }
        exec.run_ready_tasks();
        assert_eq!(COUNT.load(Ordering::SeqCst), 3);
        assert_eq!(exec.task_count(), 0, "completed tasks must be removed");
    }

    #[test_case]
    fn test_task_waker_pushes_id_to_queue() {
        let queue = Arc::new(ArrayQueue::new(4));
        let id = TaskId::new();
        let waker = TaskWaker::new(id, queue.clone());
        waker.wake_by_ref();
        waker.wake();
        assert_eq!(queue.pop(), Some(id));
        assert_eq!(queue.pop(), Some(id));
        assert_eq!(queue.pop(), None);
    }

    #[test_case]
    fn test_run_ready_tasks_ignores_unknown_ids() {
        // Pushing an id with no corresponding task must not crash; it just
        // gets skipped on the next poll.
        let mut exec = CoOpExecuter::new();
        exec.task_queue
            .push(TaskId::new())
            .expect("queue should accept push");
        exec.run_ready_tasks();
        assert_eq!(exec.task_count(), 0);
    }
}
