//! Processes — the unit of isolation between userland tasks.
//!
//! A [`Process`] today owns:
//! - a unique [`ProcessId`]
//! - an `Option<PhysAddr>` page-table root (today always `None` — every
//!   process shares the kernel address space; per-process page tables
//!   slot in here later)
//! - the user-mode entry point and the top of the user stack
//! - a lifecycle [`ProcessState`]
//!
//! Processes are tracked in [`PROCESSES`], a `Mutex<BTreeMap>` keyed by
//! [`ProcessId`]. [`crate::userland::create_demo_process`] populates the
//! address space and registers a new process, and
//! [`crate::userland::spawn_user_thread`] attaches a host
//! [`crate::thread::KernelThread`] that drops the process into ring 3.
//!
//! [`KernelStack`] / [`UserContext`] are kept as data-layout placeholders
//! for the future per-process kernel stack and saved register frame.

use crate::scheduler::SCHEDULER;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};

/// Process-wide unique identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(u64);

impl ProcessId {
    /// The implicit "process" containing every kernel thread.
    pub const KERNEL: ProcessId = ProcessId(0);

    /// Allocate a fresh, never-before-used process id.
    pub fn new() -> Self {
        // Skip 0 so it stays reserved for `KERNEL`.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        ProcessId(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Reconstruct a [`ProcessId`] from its raw `u64` representation.
    /// Used by the syscall handoff paths.
    pub(crate) fn from_raw(v: u64) -> Self {
        ProcessId(v)
    }
}

/// Lifecycle state for a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Running,
    Exited(i32),
}

/// Kernel-mode stack range used while a user process is trapped into
/// the kernel. Today the privilege-stack-table entry in the GDT module
/// owns this; the field is kept as a future home for per-process
/// kernel stacks.
#[derive(Debug, Clone, Copy)]
pub struct KernelStack {
    pub start: usize,
    pub end: usize,
}

/// Saved user-mode register state. Populated by the syscall / interrupt
/// entry path; today the [`crate::userland::SyscallFrame`] is what
/// actually carries register state across a syscall, but this struct is
/// kept as the future "process register save" type.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct UserContext {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// A live user process. Owns its identity and entry-point coordinates.
pub struct Process {
    pub pid: ProcessId,
    /// Future per-process page-table root. `None` means "use the
    /// kernel address space" (today, always).
    pub page_table_root: Option<PhysAddr>,
    /// User-mode entry point.
    pub user_entry: VirtAddr,
    /// User-mode initial stack pointer (top of the allocated stack).
    pub user_stack_top: VirtAddr,
    pub state: ProcessState,
}

impl Process {
    pub fn new(user_entry: VirtAddr, user_stack_top: VirtAddr) -> Self {
        Process {
            pid: ProcessId::new(),
            page_table_root: None,
            user_entry,
            user_stack_top,
            state: ProcessState::Ready,
        }
    }
}

/// Global process registry.
pub static PROCESSES: Mutex<BTreeMap<ProcessId, Process>> = Mutex::new(BTreeMap::new());

/// Insert `process` into the registry and bump the scheduler's
/// "processes spawned / alive" counters. Returns the new process id.
pub fn register(process: Process) -> ProcessId {
    let pid = process.pid;
    PROCESSES.lock().insert(pid, process);
    SCHEDULER.note_process_spawned();
    pid
}

/// Mark `pid` as exited with `code`, drop it from the registry, and
/// notify the scheduler.
pub fn exit(pid: ProcessId, code: i32) {
    let removed = PROCESSES.lock().remove(&pid);
    if let Some(mut p) = removed {
        // The descriptor is dropped at the end of this scope; the
        // exit code is recorded for any future `wait`-style API.
        p.state = ProcessState::Exited(code);
        let _ = p.state;
        SCHEDULER.note_process_exited();
    }
}

/// Read-only snapshot of a process descriptor for callers that need
/// the entry point / stack without holding the registry lock.
#[derive(Debug, Clone, Copy)]
pub struct ProcessSnapshot {
    pub pid: ProcessId,
    pub user_entry: VirtAddr,
    pub user_stack_top: VirtAddr,
    pub state: ProcessState,
}

pub fn snapshot(pid: ProcessId) -> Option<ProcessSnapshot> {
    PROCESSES.lock().get(&pid).map(|p| ProcessSnapshot {
        pid: p.pid,
        user_entry: p.user_entry,
        user_stack_top: p.user_stack_top,
        state: p.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_kernel_pid_is_zero() {
        assert_eq!(ProcessId::KERNEL.as_u64(), 0);
    }

    #[test_case]
    fn test_new_pids_unique_and_skip_zero() {
        let a = ProcessId::new();
        let b = ProcessId::new();
        assert_ne!(a, b);
        assert!(a.as_u64() >= 1);
        assert!(b.as_u64() > a.as_u64());
        assert_ne!(a, ProcessId::KERNEL);
    }

    #[test_case]
    fn test_process_register_and_exit_round_trip() {
        let before_alive = SCHEDULER.processes_alive();
        let before_spawned = SCHEDULER.processes_spawned();

        let p = Process::new(
            VirtAddr::new(0x4000_0000_0000),
            VirtAddr::new(0x4000_0000_3000),
        );
        let pid = p.pid;
        let registered = register(p);
        assert_eq!(registered, pid);
        assert_eq!(SCHEDULER.processes_alive(), before_alive + 1);
        assert_eq!(SCHEDULER.processes_spawned(), before_spawned + 1);

        let snap = snapshot(pid).expect("registered process should be visible");
        assert_eq!(snap.pid, pid);
        assert_eq!(snap.user_entry.as_u64(), 0x4000_0000_0000);
        assert_eq!(snap.user_stack_top.as_u64(), 0x4000_0000_3000);
        assert_eq!(snap.state, ProcessState::Ready);

        exit(pid, 0);
        assert_eq!(SCHEDULER.processes_alive(), before_alive);
        // Cumulative spawn counter is monotonic.
        assert_eq!(SCHEDULER.processes_spawned(), before_spawned + 1);
        assert!(snapshot(pid).is_none());
    }
}
