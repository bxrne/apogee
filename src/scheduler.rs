//! Minimal kernel scheduler state.
//!
//! This module currently tracks timer ticks and a simple ready queue of
//! [`TaskId`]s. It is intentionally small: allocation is handled by the global
//! allocator, while scheduling policy/context switching can be added later.

use crate::allocator::Locked;
use crate::task::{Task, TaskId};
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

/// Global scheduler instance.
pub static SCHEDULER: Locked<Scheduler> = Locked::new(Scheduler::new());

const TICK_LOG_INTERVAL: u64 = 200;

pub struct Scheduler {
    ready_queue: VecDeque<TaskId>,
    tick_count: AtomicU64,
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            ready_queue: VecDeque::new(),
            tick_count: AtomicU64::new(0),
        }
    }

    /// Enqueue a task by id (derived from the task).
    pub fn add_task(&mut self, task: &Task) {
        self.ready_queue.push_back(task.id());
        crate::kdebugln!(
            "scheduler: enqueued task {} (ready={})",
            task.id().as_u64(),
            self.ready_queue.len()
        );
    }

    /// Enqueue a known task id.
    pub fn push_task_id(&mut self, task_id: TaskId) {
        self.ready_queue.push_back(task_id);
        crate::kdebugln!(
            "scheduler: enqueued task {} (ready={})",
            task_id.as_u64(),
            self.ready_queue.len()
        );
    }

    /// Pop the next runnable task id (round-robin FIFO style).
    pub fn pop_next_task(&mut self) -> Option<TaskId> {
        self.ready_queue.pop_front()
    }

    /// Return a task id to the end of the queue.
    pub fn requeue_task(&mut self, task_id: TaskId) {
        self.ready_queue.push_back(task_id);
    }

    pub fn queue_len(&self) -> usize {
        self.ready_queue.len()
    }

    /// Called from the timer interrupt.
    pub fn tick(&self) {
        let ticks = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks % TICK_LOG_INTERVAL == 0 {
            crate::kdebugln!(
                "scheduler tick {} (ready queue: {})",
                ticks,
                self.ready_queue.len()
            );
        }
    }

    pub fn ticks(&self) -> u64 {
        self.tick_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_new_scheduler_is_empty() {
        let scheduler = Scheduler::new();
        assert_eq!(scheduler.queue_len(), 0);
        assert_eq!(scheduler.ticks(), 0);
    }

    fn make_task_id() -> TaskId {
        Task::new(async {}).id()
    }

    #[test_case]
    fn test_push_and_pop_task_ids_fifo() {
        let mut scheduler = Scheduler::new();
        scheduler.push_task_id(make_task_id());
        scheduler.push_task_id(make_task_id());

        let first = scheduler.pop_next_task().expect("first task missing");
        let second = scheduler.pop_next_task().expect("second task missing");

        assert!(first.as_u64() < second.as_u64());
        assert_eq!(scheduler.pop_next_task(), None);
    }

    #[test_case]
    fn test_add_task_enqueues_task_id() {
        let mut scheduler = Scheduler::new();
        let task = Task::new(async {});
        let id = task.id();

        scheduler.add_task(&task);
        assert_eq!(scheduler.queue_len(), 1);
        assert_eq!(scheduler.pop_next_task(), Some(id));
    }

    #[test_case]
    fn test_requeue_puts_task_back() {
        let mut scheduler = Scheduler::new();
        let id = make_task_id();

        scheduler.push_task_id(id);
        let popped = scheduler.pop_next_task().expect("queue should not be empty");
        scheduler.requeue_task(popped);

        assert_eq!(scheduler.pop_next_task(), Some(id));
        assert_eq!(scheduler.queue_len(), 0);
    }

    #[test_case]
    fn test_tick_increments_counter() {
        let scheduler = Scheduler::new();
        scheduler.tick();
        scheduler.tick();
        assert_eq!(scheduler.ticks(), 2);
    }
}
