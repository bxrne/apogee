use crate::gdt;
use crate::scheduler;
use crate::println;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

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
        idt[SYSCALL_VECTOR as usize].set_handler_fn(syscall_interrupt_handler);
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
    crate::kdebugln!(
        "idt ready: timer={} keyboard={} syscall={:#x}",
        InterruptIndex::Timer.as_u8(),
        InterruptIndex::Keyboard.as_u8(),
        SYSCALL_VECTOR
    );
}

// Handlers for CPU exceptions and hardware interrupts

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
    // Keep IRQ work minimal: only tick scheduler state here.
    scheduler::SCHEDULER.lock().tick();

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
    crate::task::keyboard::add_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

// Placeholder syscall interrupt handler for `int 0x80`.
// Software interrupts do not require PIC EOI.
extern "x86-interrupt" fn syscall_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::kdebugln!("syscall interrupt hit (stub)");
}

#[test_case]
fn test_breakpoint_exception() {
    x86_64::instructions::interrupts::int3();
}
