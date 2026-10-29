//! Service implementations.
//!
//! Each service owns one kind of state. The structs here are deliberately
//! small: their job is to be the only place that reaches into the
//! corresponding subsystem's storage.

use alloc::collections::BTreeMap;
use spin::Mutex;
use x86_64::VirtAddr;

use super::abi::{MemoryStats, SchedulerStats};
use crate::process::{Process, ProcessId, ProcessSnapshot};
use crate::scheduler::SCHEDULER;
use crate::thread::{self, ThreadId};

/// Owns the process registry.
pub struct ProcessService {
    processes: Mutex<BTreeMap<ProcessId, Process>>,
}

impl ProcessService {
    pub const fn new() -> Self {
        Self {
            processes: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create and register a process from its entry coordinates.
    pub fn create(&self, entry: VirtAddr, stack_top: VirtAddr) -> ProcessId {
        self.register(Process::new(entry, stack_top))
    }

    /// Register an already-constructed process.
    pub fn register(&self, process: Process) -> ProcessId {
        let pid = process.pid;
        self.processes.lock().insert(pid, process);
        SCHEDULER.note_process_spawned();
        pid
    }

    /// Remove `pid` from the registry. Returns `false` if it was unknown.
    pub fn exit(&self, pid: ProcessId, _code: i32) -> bool {
        let removed = self.processes.lock().remove(&pid);
        match removed {
            Some(_) => {
                SCHEDULER.note_process_exited();
                true
            }
            None => false,
        }
    }

    pub fn snapshot(&self, pid: ProcessId) -> Option<ProcessSnapshot> {
        self.processes
            .lock()
            .get(&pid)
            .map(|process| ProcessSnapshot {
                pid: process.pid,
                user_entry: process.user_entry,
                user_stack_top: process.user_stack_top,
                state: process.state,
            })
    }

    pub fn count(&self) -> usize {
        self.processes.lock().len()
    }
}

/// Kernel-thread control plane.
///
/// The ready queue itself lives in [`crate::thread`]; this service is the
/// only caller allowed to spawn through it.
pub struct ThreadService;

impl ThreadService {
    pub const fn new() -> Self {
        Self
    }

    pub fn spawn(&self, entry: extern "C" fn() -> !, process: ProcessId) -> ThreadId {
        thread::spawn(entry, process)
    }

    pub fn yield_now(&self) {
        thread::yield_now();
    }

    pub fn exit_current(&self) -> ! {
        thread::exit_thread()
    }
}

/// Lock-free runtime counters.
pub struct SchedulerService;

impl SchedulerService {
    pub const fn new() -> Self {
        Self
    }

    pub fn tick(&self) {
        SCHEDULER.tick();
    }

    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            ticks: SCHEDULER.ticks(),
            processes_alive: SCHEDULER.processes_alive(),
            processes_spawned: SCHEDULER.processes_spawned(),
            threads_alive: SCHEDULER.threads_alive(),
            threads_spawned: SCHEDULER.threads_spawned(),
            tasks_alive: SCHEDULER.tasks_alive(),
            tasks_spawned: SCHEDULER.tasks_spawned(),
            tasks_completed: SCHEDULER.tasks_completed(),
        }
    }
}

/// Heap accounting.
pub struct MemoryService;

impl MemoryService {
    pub const fn new() -> Self {
        Self
    }

    pub fn stats(&self) -> MemoryStats {
        let stats = crate::allocator::stats();
        MemoryStats {
            heap_size: stats.heap_size,
            heap_free: stats.free_bytes,
        }
    }
}

/// Monotonic time.
pub struct TimerService;

impl TimerService {
    pub const fn new() -> Self {
        Self
    }

    pub fn ticks(&self) -> u64 {
        SCHEDULER.ticks()
    }
}

/// Keyboard input.
///
/// The IRQ handler pushes into the raw scancode queue; consumers use the
/// decoded byte queue. Neither path allocates.
pub struct InputService;

impl InputService {
    pub const fn new() -> Self {
        Self
    }

    pub fn on_scancode(&self, byte: u8) {
        crate::task::keyboard::add_scancode(byte);
    }

    pub fn read_byte(&self) -> Option<u8> {
        crate::task::keyboard::read_byte()
    }
}

/// Console output.
pub struct ConsoleService;

impl ConsoleService {
    pub const fn new() -> Self {
        Self
    }

    pub fn write(&self, bytes: &[u8]) -> usize {
        for &byte in bytes {
            crate::print!("{}", byte as char);
        }
        bytes.len()
    }
}
