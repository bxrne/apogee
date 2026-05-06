use x86_64::structures::paging::PageTable;

// The process struct represents a running process in the operating system. 
pub struct Process {
    pub pid: usize,
    pub page_table: PageTable,
    pub kernel_stack: KernelStack,
    pub ctx: UserContext,
}

// The kernel stack is used for handling interrupts and system calls when the process is running in
// kernel mode.
pub struct KernelStack {
    pub start: usize,
    pub end: usize,
}

// The user context contains the state of the user-space registers when a process is interrupted or
// switched out.
pub struct UserContext {
    // instruction ptr
    pub rip: usize,
    // stack ptr
    pub rsp: usize,
    // flags register
    pub rflags: usize,

    // general purpose registers
    pub rax: usize,
    pub rbx: usize,
    pub rcx: usize,
    pub rdx: usize,
    pub rsi: usize,
    pub rdi: usize,
    pub rbp: usize,

    // additional registers
    pub r8: usize,
    pub r9: usize,
    pub r10: usize,
    pub r11: usize,
    pub r12: usize,
    pub r13: usize,
    pub r14: usize,
    pub r15: usize,
}
