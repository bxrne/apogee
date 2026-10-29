//! Ring-3 (userland) transitions and the demo programs.
//!
//! There is no userland loader yet: the demo program is hand-assembled
//! machine code embedded in the kernel image (see [`USER_DEMO]). To
//! launch it we map a code page and a stack page, register the process
//! with the kernel's process service, and spawn a host kernel thread
//! tagged with the new pid. That thread drops into ring 3 via `iretq`.
//!
//! The `int 0x80` transport itself lives in [`crate::syscall`]; this
//! module covers the ring-3 transitions (dropping into and returning
//! from user context) and the demo programs.

use core::arch::asm;
use x86_64::VirtAddr;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB,
};

use crate::gdt;
use crate::kernel::{Caller, KERNEL, Request, Response};
use crate::kinfoln;
use crate::process::ProcessId;
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
///   mov rax, 1                ; sys_write
///   mov rdi, 1                ; fd = stdout
///   lea rsi, [rip + msg]
///   mov rdx, msg_len
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
    0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00, // mov rdi, 1
    0x48, 0x8d, 0x35, 0x18, 0x00, 0x00, 0x00, // lea rsi, [rip+0x18]
    0x48, 0xc7, 0xc2, 0x13, 0x00, 0x00, 0x00, // mov rdx, 19
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
/// Used by the fault-recovery tests to verify a ring-3 page fault
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
/// Used by the fault-recovery tests to verify a ring-3 general
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
            .map_err(|_| "map_to_user_code failed")?
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
            .map_err(|_| "map_to_user stack failed")?
            .flush();
    }

    let entry = VirtAddr::new(USER_CODE_BASE);
    let stack_top = VirtAddr::new(USER_STACK_TOP);
    let pid = match KERNEL.call(Caller::Kernel, Request::ProcessCreate { entry, stack_top }) {
        Response::Process(pid) => pid,
        Response::Err(_) => return Err("process registration denied"),
        _ => return Err("process registration returned an unexpected response"),
    };
    kinfoln!(
        "USER",
        "created process {} entry={:#x} stack_top={:#x}",
        pid.as_u64(),
        USER_CODE_BASE,
        USER_STACK_TOP
    );
    Ok(pid)
}

/// Spawn a kernel thread that, when scheduled, drops into ring 3 for
/// `pid`. The pid travels as the thread's process tag, so there is no
/// global hand-off slot.
pub fn spawn_user_thread(pid: ProcessId) {
    thread::spawn(user_thread_entry, pid);
}

extern "C" fn user_thread_entry() -> ! {
    let pid = thread::current_process();
    let snap = match KERNEL.call(Caller::User(pid), Request::ProcessSnapshot { pid }) {
        Response::Snapshot(Some(snapshot)) => snapshot,
        _ => panic!("user_thread_entry: pid missing from registry"),
    };
    kinfoln!(
        "USER",
        "thread {} entering ring 3 for pid {} @ rip={:#x}",
        thread::current_id().as_u64(),
        pid.as_u64(),
        snap.user_entry.as_u64()
    );
    unsafe { enter_userland(snap.user_entry.as_u64(), snap.user_stack_top.as_u64()) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_user_addresses_are_page_aligned_and_distinct() {
        assert_eq!(USER_CODE_BASE % 4096, 0);
        assert_eq!(USER_STACK_BASE % 4096, 0);
        assert_eq!(USER_STACK_TOP % 4096, 0);
        assert!(USER_STACK_TOP > USER_STACK_BASE);
        assert_ne!(USER_CODE_BASE, USER_STACK_BASE);
    }

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
}
