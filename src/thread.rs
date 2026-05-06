//! Kernel-thread context switching.
//!
//! Adds true context switching on top of the existing cooperative async
//! executor. Each [`KernelThread`] owns its own stack and a [`Context`]
//! holding the saved stack pointer; on a switch, the callee-saved registers
//! (rbx, rbp, r12-r15) are pushed onto the outgoing thread's stack, the
//! stack pointer is swapped, and the incoming thread's registers are
//! popped. Control then `ret`s into wherever the new thread was last
//! running.
//!
//! Switching is **cooperative**: a thread calls [`yield_now`] to give up
//! the CPU. The bootstrap kernel control flow (i.e. whoever first calls
//! into the scheduler) participates as an implicit "thread 0" so that
//! yields round-robin between it and any spawned threads.

use crate::allocator::Locked;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Per-thread stack size. 16 KiB is plenty for the trivial demo workloads
/// driven by this module and keeps heap pressure low on a small kernel
/// heap.
pub const STACK_SIZE: usize = 16 * 1024;

/// Saved CPU state for a kernel thread.
///
/// Only the stack pointer needs to live here; the callee-saved registers
/// are pushed onto the outgoing thread's own stack inside
/// [`switch_context`] and popped from the incoming thread's stack on the
/// other side of the swap.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Context {
    rsp: u64,
}

/// Process-wide unique identifier for a kernel thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u64);

/// Reserved id for the bootstrap (initial) kernel control flow.
pub const BOOTSTRAP_ID: ThreadId = ThreadId(0);

impl ThreadId {
    fn new() -> Self {
        // Skip 0 so it stays reserved for the bootstrap thread.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        ThreadId(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Finished,
}

/// A kernel thread: an entry function plus an owned stack and saved
/// register state.
pub struct KernelThread {
    id: ThreadId,
    state: ThreadState,
    context: Context,
    /// Backing storage for the thread stack. Held in a `Box` so that the
    /// allocation has a stable address for the lifetime of the thread —
    /// the saved `rsp` points into this buffer.
    _stack: Box<[u8; STACK_SIZE]>,
}

impl KernelThread {
    /// Build a new ready-to-run kernel thread that will start by jumping to
    /// `entry`. `entry` must be `extern "C" fn() -> !` so the compiler does
    /// not insert a function epilogue: when the thread "completes" it must
    /// call [`exit_thread`] instead of returning.
    pub fn new(entry: extern "C" fn() -> !) -> Self {
        let mut stack: Box<[u8; STACK_SIZE]> = Box::new([0; STACK_SIZE]);
        // Stacks grow downward; start at the high end of the buffer.
        let stack_top = unsafe { stack.as_mut_ptr().add(STACK_SIZE) } as u64;
        // SysV requires 16-byte alignment immediately *before* a CALL,
        // i.e. `rsp % 16 == 0` when entry is reached via `ret`.
        let mut sp = stack_top & !0xF;

        unsafe {
            // Push entry as the return address consumed by the final `ret`
            // inside `switch_context`.
            sp -= 8;
            (sp as *mut u64).write(entry as u64);
            // Reserve six zero slots for the callee-saved registers
            // popped in order: r15, r14, r13, r12, rbx, rbp.
            for _ in 0..6 {
                sp -= 8;
                (sp as *mut u64).write(0);
            }
        }

        KernelThread {
            id: ThreadId::new(),
            state: ThreadState::Ready,
            context: Context { rsp: sp },
            _stack: stack,
        }
    }

    pub fn id(&self) -> ThreadId {
        self.id
    }

    pub fn state(&self) -> ThreadState {
        self.state
    }
}

/// Save callee-saved registers and `rsp` into `*prev`, then load `rsp`
/// from `*next` and pop the incoming thread's callee-saved registers.
///
/// SysV x86-64 calling convention: `rdi = prev`, `rsi = next`.
///
/// # Safety
/// Both pointers must point to valid `Context` values, and `*next` must
/// have been initialized either by [`KernelThread::new`] or by a previous
/// call to this function.
#[unsafe(naked)]
unsafe extern "C" fn switch_context(_prev: *mut Context, _next: *const Context) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, [rsi]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// Global cooperative kernel-thread scheduler.
pub static THREADS: Locked<ThreadScheduler> = Locked::new(ThreadScheduler::new());

pub struct ThreadScheduler {
    threads: BTreeMap<ThreadId, Box<KernelThread>>,
    ready: VecDeque<ThreadId>,
    current: ThreadId,
    /// Saved register state for the bootstrap (initial) kernel thread,
    /// which has no associated [`KernelThread`] entry because it uses the
    /// kernel's startup stack.
    bootstrap_context: Context,
}

impl ThreadScheduler {
    pub const fn new() -> Self {
        ThreadScheduler {
            threads: BTreeMap::new(),
            ready: VecDeque::new(),
            current: BOOTSTRAP_ID,
            bootstrap_context: Context { rsp: 0 },
        }
    }

    /// Spawn a new kernel thread that will start by jumping to `entry`.
    /// Returns the new thread's id.
    pub fn spawn(&mut self, entry: extern "C" fn() -> !) -> ThreadId {
        let thread = Box::new(KernelThread::new(entry));
        let id = thread.id();
        self.threads.insert(id, thread);
        self.ready.push_back(id);
        id
    }

    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    pub fn ready_count(&self) -> usize {
        self.ready.len()
    }

    pub fn current(&self) -> ThreadId {
        self.current
    }
}

/// Voluntarily yield the CPU to the next ready thread.
///
/// If no other threads are ready, returns immediately without switching.
/// Otherwise the current thread (including the bootstrap thread) is
/// re-queued at the back of the ready list and the next ready thread is
/// switched in.
pub fn yield_now() {
    // Decide who we're switching to/from while holding the scheduler
    // lock, then drop the lock and snap raw context pointers across the
    // switch. The lock must be released before `switch_context` because
    // the incoming thread will need to re-acquire it.
    let (prev_ctx, next_ctx) = {
        let mut sched = THREADS.lock();

        let next_id = match sched.ready.pop_front() {
            Some(id) => id,
            None => return,
        };
        let prev_id = sched.current;
        sched.current = next_id;

        let prev_ctx_ptr: *mut Context = if prev_id == BOOTSTRAP_ID {
            sched.ready.push_back(BOOTSTRAP_ID);
            &mut sched.bootstrap_context as *mut Context
        } else {
            let t = sched
                .threads
                .get_mut(&prev_id)
                .expect("yield_now: current thread missing from map");
            t.state = ThreadState::Ready;
            let p = &mut t.context as *mut Context;
            sched.ready.push_back(prev_id);
            p
        };

        let next_ctx_ptr: *const Context = if next_id == BOOTSTRAP_ID {
            &sched.bootstrap_context as *const Context
        } else {
            let t = sched
                .threads
                .get_mut(&next_id)
                .expect("yield_now: next ready thread missing from map");
            t.state = ThreadState::Running;
            &t.context as *const Context
        };

        (prev_ctx_ptr, next_ctx_ptr)
    };

    // SAFETY: both pointers refer to `Context` values owned by the global
    // scheduler. Boxed `KernelThread`s have stable addresses for as long
    // as they remain in `threads`, and `bootstrap_context` is a static
    // field of the same `ThreadScheduler`. The lock has been released so
    // the incoming thread can re-acquire it.
    unsafe { switch_context(prev_ctx, next_ctx) };
}

/// Mark the current thread as finished, drop its resources, and switch
/// to the next ready thread. Never returns.
///
/// A thread function (`extern "C" fn() -> !`) must call this when its
/// work is done — returning would jump to a garbage address since the
/// thread's initial stack frame has no return slot beyond `entry`.
pub fn exit_thread() -> ! {
    // Pick the next thread to run while holding the lock; remove the
    // current thread from the map *before* releasing the lock so its
    // backing stack stays alive only until the scratch save below
    // completes (we never resume from `scratch`).
    let next_ctx = {
        let mut sched = THREADS.lock();

        let prev_id = sched.current;
        if prev_id != BOOTSTRAP_ID {
            sched.threads.remove(&prev_id);
        }

        let next_id = sched
            .ready
            .pop_front()
            .expect("exit_thread: no other thread to switch into");
        sched.current = next_id;

        if next_id == BOOTSTRAP_ID {
            &sched.bootstrap_context as *const Context
        } else {
            let t = sched
                .threads
                .get_mut(&next_id)
                .expect("exit_thread: next ready thread missing from map");
            t.state = ThreadState::Running;
            &t.context as *const Context
        }
    };

    // Throwaway save slot: we never resume from here, so the saved rsp
    // it ends up holding is discarded immediately.
    let mut scratch = Context::default();
    unsafe { switch_context(&mut scratch as *mut Context, next_ctx) };
    // `switch_context` does not return for an exiting thread because
    // nothing will ever switch back in to `scratch`. Hint to the
    // optimiser and satisfy `-> !`.
    unreachable!("exit_thread resumed after switch");
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test_case]
    fn test_thread_ids_are_unique_and_skip_zero() {
        let a = ThreadId::new();
        let b = ThreadId::new();
        assert_ne!(a, b);
        assert!(a.as_u64() >= 1);
        assert!(b.as_u64() > a.as_u64());
        assert_ne!(a, BOOTSTRAP_ID);
    }

    #[test_case]
    fn test_kernel_thread_initial_state_and_stack_setup() {
        extern "C" fn dummy() -> ! {
            loop {}
        }
        let thread = KernelThread::new(dummy);
        assert_eq!(thread.state(), ThreadState::Ready);
        // Saved rsp must lie within the owned stack buffer and be
        // 16-byte aligned + 8 (six register slots below the alignment).
        let sp = thread.context.rsp as usize;
        let base = thread._stack.as_ptr() as usize;
        assert!(sp > base);
        assert!(sp <= base + STACK_SIZE);
        assert_eq!(sp % 8, 0);
    }

    #[test_case]
    fn test_scheduler_spawn_inserts_and_queues() {
        // Use a private scheduler to avoid touching the global one.
        let mut sched = ThreadScheduler::new();
        extern "C" fn dummy() -> ! {
            loop {}
        }
        let id = sched.spawn(dummy);
        assert_eq!(sched.thread_count(), 1);
        assert_eq!(sched.ready_count(), 1);
        assert_eq!(sched.ready.front().copied(), Some(id));
    }

    static SWITCH_HITS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn ping_thread() -> ! {
        SWITCH_HITS.fetch_add(1, Ordering::SeqCst);
        // Yield back to bootstrap so the test can observe progress.
        yield_now();
        SWITCH_HITS.fetch_add(1, Ordering::SeqCst);
        exit_thread();
    }

    #[test_case]
    fn test_yield_round_trips_through_spawned_thread() {
        SWITCH_HITS.store(0, Ordering::SeqCst);
        // Spawn directly into the global scheduler; the test is the
        // bootstrap thread.
        {
            let mut sched = THREADS.lock();
            sched.spawn(ping_thread);
        }
        // First yield: bootstrap -> ping_thread (runs first half, yields).
        yield_now();
        assert_eq!(SWITCH_HITS.load(Ordering::SeqCst), 1);
        // Second yield: bootstrap -> ping_thread (runs second half, exits).
        yield_now();
        assert_eq!(SWITCH_HITS.load(Ordering::SeqCst), 2);
        // ping_thread is gone; nothing else is queued.
        let sched = THREADS.lock();
        assert_eq!(sched.thread_count(), 0);
        assert_eq!(sched.ready_count(), 0);
        assert_eq!(sched.current(), BOOTSTRAP_ID);
    }
}
