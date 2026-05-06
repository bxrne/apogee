//! Kernel-wide runtime observability and tick source.
//!
//! This module owns the *global* scheduler state: a monotonic timer-tick
//! counter and live counts of kernel threads and async tasks. The actual
//! ready queues live with the things that own them — kernel threads in
//! [`crate::thread`] and async tasks in [`crate::task::executor`] — and
//! call back into here so that interrupts and tests have a single place
//! to look at what the kernel is doing.
//!
//! Everything here is lock-free: every field is an atomic. The timer
//! interrupt calls [`Scheduler::tick`] without ever acquiring a lock,
//! which closes the entire deadlock class where a non-IRQ caller holds
//! a lock that an IRQ then tries to take.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Global scheduler instance.
pub static SCHEDULER: Scheduler = Scheduler::new();

/// How often (in ticks) to log a heartbeat with current liveness counts.
const TICK_LOG_INTERVAL: u64 = 200;

pub struct Scheduler {
    tick_count: AtomicU64,
    threads_alive: AtomicUsize,
    threads_spawned: AtomicU64,
    tasks_alive: AtomicUsize,
    tasks_spawned: AtomicU64,
    tasks_completed: AtomicU64,
    processes_alive: AtomicUsize,
    processes_spawned: AtomicU64,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            tick_count: AtomicU64::new(0),
            threads_alive: AtomicUsize::new(0),
            threads_spawned: AtomicU64::new(0),
            tasks_alive: AtomicUsize::new(0),
            tasks_spawned: AtomicU64::new(0),
            tasks_completed: AtomicU64::new(0),
            processes_alive: AtomicUsize::new(0),
            processes_spawned: AtomicU64::new(0),
        }
    }

    /// Called from the timer interrupt. Lock-free; safe from any
    /// context (including inside an IRQ handler).
    pub fn tick(&self) {
        let ticks = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks % TICK_LOG_INTERVAL == 0 {
            crate::kdebugln!(
                "scheduler tick {} (threads={} tasks={})",
                ticks,
                self.threads_alive.load(Ordering::Relaxed),
                self.tasks_alive.load(Ordering::Relaxed),
            );
        }
    }

    pub fn ticks(&self) -> u64 {
        self.tick_count.load(Ordering::Relaxed)
    }

    // ---- thread observability hooks ---------------------------------

    /// Record that a kernel thread was just spawned.
    pub fn note_thread_spawned(&self) {
        self.threads_alive.fetch_add(1, Ordering::Relaxed);
        self.threads_spawned.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a kernel thread has terminated.
    pub fn note_thread_exited(&self) {
        self.threads_alive.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn threads_alive(&self) -> usize {
        self.threads_alive.load(Ordering::Relaxed)
    }
    pub fn threads_spawned(&self) -> u64 {
        self.threads_spawned.load(Ordering::Relaxed)
    }

    // ---- task observability hooks -----------------------------------

    /// Record that an async task was just spawned.
    pub fn note_task_spawned(&self) {
        self.tasks_alive.fetch_add(1, Ordering::Relaxed);
        self.tasks_spawned.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that an async task ran to `Poll::Ready` and was reaped.
    pub fn note_task_completed(&self) {
        self.tasks_alive.fetch_sub(1, Ordering::Relaxed);
        self.tasks_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn tasks_alive(&self) -> usize {
        self.tasks_alive.load(Ordering::Relaxed)
    }
    pub fn tasks_spawned(&self) -> u64 {
        self.tasks_spawned.load(Ordering::Relaxed)
    }
    pub fn tasks_completed(&self) -> u64 {
        self.tasks_completed.load(Ordering::Relaxed)
    }

    // ---- process observability hooks --------------------------------

    pub fn note_process_spawned(&self) {
        self.processes_alive.fetch_add(1, Ordering::Relaxed);
        self.processes_spawned.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_process_exited(&self) {
        self.processes_alive.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn processes_alive(&self) -> usize {
        self.processes_alive.load(Ordering::Relaxed)
    }
    pub fn processes_spawned(&self) -> u64 {
        self.processes_spawned.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_new_scheduler_is_zeroed() {
        let s = Scheduler::new();
        assert_eq!(s.ticks(), 0);
        assert_eq!(s.threads_alive(), 0);
        assert_eq!(s.threads_spawned(), 0);
        assert_eq!(s.tasks_alive(), 0);
        assert_eq!(s.tasks_spawned(), 0);
        assert_eq!(s.tasks_completed(), 0);
    }

    #[test_case]
    fn test_tick_increments_counter() {
        let s = Scheduler::new();
        s.tick();
        s.tick();
        s.tick();
        assert_eq!(s.ticks(), 3);
    }

    #[test_case]
    fn test_thread_spawn_and_exit_balance() {
        let s = Scheduler::new();
        s.note_thread_spawned();
        s.note_thread_spawned();
        assert_eq!(s.threads_alive(), 2);
        assert_eq!(s.threads_spawned(), 2);
        s.note_thread_exited();
        assert_eq!(s.threads_alive(), 1);
        // Cumulative spawn counter must not regress on exit.
        assert_eq!(s.threads_spawned(), 2);
    }

    #[test_case]
    fn test_process_spawn_and_exit_balance() {
        let s = Scheduler::new();
        s.note_process_spawned();
        s.note_process_spawned();
        assert_eq!(s.processes_alive(), 2);
        assert_eq!(s.processes_spawned(), 2);
        s.note_process_exited();
        assert_eq!(s.processes_alive(), 1);
        assert_eq!(s.processes_spawned(), 2);
    }

    #[test_case]
    fn test_task_spawn_and_complete_balance() {
        let s = Scheduler::new();
        s.note_task_spawned();
        s.note_task_spawned();
        assert_eq!(s.tasks_alive(), 2);
        assert_eq!(s.tasks_spawned(), 2);
        s.note_task_completed();
        assert_eq!(s.tasks_alive(), 1);
        assert_eq!(s.tasks_completed(), 1);
        assert_eq!(s.tasks_spawned(), 2);
    }
}
