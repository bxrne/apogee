//! Ring-3 (userland) transitions, syscall dispatch, and the demo
//! user program.
//!
//! There is no userland loader yet: the demo program is hand-assembled
//! machine code embedded in the kernel image (see [`USER_DEMO`]). To
//! launch it we
//!
//! 1. allocate a code frame and a stack frame from the kernel frame
//!    allocator,
//! 2. map both at fixed virtual addresses inside the user address
//!    range with `USER_ACCESSIBLE` flags (and the same on every
//!    intermediate page-table level),
//! 3. copy [`USER_DEMO`] into the code page,
//! 4. register a [`Process`] in the global registry,
//! 5. spawn a kernel thread whose entry function calls
//!    [`enter_userland`] to drop into ring 3 via `iretq`.
//!
//! When the user program issues `int 0x80`, the CPU walks the IDT to
//! the gate installed at [`crate::interrupts::SYSCALL_VECTOR`] (DPL=3
//! so ring 3 may invoke it), switches to the kernel stack registered
//! in `TSS.privilege_stack_table[0]`, and jumps to [`syscall_isr`].
//! That naked stub saves the user's GP regs, calls
//! [`syscall_dispatch`] in Rust, restores the regs, and `iretq`s back
//! to user mode. `sys_exit` instead reaps the process and tears down
//! the host kernel thread via [`thread::exit_thread`], so control
//! returns to the bootstrap thread rather than ring 3.

use core::arch::{asm, naked_asm};
use x86_64::VirtAddr;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB,
};

use crate::gdt;
use crate::kinfoln;
use crate::kwarnln;
use crate::process::{self, Process, ProcessId};
use crate::thread;

/// Virtual address of the user code page in every process's address
/// space. We support a single user process at a time today, so a fixed
/// address is fine.
pub const USER_CODE_BASE: u64 = 0x4000_0000_0000;
/// Virtual address of the user stack base (inclusive low end).
pub const USER_STACK_BASE: u64 = 0x4000_0000_2000;
/// One-past-the-top of the user stack — initial RSP for the user.
pub const USER_STACK_TOP: u64 = 0x4000_0000_3000;

/// Hand-assembled user-mode demo program.
///
/// ```text
///   mov rax, 1                ; sys_print
///   lea rdi, [rip + msg]
///   mov rsi, msg_len
///   int 0x80
///   mov rax, 0                ; sys_exit
///   xor rdi, rdi
///   int 0x80
///   hlt                       ; safety net — never reached
/// jmp_self:
///   jmp jmp_self
/// msg: "hello from ring 3!\n"
/// ```
///
/// Sized so the entire program (code + embedded string) fits in a
/// single 4 KiB page.
#[rustfmt::skip]
static USER_DEMO: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
    0x48, 0x8d, 0x3d, 0x18, 0x00, 0x00, 0x00, // lea rdi, [rip+0x18]
    0x48, 0xc7, 0xc6, 0x13, 0x00, 0x00, 0x00, // mov rsi, 19
    0xcd, 0x80,                               // int 0x80
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x48, 0x31, 0xff,                         // xor rdi, rdi
    0xcd, 0x80,                               // int 0x80
    0xf4,                                     // hlt
    0xeb, 0xfe,                               // jmp $-0
    // "hello from ring 3!\n"
    b'h', b'e', b'l', b'l', b'o', b' ', b'f', b'r', b'o', b'm',
    b' ', b'r', b'i', b'n', b'g', b' ', b'3', b'!', b'\n',
];

const _: () = assert!(USER_DEMO.len() <= 4096, "user demo doesn't fit in one page");

/// Hand-assembled ring-3 demo that deliberately faults by writing
/// through an address nowhere near the mapped user region:
///
/// ```text
///   mov rax, 0x0000500000000000
///   mov qword ptr [rax], 0    ; page fault: not present, from ring 3
///   hlt                       ; safety net — never reached
/// jmp_self:
///   jmp jmp_self
/// ```
///
/// Used by [`crate::interrupts`] tests to verify a ring-3 page fault
/// kills the offending process instead of panicking the kernel.
#[rustfmt::skip]
pub static CRASH_PF_DEMO: &[u8] = &[
    0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, // mov rax, 0x0000500000000000
    0x48, 0xc7, 0x00, 0x00, 0x00, 0x00, 0x00,                   // mov qword [rax], 0
    0xf4,                                                       // hlt
    0xeb, 0xfe,                                                 // jmp $-0
];

/// Hand-assembled ring-3 demo that deliberately faults by executing a
/// privileged instruction (`hlt` requires CPL=0, so ring 3 takes a
/// general-protection fault instead):
///
/// ```text
///   hlt                       ; #GP: privileged instruction from ring 3
/// jmp_self:
///   jmp jmp_self
/// ```
///
/// Used by [`crate::interrupts`] tests to verify a ring-3 general
/// protection fault kills the offending process instead of panicking
/// the kernel.
#[rustfmt::skip]
pub static CRASH_GPF_DEMO: &[u8] = &[
    0xf4,       // hlt
    0xeb, 0xfe, // jmp $-0
];

/// Set up the demo user process: allocate + map code and stack pages
/// (with `USER_ACCESSIBLE` set on all page-table levels), copy the
/// demo program into the code page, and register the process. Returns
/// the new pid.
pub fn create_demo_process<M, A>(
    mapper: &mut M,
    frame_allocator: &mut A,
) -> Result<ProcessId, &'static str>
where
    M: Mapper<Size4KiB>,
    A: FrameAllocator<Size4KiB>,
{
    create_process_from_program(mapper, frame_allocator, USER_DEMO)
}

/// Same as [`create_demo_process`] but loads `program` instead of the
/// built-in [`USER_DEMO`] blob. Used to launch the crash-test demos.
pub fn create_process_from_program<M, A>(
    mapper: &mut M,
    frame_allocator: &mut A,
    program: &[u8],
) -> Result<ProcessId, &'static str>
where
    M: Mapper<Size4KiB>,
    A: FrameAllocator<Size4KiB>,
{
    if program.len() > 4096 {
        return Err("program doesn't fit in one page");
    }

    let leaf_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    // Intermediate page-table levels must also be user-accessible,
    // otherwise the page-walker rejects user accesses to a leaf that
    // *is* user-accessible.
    let parent_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    // ---- code page ------------------------------------------------
    let code_frame: PhysFrame = frame_allocator
        .allocate_frame()
        .ok_or("no frame for user code")?;
    let code_page: Page = Page::containing_address(VirtAddr::new(USER_CODE_BASE));
    unsafe {
        mapper
            .map_to_with_table_flags(
                code_page,
                code_frame,
                leaf_flags,
                parent_flags,
                frame_allocator,
            )
            .map_err(|_| "map_to user code failed")?
            .flush();
        // Copy the demo program into the freshly-mapped code page.
        // The page is mapped writable for setup; ring 3 runs fine
        // against a writable code page (we don't enforce W^X yet).
        core::ptr::copy_nonoverlapping(program.as_ptr(), USER_CODE_BASE as *mut u8, program.len());
    }

    // ---- stack page -----------------------------------------------
    let stack_frame: PhysFrame = frame_allocator
        .allocate_frame()
        .ok_or("no frame for user stack")?;
    let stack_page: Page = Page::containing_address(VirtAddr::new(USER_STACK_BASE));
    unsafe {
        mapper
            .map_to_with_table_flags(
                stack_page,
                stack_frame,
                leaf_flags,
                parent_flags,
                frame_allocator,
            )
            .map_err(|_| "map_to user stack failed")?
            .flush();
    }

    let process = Process::new(VirtAddr::new(USER_CODE_BASE), VirtAddr::new(USER_STACK_TOP));
    let pid = process::register(process);
    kinfoln!(
        "USER",
        "created process {} entry={:#x} stack_top={:#x}",
        pid.as_u64(),
        USER_CODE_BASE,
        USER_STACK_TOP
    );
    Ok(pid)
}

/// Stash for the pid the next [`user_thread_entry`] should pick up.
/// We currently support a single user process at a time, so a single
/// slot is enough.
static PENDING_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Spawn a kernel thread that, when scheduled, drops into ring 3 for
/// `pid`. When the user program calls `sys_exit` the host kernel
/// thread is torn down via [`thread::exit_thread`], so the bootstrap
/// thread regains the CPU and the rest of `kernel_main` proceeds.
pub fn spawn_user_thread(pid: ProcessId) {
    PENDING_PID.store(pid.as_u64(), core::sync::atomic::Ordering::SeqCst);
    let mut sched = thread::THREADS.lock();
    sched.spawn(user_thread_entry);
}

extern "C" fn user_thread_entry() -> ! {
    let raw = PENDING_PID.swap(0, core::sync::atomic::Ordering::SeqCst);
    let pid = ProcessId::from_raw(raw);
    let snap = process::snapshot(pid).expect("user_thread_entry: pid missing from registry");
    kinfoln!(
        "USER",
        "thread {} entering ring 3 for pid {} @ rip={:#x}",
        thread::THREADS.lock().current().as_u64(),
        pid.as_u64(),
        snap.user_entry.as_u64()
    );
    unsafe { enter_userland(snap.user_entry.as_u64(), snap.user_stack_top.as_u64()) };
}

/// Drop into ring 3 at (`rip`, `rsp`). Builds the iretq frame
/// (top-to-bottom: ss, rsp, rflags, cs, rip) and `iretq`s. Never
/// returns.
///
/// # Safety
/// `rip` and `rsp` must point into pages mapped `USER_ACCESSIBLE` in
/// the active page table.
pub unsafe fn enter_userland(rip: u64, rsp: u64) -> ! {
    let sels = gdt::selectors();
    let user_cs = sels.user_code.0 as u64;
    let user_ss = sels.user_data.0 as u64;
    unsafe {
        asm!(
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = in(reg) user_ss,
            rsp = in(reg) rsp,
            rflags = const 0x202u64,    // IF=1, reserved bit 1 set
            cs = in(reg) user_cs,
            rip = in(reg) rip,
            options(noreturn),
        );
    }
}

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
const SYS_PRINT: u64 = 1;

/// Maximum byte count accepted by `sys_print` to bound the slice we
/// build over user memory.
const SYS_PRINT_MAX: u64 = 4096;

#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) {
    // SAFETY: `frame` is `rsp` immediately after our ISR's pushes.
    // It points at the saved GP-reg block on the privilege stack.
    let frame = unsafe { &mut *frame };
    match frame.rax {
        SYS_EXIT => sys_exit(frame.rdi as i32),
        SYS_PRINT => sys_print(frame, frame.rdi, frame.rsi),
        n => {
            kwarnln!("SYCL", "unknown syscall {} from ring 3", n);
            frame.rax = u64::MAX;
        }
    }
}

/// `sys_exit(code: i32)` — terminate the current process.
///
/// Removes the process from the registry, decrements the live
/// counters, and tears down the host kernel thread via
/// [`thread::exit_thread`]. Control returns to whichever thread is
/// next on the kernel-thread ready queue (the bootstrap thread, in
/// the demo).
fn sys_exit(code: i32) -> ! {
    kinfoln!("USER", "sys_exit({})", code);
    // Today there is at most one user process at a time; pick the
    // first one out of the registry.
    let pid_opt = process::PROCESSES.lock().keys().next().copied();
    if let Some(pid) = pid_opt {
        process::exit(pid, code);
    }
    thread::exit_thread();
}

/// `sys_print(ptr, len) -> len` — print `len` bytes from user memory.
///
/// User memory is shared with the kernel address space today, so the
/// kernel can read the buffer directly. Bytes are forwarded to the
/// VGA / serial writer one at a time so a non-UTF-8 buffer doesn't
/// panic.
fn sys_print(frame: &mut SyscallFrame, ptr: u64, len: u64) {
    if len > SYS_PRINT_MAX {
        kwarnln!("SYCL", "sys_print: refusing oversized len {}", len);
        frame.rax = u64::MAX;
        return;
    }
    // SAFETY: ptr+len lives in the user's address space, which is
    // mapped into ours; the bound check above keeps len modest.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    // Forward to the VGA writer so user output appears on screen.
    for &b in bytes {
        crate::print!("{}", b as char);
    }
    frame.rax = len;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_user_demo_fits_in_a_page() {
        assert!(USER_DEMO.len() <= 4096);
        assert!(USER_DEMO.len() >= 19, "must include the message");
    }

    #[test_case]
    fn test_user_addresses_are_page_aligned_and_distinct() {
        assert_eq!(USER_CODE_BASE % 4096, 0);
        assert_eq!(USER_STACK_BASE % 4096, 0);
        assert_eq!(USER_STACK_TOP % 4096, 0);
        assert!(USER_STACK_TOP > USER_STACK_BASE);
        assert_ne!(USER_CODE_BASE, USER_STACK_BASE);
    }

    #[test_case]
    fn test_syscall_frame_layout_is_120_plus_40_bytes() {
        // 15 saved GP regs (120) + 5 iretq slots (40) = 160 bytes.
        assert_eq!(core::mem::size_of::<SyscallFrame>(), 160);
    }
}
