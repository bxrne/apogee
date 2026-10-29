//! Process descriptors and lifecycle states.
//!
//! The registry itself lives in [`crate::kernel::KERNEL`]'s process
//! service. This module defines the data model and the small compatibility
//! helpers used by tests; new code should go through `KERNEL.call`.
//!
//! A [`Process`] owns its identity, the user entry coordinates, and a
//! lifecycle state. The page-table root is `None` today: every process
//! shares the kernel address space until per-process page tables land.

use core::sync::atomic::{AtomicU64, Ordering};
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
/// entry path; today the [`crate::syscall::SyscallFrame`] is what
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

/// Register a process with the kernel. Compatibility wrapper around the
/// process service; new code should use [`crate::kernel::KERNEL`].
pub fn register(process: Process) -> ProcessId {
    crate::kernel::KERNEL.register_process(process)
}

/// Remove `pid` from the registry and notify the scheduler.
pub fn exit(pid: ProcessId, code: i32) {
    let _ = crate::kernel::KERNEL.exit_process(pid, code);
}

/// Read-only snapshot of a process descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub pid: ProcessId,
    pub user_entry: VirtAddr,
    pub user_stack_top: VirtAddr,
    pub state: ProcessState,
}

pub fn snapshot(pid: ProcessId) -> Option<ProcessSnapshot> {
    crate::kernel::KERNEL.process_snapshot(pid)
}

/// Whether `pid` is currently registered.
pub fn is_registered(pid: ProcessId) -> bool {
    snapshot(pid).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::SCHEDULER;

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
