//! Global Descriptor Table, Task State Segment, and segment selectors.
//!
//! Apogee uses a flat-memory model and segment registers serve mostly
//! as privilege-level handles. The GDT installed here exposes:
//!
//! - the kernel code/data segments (ring 0)
//! - the user code/data segments (ring 3) — used by [`crate::userland`]
//!   when transitioning into ring 3 via `iretq` and by the syscall ISR
//!   on the way back out
//! - a TSS with two stacks plumbed in:
//!   - `interrupt_stack_table[DOUBLE_FAULT_IST_INDEX]` — a dedicated
//!     stack the CPU switches to on a double fault, so a stack overflow
//!     can be handled cleanly
//!   - `privilege_stack_table[0]` — the ring-0 stack the CPU switches
//!     to on a ring-3 → ring-0 transition (e.g. `int 0x80`)

use lazy_static::lazy_static;
use x86_64::VirtAddr;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the dedicated double-fault IST stack.
const IST_STACK_SIZE: usize = 4096 * 5;

/// Size of the ring-0 stack used on syscall entry from ring 3.
const PRIVILEGE_STACK_SIZE: usize = 4096 * 5;

/// 16-byte aligned stack storage. We need the alignment so that the
/// CPU's automatic push of the iretq frame leaves rsp at a SysV-friendly
/// offset for the syscall ISR's call into Rust.
#[repr(C, align(16))]
struct AlignedStack<const N: usize>([u8; N]);

static mut IST_STACK: AlignedStack<IST_STACK_SIZE> = AlignedStack([0; IST_STACK_SIZE]);
static mut PRIVILEGE_STACK: AlignedStack<PRIVILEGE_STACK_SIZE> =
    AlignedStack([0; PRIVILEGE_STACK_SIZE]);

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();

        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            // IST_STACK is only ever consumed by the CPU as a stack for
            // the double-fault IST entry; it is never read or written
            // as a Rust reference, so taking its address is safe.
            let base = VirtAddr::from_ptr(core::ptr::addr_of!(IST_STACK));
            base + IST_STACK_SIZE as u64
        };

        // Stack the CPU switches to on every ring 3 → ring 0
        // transition (interrupts, exceptions, `int 0x80`).
        tss.privilege_stack_table[0] = {
            let base = VirtAddr::from_ptr(core::ptr::addr_of!(PRIVILEGE_STACK));
            base + PRIVILEGE_STACK_SIZE as u64
        };

        tss
    };
}

/// All segment selectors installed by [`init`].
#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.add_entry(Descriptor::kernel_code_segment());
        let kernel_data = gdt.add_entry(Descriptor::kernel_data_segment());
        let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
        // user_data BEFORE user_code keeps the layout SYSRET-friendly
        // (STAR.usercs implicitly = STAR.userdata - 8). We don't use
        // SYSRET today but the convention costs nothing.
        let user_data = gdt.add_entry(Descriptor::user_data_segment());
        let user_code = gdt.add_entry(Descriptor::user_code_segment());
        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_code,
                user_data,
                tss,
            },
        )
    };
}

/// Read the active selectors. Useful for the userland transition path
/// which needs the user CS/SS to push onto the iretq frame.
pub fn selectors() -> Selectors {
    GDT.1
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, Segment};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        load_tss(GDT.1.tss);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_double_fault_ist_index_zero() {
        assert_eq!(DOUBLE_FAULT_IST_INDEX, 0);
    }

    #[test_case]
    fn test_ist_index_within_bounds() {
        assert!(DOUBLE_FAULT_IST_INDEX < 7);
    }

    #[test_case]
    fn test_ist_stack_size_is_20_kib() {
        assert_eq!(IST_STACK_SIZE, 20480);
    }

    #[test_case]
    fn test_user_selectors_have_rpl3() {
        // After init() runs in test_kernel_main, GDT is loaded and the
        // selectors are observable. User selectors must carry RPL=3 so
        // iretq actually transitions to ring 3.
        let s = selectors();
        assert_eq!(s.user_code.rpl() as u8, 3);
        assert_eq!(s.user_data.rpl() as u8, 3);
        assert_eq!(s.kernel_code.rpl() as u8, 0);
    }
}
