//! Process abstractions (stub for future userland support).
//!
//! Today the kernel runs only kernel-mode threads sharing one address
//! space. This module reserves the types we'll need once we add real
//! processes: a process identity, a kernel-stack range used while a
//! process is trapped into the kernel, and a saved user-mode register
//! frame.
//!
//! Nothing in here drives the running kernel yet — it exists so the
//! rest of the codebase can refer to "the kernel process" via
//! [`ProcessId::KERNEL`] (every spawned [`crate::thread::KernelThread`]
//! is tagged with a [`ProcessId`]) and so the eventual syscall /
//! context-switch code has a stable place to live.

use core::sync::atomic::{AtomicU64, Ordering};

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

/// Kernel-mode stack range used while a user process is trapped into
/// the kernel. Kernel-only threads instead carry their own owned stacks
/// via [`crate::thread::KernelThread`].
#[derive(Debug, Clone, Copy)]
pub struct KernelStack {
    pub start: usize,
    pub end: usize,
}

/// Saved user-mode register state. Populated by the syscall / interrupt
/// entry path once we have userland; unused today.
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

/// A process: an address space + at least one thread.
///
/// Stub — no page table, no thread list. Once we have userland, this
/// will own the per-process page table root and a list of `ThreadId`s.
pub struct Process {
    pub pid: ProcessId,
}

impl Process {
    /// Build the canonical kernel-process descriptor.
    pub const fn kernel() -> Self {
        Process {
            pid: ProcessId::KERNEL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_kernel_pid_is_zero() {
        assert_eq!(ProcessId::KERNEL.as_u64(), 0);
        assert_eq!(Process::kernel().pid, ProcessId::KERNEL);
    }

    #[test_case]
    fn test_new_pids_are_unique_and_skip_zero() {
        let a = ProcessId::new();
        let b = ProcessId::new();
        assert_ne!(a, b);
        assert!(a.as_u64() >= 1);
        assert!(b.as_u64() > a.as_u64());
        assert_ne!(a, ProcessId::KERNEL);
    }

    #[test_case]
    fn test_user_context_default_is_zeroed() {
        let ctx = UserContext::default();
        assert_eq!(ctx.rip, 0);
        assert_eq!(ctx.rsp, 0);
        assert_eq!(ctx.r15, 0);
    }
}
