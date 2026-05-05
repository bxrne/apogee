use super::{Locked, align_up};
use alloc::alloc::{GlobalAlloc, Layout};
use core::ptr;

/// Bump allocator: hands out memory linearly until the heap is exhausted.
/// Frees only when the allocation count returns to zero.
pub struct BumpAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
    allocations: usize,
}

impl BumpAllocator {
    /// Creates an empty bump allocator. Call [`init`] before using it.
    pub const fn empty() -> Self {
        BumpAllocator {
            heap_start: 0,
            heap_end: 0,
            next: 0,
            allocations: 0,
        }
    }

    /// Initializes the allocator with the given heap bounds.
    ///
    /// # Safety
    /// The caller must guarantee that the `[heap_start, heap_start + heap_size)`
    /// range is valid, unused memory and that this function is called only once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_end = heap_start.saturating_add(heap_size);
        self.next = heap_start;
        self.allocations = 0;
    }
}

unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut bump = self.lock();

        let alloc_start = align_up(bump.next, layout.align());
        let alloc_end = match alloc_start.checked_add(layout.size()) {
            Some(end) => end,
            None => return ptr::null_mut(),
        };

        if alloc_end > bump.heap_end {
            ptr::null_mut()
        } else {
            bump.next = alloc_end;
            bump.allocations += 1;
            alloc_start as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        let mut bump = self.lock();

        bump.allocations -= 1;
        if bump.allocations == 0 {
            bump.next = bump.heap_start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test_case]
    fn test_bump_allocator_empty() {
        let allocator = BumpAllocator::empty();
        assert_eq!(allocator.heap_start, 0);
        assert_eq!(allocator.heap_end, 0);
        assert_eq!(allocator.next, 0);
        assert_eq!(allocator.allocations, 0);
    }

    #[test_case]
    fn test_bump_allocator_init() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 4096);
        }
        assert_eq!(allocator.heap_start, 0x1000);
        assert_eq!(allocator.heap_end, 0x1000 + 4096);
        assert_eq!(allocator.next, 0x1000);
        assert_eq!(allocator.allocations, 0);
    }

    #[test_case]
    fn test_align_up_function() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(1000, 4096), 4096);
    }

    #[test_case]
    fn test_bump_allocator_alignment() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 4096);
        }
        let locked_allocator = Locked::new(allocator);
        let layout = Layout::from_size_align(8, 16).unwrap();
        let ptr = unsafe { locked_allocator.alloc(layout) };
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 16, 0);
    }
}
