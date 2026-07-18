use crate::gdt;
use crate::kdebugln;
use crate::println;
use crate::scheduler;
use crate::task::keyboard::add_scancode;
use crate::userland;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin;
use x86_64::PrivilegeLevel;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use x86_64::registers::control::Cr2;
// range is 32-47 for hardware interrupts (IRQs)
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;
pub const SYSCALL_VECTOR: u8 = 0x80;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(gpf_handler);
        // Install the naked syscall ISR. The IDT entry stores only the
        // function address; we transmute the type so the x86_64 crate
        // accepts our naked `extern "C"` function. DPL=3 is required
        // so ring-3 user code can issue `int 0x80`.
        unsafe {
            let typed: extern "x86-interrupt" fn(InterruptStackFrame) =
                core::mem::transmute(userland::syscall_isr as *const ());
            idt[SYSCALL_VECTOR as usize]
                .set_handler_fn(typed)
                .set_privilege_level(PrivilegeLevel::Ring3);
        }
        idt
    };
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

pub fn init_idt() {
    IDT.load();
    kdebugln!(
        "IDT ",
        "ready: timer={} keyboard={} syscall={:#x}",
        InterruptIndex::Timer.as_u8(),
        InterruptIndex::Keyboard.as_u8(),
        SYSCALL_VECTOR
    );
}

// Handlers for CPU exceptions and hardware interrupts

// The general protection fault handler is called when a general protection fault occurs, which
// happens when the CPU detects a violation of the protection rules (e.g., accessing a segment that
// is not present or trying to execute a privileged instruction in user mode). The handler prints
// the error code and the stack frame, and then panics.
extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    println!(
        "EXCEPTION: GENERAL PROTECTION FAULT\nError Code: {}\n{:#?}",
        error_code, stack_frame
    );
    panic!("General protection fault");
}

// The page fault handler is called when a page fault occurs, which happens when the CPU tries to
// access a page that is not present in memory or that the CPU does not have permission to access.
// The handler prints the error code and the address that caused the fault, and then panics.
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let faulting_address = Cr2::read();
    println!(
        "EXCEPTION: PAGE FAULT\nAccessed Address: {:?}\nError Code: {:?}\n{:#?}",
        faulting_address, error_code, stack_frame
    );
    panic!("Page fault");
}

// The breakpoint handler is used for testing and debugging. It will be triggered by the `int3`
// instruction.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

// The double fault handler is used to handle double faults, which occur when an exception is
// triggered while trying to call an exception handler. This can happen, for example, if the stack
// overflows while trying to handle a page fault. The double fault handler must be marked as
// `noreturn` because it will not return to the caller.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    panic!(
        "EXCEPTION: DOUBLE FAULT\n{:#?}\nError code: {}",
        stack_frame, error_code
    );
}

// The timer interrupt handler is called by the hardware timer at regular intervals (e.g., every
// 10ms).
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Lock-free tick — safe from interrupt context, no deadlock window
    // against any non-IRQ caller.
    scheduler::SCHEDULER.tick();

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

// The keyboard interrupt handler is called by the keyboard controller when a key is pressed or
// released.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Read the raw scancode and hand it off to the async keyboard task.
    // Doing the decode here would be slow and would block other interrupts.
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    add_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

#[test_case]
fn test_pic_offsets_are_standard() {
    assert_eq!(PIC_1_OFFSET, 32);
    assert_eq!(PIC_2_OFFSET, 40);
}

#[test_case]
fn test_interrupt_index_values() {
    assert_eq!(InterruptIndex::Timer.as_u8(), PIC_1_OFFSET);
    assert_eq!(InterruptIndex::Keyboard.as_u8(), PIC_1_OFFSET + 1);
}

#[test_case]
fn test_syscall_vector_value() {
    assert_eq!(SYSCALL_VECTOR, 0x80);
}

#[test_case]
fn test_breakpoint_exception() {
    x86_64::instructions::interrupts::int3();
}
