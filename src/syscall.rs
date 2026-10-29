//! Ring-3 syscall ABI: the `int 0x80` transport, its saved register
//! frame, and dispatch into the kernel's typed request/response
//! boundary.
//!
//! `int 0x80` is only a transport: [`syscall_dispatch`] decodes the
//! register frame into a [`Request`] and calls
//! [`KERNEL.call`](crate::kernel::KERNEL). The same requests are
//! available to kernel callers, so authorization and validation live in
//! one place.

use crate::kernel::{Caller, KERNEL, Request, Response, SysError};
use crate::kinfoln;
use crate::kwarnln;
use crate::thread;
use crate::userland::{USER_CODE_BASE, USER_STACK_TOP};
use core::arch::naked_asm;

/// Layout of the saved register frame [`syscall_isr`] builds before
/// calling [`syscall_dispatch`]. The order matches the push sequence;
/// the iretq frame the CPU itself pushed sits below it.
#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    // Below this point is the CPU-pushed iretq frame:
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub user_rsp: u64,
    pub user_ss: u64,
}

/// Naked ISR wired to `int 0x80`. Saves all GP regs in the order
/// expected by [`SyscallFrame`], calls [`syscall_dispatch`] with the
/// frame pointer in `rdi`, restores the regs, and `iretq`s.
///
/// The 15 saved registers (8 bytes each) bring rsp from the entry
/// alignment of `8 mod 16` (CPU pushed five 8-byte iretq slots from a
/// 16-aligned `TSS.privilege_stack_table[0]`) down to `0 mod 16`,
/// which is the SysV-required alignment immediately before `call`.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_isr() {
    naked_asm!(
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "iretq",
        dispatch = sym syscall_dispatch,
    )
}

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_YIELD: u64 = 3;

const STDOUT: u64 = 1;

/// Maximum byte count accepted by a single syscall buffer.
const MAX_USER_IO: usize = 4096;

fn errno(error: SysError) -> u64 {
    error.errno() as u64
}

#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) {
    // SAFETY: `frame` is `rsp` immediately after our ISR's pushes.
    // It points at the saved GP-reg block on the privilege stack.
    let frame = unsafe { &mut *frame };
    let caller = Caller::User(thread::current_process());

    frame.rax = match frame.rax {
        SYS_EXIT => sys_exit(caller, frame.rdi as i32),
        SYS_WRITE => sys_write(caller, frame),
        SYS_READ => sys_read(caller),
        SYS_YIELD => sys_yield(caller),
        n => {
            kwarnln!("SYSC", "unknown syscall {} from ring 3", n);
            errno(SysError::Unsupported)
        }
    };
}

/// `sys_exit(code)` — terminate the calling process and its host thread.
fn sys_exit(caller: Caller, code: i32) -> ! {
    let Caller::User(pid) = caller else {
        panic!("sys_exit called from kernel context");
    };
    kinfoln!("USER", "sys_exit({}) for pid {}", code, pid.as_u64());
    let _ = KERNEL.call(caller, Request::ProcessExit { pid, code });
    thread::exit_thread();
}

/// `sys_write(fd, ptr, len) -> len` — write a validated user buffer.
fn sys_write(caller: Caller, frame: &SyscallFrame) -> u64 {
    if frame.rdi != STDOUT {
        return errno(SysError::BadHandle);
    }
    let Some(bytes) = user_bytes(frame.rsi, frame.rdx) else {
        return errno(SysError::BadPointer);
    };
    match KERNEL.call(caller, Request::ConsoleWrite(bytes)) {
        Response::Written(written) => written as u64,
        Response::Err(error) => errno(error),
        _ => errno(SysError::Unsupported),
    }
}

/// `sys_read() -> u8` — pop one decoded keyboard byte, or `-EAGAIN`.
fn sys_read(caller: Caller) -> u64 {
    match KERNEL.call(caller, Request::InputReadByte) {
        Response::Byte(Some(byte)) => byte as u64,
        Response::Byte(None) => errno(SysError::WouldBlock),
        Response::Err(error) => errno(error),
        _ => errno(SysError::Unsupported),
    }
}

/// `sys_yield()` — voluntarily give up the CPU.
fn sys_yield(caller: Caller) -> u64 {
    match KERNEL.call(caller, Request::ThreadYield) {
        Response::Ok => 0,
        Response::Err(error) => errno(error),
        _ => errno(SysError::Unsupported),
    }
}

/// Validate a ring-3 buffer before the kernel dereferences it.
///
/// The kernel and every process currently share one address space, so a
/// pointer check is the only isolation available. Restrict buffers to
/// the mapped user code/stack window and to one page-sized transfer.
fn user_bytes(ptr: u64, len: u64) -> Option<&'static [u8]> {
    if len > MAX_USER_IO as u64 {
        return None;
    }
    let end = ptr.checked_add(len)?;
    if ptr < USER_CODE_BASE || end > USER_STACK_TOP {
        return None;
    }
    // SAFETY: the range lies inside the user window, which the process
    // setup mapped USER_ACCESSIBLE. The kernel shares the page tables,
    // so the bytes are readable from ring 0.
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_syscall_frame_layout_is_120_plus_40_bytes() {
        // 15 saved GP regs (120) + 5 iretq slots (40) = 160 bytes.
        assert_eq!(core::mem::size_of::<SyscallFrame>(), 160);
    }

    #[test_case]
    fn test_user_buffer_validation_rejects_out_of_range() {
        // These paths return before constructing a slice, so they are
        // safe even though the pages are not mapped in the test kernel.
        assert!(user_bytes(0x1000, 1).is_none());
        assert!(user_bytes(USER_STACK_TOP - 1, 2).is_none());
        assert!(user_bytes(USER_CODE_BASE, MAX_USER_IO as u64 + 1).is_none());
        assert!(user_bytes(u64::MAX, 2).is_none());
    }
}
