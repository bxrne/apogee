//! Dynamic memory allocation and heap management.
//!
//! This module provides the global allocator implementation using the
//! `linked_list_allocator` crate, enabling `alloc::Box`, `alloc::Vec`,
//! and `alloc::Rc` (reference counting) in the bare-metal kernel.

use x86_64::{
    VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB, mapper::MapToError,
    },
};

pub mod bump;
pub mod fixed_size_block;
pub mod linkedlist;

use fixed_size_block::FixedSizeBlockAllocator;

#[global_allocator]
static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::empty());

/// Virtual address where the heap starts.
/// Chosen to be in a high, unused region of the virtual address space.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Size of the heap in bytes (100 KiB).
pub const HEAP_SIZE: usize = 100 * 1024;

/// Initializes the kernel heap by mapping virtual pages to physical frames.
///
/// This function:
/// 1. Creates a range of virtual pages spanning the heap region
/// 2. Allocates physical frames for each page from the frame allocator
/// 3. Maps each page to its corresponding frame with read-write permissions
/// 4. Initializes the global allocator with the heap's virtual address range
///
/// # Arguments
/// * `mapper` - The page table mapper used to create page mappings
/// * `frame_allocator` - The frame allocator providing physical memory frames
///
/// # Returns
/// * `Ok(())` on successful initialization
/// * `Err(MapToError)` if page mapping or frame allocation fails
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() };
    }
    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

// A simple wrapper around `spin::Mutex` to provide thread-safe access to the global allocator.
pub struct Locked<A> {
    inner: spin::Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: spin::Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<'_, A> {
        self.inner.lock()
    }
}

/// Align the given address `addr` upwards to alignment `align`.
///
/// Requires that `align` is a power of two.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_heap_start_aligned() {
        assert_eq!(HEAP_START % 4096, 0, "HEAP_START must be page-aligned");
    }

    #[test_case]
    fn test_heap_size_multiple_of_page() {
        assert_eq!(
            HEAP_SIZE % 4096,
            0,
            "HEAP_SIZE must be multiple of page size"
        );
    }

    #[test_case]
    fn test_heap_size_100_kib() {
        assert_eq!(HEAP_SIZE, 100 * 1024, "HEAP_SIZE should be 100 KiB");
    }

    #[test_case]
    fn test_heap_start_address_range() {
        assert!(
            HEAP_START > 0x1_0000,
            "HEAP_START should be in valid address range"
        );
    }
}
