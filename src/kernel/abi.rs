//! The kernel's internal call ABI.
//!
//! A [`Request`] is a value describing one operation on one service. The
//! same value is produced by kernel callers and by the ring-3 syscall
//! decoder, so both paths share the same validation and dispatch.

use crate::process::{ProcessId, ProcessSnapshot};
use crate::thread::ThreadId;
use x86_64::VirtAddr;

/// Who is making a request.
///
/// The router uses this for authorization, not for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// Kernel code acting on its own behalf.
    Kernel,
    /// A ring-3 process, identified by its host kernel thread's tag.
    User(ProcessId),
    /// An interrupt handler. Handlers normally call service methods
    /// directly; this variant exists so every entry point is represented.
    Irq,
}

/// The service a request belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceId {
    Process,
    Thread,
    Scheduler,
    Memory,
    Timer,
    Input,
    Console,
}

/// Stable, negative errno values returned through [`Response::Err`] and
/// the ring-3 `rax` register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SysError {
    BadHandle = 1,
    Denied = 2,
    BadPointer = 3,
    WouldBlock = 4,
    NoResources = 5,
    Unsupported = 6,
}

impl SysError {
    /// Two's-complement representation expected in `rax`.
    pub const fn errno(self) -> i64 {
        -(self as i64)
    }
}

/// Point-in-time scheduler counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    pub ticks: u64,
    pub processes_alive: usize,
    pub processes_spawned: u64,
    pub threads_alive: usize,
    pub threads_spawned: u64,
    pub tasks_alive: usize,
    pub tasks_spawned: u64,
    pub tasks_completed: u64,
}

/// Point-in-time heap counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStats {
    pub heap_size: usize,
    pub heap_free: usize,
}

/// One operation on one service.
///
/// The control-plane requests carry values; the only borrowed payload is
/// the console buffer. A ring-3 buffer is validated before it is turned
/// into a slice, so the kernel never dereferences an untrusted pointer.
pub enum Request<'a> {
    ProcessCreate {
        entry: VirtAddr,
        stack_top: VirtAddr,
    },
    ProcessExit {
        pid: ProcessId,
        code: i32,
    },
    ProcessSnapshot {
        pid: ProcessId,
    },
    ProcessCount,
    ThreadSpawn {
        entry: extern "C" fn() -> !,
        process: ProcessId,
    },
    ThreadYield,
    ThreadExit,
    SchedulerStats,
    MemoryStats,
    TimerTicks,
    InputReadByte,
    ConsoleWrite(&'a [u8]),
}

impl Request<'_> {
    pub const fn service(&self) -> ServiceId {
        match self {
            Self::ProcessCreate { .. }
            | Self::ProcessExit { .. }
            | Self::ProcessSnapshot { .. }
            | Self::ProcessCount => ServiceId::Process,
            Self::ThreadSpawn { .. } | Self::ThreadYield | Self::ThreadExit => ServiceId::Thread,
            Self::SchedulerStats => ServiceId::Scheduler,
            Self::MemoryStats => ServiceId::Memory,
            Self::TimerTicks => ServiceId::Timer,
            Self::InputReadByte => ServiceId::Input,
            Self::ConsoleWrite(_) => ServiceId::Console,
        }
    }
}

/// The result of a [`Request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Ok,
    Written(usize),
    Byte(Option<u8>),
    Process(ProcessId),
    Snapshot(Option<ProcessSnapshot>),
    Thread(ThreadId),
    Count(usize),
    Stats(SchedulerStats),
    Memory(MemoryStats),
    Ticks(u64),
    Err(SysError),
}
