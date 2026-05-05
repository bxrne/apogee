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
    }

    /// Enqueue a known task id.
    pub fn push_task_id(&mut self, task_id: TaskId) {
        self.ready_queue.push_back(task_id);
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
        self.tick_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn ticks(&self) -> u64 {
        self.tick_count.load(Ordering::Relaxed)
    }
}
