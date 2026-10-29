//! The kernel object and its call boundary.
//!
//! The kernel is a composition root: it owns every service, and the only
//! way one service can reach another is through [`Kernel::call`]. Ring-3
//! syscalls decode registers into the same [`Request`] values, so there is
//! exactly one implementation of every operation and one place to enforce
//! caller identity.
//!
//! Interrupt handlers are the exception: they use the direct
//! [`Kernel::tick`] and [`Kernel::on_scancode`] entry points because they
//! must not allocate, block, or take a service lock that a non-IRQ caller
//! might already hold.

pub mod abi;

mod service;

pub use abi::{Caller, Request, Response, SysError};
use service::{
    ConsoleService, InputService, MemoryService, ProcessService, SchedulerService, ThreadService,
    TimerService,
};

use crate::process::{Process, ProcessId, ProcessSnapshot};
use crate::thread::ThreadId;

/// The one kernel root. There is deliberately no other global service
/// handle: a subsystem that needs another subsystem asks this object.
pub static KERNEL: Kernel = Kernel::new();

pub struct Kernel {
    process: ProcessService,
    thread: ThreadService,
    scheduler: SchedulerService,
    memory: MemoryService,
    timer: TimerService,
    input: InputService,
    console: ConsoleService,
}

impl Kernel {
    pub const fn new() -> Self {
        Self {
            process: ProcessService::new(),
            thread: ThreadService::new(),
            scheduler: SchedulerService::new(),
            memory: MemoryService::new(),
            timer: TimerService::new(),
            input: InputService::new(),
            console: ConsoleService::new(),
        }
    }

    /// Route one request. This is the only entry point used by kernel
    /// callers and by the ring-3 syscall adapter.
    pub fn call(&self, caller: Caller, request: Request<'_>) -> Response {
        match request {
            Request::ProcessCreate { entry, stack_top } => {
                if !matches!(caller, Caller::Kernel) {
                    return Response::Err(SysError::Denied);
                }
                Response::Process(self.process.create(entry, stack_top))
            }
            Request::ProcessExit { pid, code } => {
                if let Caller::User(caller_pid) = caller
                    && caller_pid != pid
                {
                    return Response::Err(SysError::Denied);
                }
                if self.process.exit(pid, code) {
                    Response::Ok
                } else {
                    Response::Err(SysError::BadHandle)
                }
            }
            Request::ProcessSnapshot { pid } => {
                if let Caller::User(caller_pid) = caller
                    && caller_pid != pid
                {
                    return Response::Err(SysError::Denied);
                }
                Response::Snapshot(self.process.snapshot(pid))
            }
            Request::ProcessCount => {
                if !matches!(caller, Caller::Kernel) {
                    return Response::Err(SysError::Denied);
                }
                Response::Count(self.process.count())
            }
            Request::ThreadSpawn { entry, process } => {
                if !matches!(caller, Caller::Kernel) {
                    return Response::Err(SysError::Denied);
                }
                Response::Thread(self.thread.spawn(entry, process))
            }
            Request::ThreadYield => {
                self.thread.yield_now();
                Response::Ok
            }
            Request::ThreadExit => self.thread.exit_current(),
            Request::SchedulerStats => Response::Stats(self.scheduler.stats()),
            Request::MemoryStats => Response::Memory(self.memory.stats()),
            Request::TimerTicks => Response::Ticks(self.timer.ticks()),
            Request::InputReadByte => Response::Byte(self.input.read_byte()),
            Request::ConsoleWrite(bytes) => Response::Written(self.console.write(bytes)),
        }
    }

    /// Timer IRQ entry point. Lock-free.
    pub fn tick(&self) {
        self.scheduler.tick();
    }

    /// Keyboard IRQ entry point. Allocation-free and non-blocking.
    pub fn on_scancode(&self, byte: u8) {
        self.input.on_scancode(byte);
    }

    /// Kernel-only process registration, used by the compatibility
    /// helpers in [`crate::process`] and by tests.
    pub fn register_process(&self, process: Process) -> ProcessId {
        self.process.register(process)
    }

    /// Kernel-only process termination.
    pub fn exit_process(&self, pid: ProcessId, code: i32) -> bool {
        self.process.exit(pid, code)
    }

    pub fn process_snapshot(&self, pid: ProcessId) -> Option<ProcessSnapshot> {
        self.process.snapshot(pid)
    }

    pub fn process_count(&self) -> usize {
        self.process.count()
    }

    /// Spawn a kernel thread from kernel code.
    pub fn spawn_thread(&self, entry: extern "C" fn() -> !, process: ProcessId) -> ThreadId {
        self.thread.spawn(entry, process)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::abi::{ServiceId, SysError};
    use x86_64::VirtAddr;

    fn create(entry: u64, stack_top: u64) -> ProcessId {
        match KERNEL.call(
            Caller::Kernel,
            Request::ProcessCreate {
                entry: VirtAddr::new(entry),
                stack_top: VirtAddr::new(stack_top),
            },
        ) {
            Response::Process(pid) => pid,
            other => panic!("unexpected create response: {other:?}"),
        }
    }

    #[test_case]
    fn test_process_lifecycle_through_call() {
        let before = KERNEL.process_count();
        let pid = create(0x4000_0000_0000, 0x4000_0000_3000);
        assert_eq!(KERNEL.process_count(), before + 1);
        assert!(KERNEL.process_snapshot(pid).is_some());

        let response = KERNEL.call(Caller::User(pid), Request::ProcessExit { pid, code: 7 });
        assert_eq!(response, Response::Ok);
        assert_eq!(KERNEL.process_count(), before);
        assert!(KERNEL.process_snapshot(pid).is_none());
    }

    #[test_case]
    fn test_user_cannot_exit_another_process() {
        let first = create(0x4000_0000_0000, 0x4000_0000_3000);
        let second = create(0x4000_0000_0000, 0x4000_0000_3000);

        let response = KERNEL.call(
            Caller::User(first),
            Request::ProcessExit {
                pid: second,
                code: 0,
            },
        );
        assert_eq!(response, Response::Err(SysError::Denied));
        assert!(KERNEL.process_snapshot(second).is_some());

        KERNEL.exit_process(first, 0);
        KERNEL.exit_process(second, 0);
    }

    #[test_case]
    fn test_user_cannot_create_processes() {
        let response = KERNEL.call(
            Caller::User(ProcessId::KERNEL),
            Request::ProcessCreate {
                entry: VirtAddr::new(0x4000_0000_0000),
                stack_top: VirtAddr::new(0x4000_0000_3000),
            },
        );
        assert_eq!(response, Response::Err(SysError::Denied));
    }

    #[test_case]
    fn test_memory_and_timer_requests() {
        let Response::Memory(memory) = KERNEL.call(Caller::Kernel, Request::MemoryStats) else {
            panic!("memory stats request failed");
        };
        assert_eq!(memory.heap_size, crate::allocator::HEAP_SIZE);
        assert!(memory.heap_free <= memory.heap_size);

        let Response::Ticks(ticks) = KERNEL.call(Caller::Kernel, Request::TimerTicks) else {
            panic!("timer request failed");
        };
        let Response::Ticks(again) = KERNEL.call(Caller::Kernel, Request::TimerTicks) else {
            panic!("timer request failed");
        };
        assert!(again >= ticks);
    }

    #[test_case]
    fn test_request_service_mapping() {
        assert_eq!(Request::ProcessCount.service(), ServiceId::Process);
        assert_eq!(Request::ThreadYield.service(), ServiceId::Thread);
        assert_eq!(Request::SchedulerStats.service(), ServiceId::Scheduler);
        assert_eq!(Request::MemoryStats.service(), ServiceId::Memory);
        assert_eq!(Request::TimerTicks.service(), ServiceId::Timer);
        assert_eq!(Request::InputReadByte.service(), ServiceId::Input);
        assert_eq!(Request::ConsoleWrite(b"x").service(), ServiceId::Console);
    }
}
