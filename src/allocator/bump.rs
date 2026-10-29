//! Bump allocator.
//!
//! Hands out memory linearly from a fixed region and never reuses it.
//! [`dealloc`](GlobalAlloc::dealloc) is a deliberate no-op: this allocator is
//! intended for monotonic, boot-time allocation where the whole region is
//! discarded together, not for general kernel use.
//!
//! There is no free list, no coalescing, and no per-allocation bookkeeping.
//! Once the cursor reaches the end of the region, allocation fails and stays
//! failed until the allocator is re-initialised.

use super::{Locked, align_up};
use alloc::alloc::{GlobalAlloc, Layout};
use core::ptr;

pub struct BumpAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
}

impl BumpAllocator {
    /// Creates an empty bump allocator. Call [`init`](Self::init) before use.
    pub const fn empty() -> Self {
        BumpAllocator {
            heap_start: 0,
            heap_end: 0,
            next: 0,
        }
    }

    /// Initializes the allocator with the given heap bounds and resets the
    /// cursor to `heap_start`.
    ///
    /// # Safety
    /// The caller must guarantee that `[heap_start, heap_start + heap_size)`
    /// is valid, unused memory and that this function is called only once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_end = heap_start.saturating_add(heap_size);
        self.next = heap_start;
    }

    /// Current bump cursor. Everything below it has been handed out.
    pub fn cursor(&self) -> usize {
        self.next
    }

    /// Number of bytes still available, ignoring alignment padding.
    pub fn remaining(&self) -> usize {
        self.heap_end.saturating_sub(self.next)
    }

    /// Total size of the region managed by this allocator.
    pub fn heap_size(&self) -> usize {
        self.heap_end.saturating_sub(self.heap_start)
    }

    /// Allocate `layout` by advancing the cursor. Returns null when the region
    /// is exhausted.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let alloc_start = align_up(self.next, layout.align());
        let Some(alloc_end) = alloc_start.checked_add(layout.size()) else {
            return ptr::null_mut();
        };

        if alloc_end > self.heap_end {
            ptr::null_mut()
        } else {
            self.next = alloc_end;
            alloc_start as *mut u8
        }
    }
}

unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocators do not reclaim individual blocks. Reuse requires a
        // free list (see `linkedlist.rs`) or resetting the whole region.
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
        assert_eq!(allocator.remaining(), 0);
    }

    #[test_case]
    fn test_bump_allocator_init() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 4096);
        }
        assert_eq!(allocator.heap_start, 0x1000);
        assert_eq!(allocator.heap_end, 0x1000 + 4096);
        assert_eq!(allocator.cursor(), 0x1000);
        assert_eq!(allocator.remaining(), 4096);
        assert_eq!(allocator.heap_size(), 4096);
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

    #[test_case]
    fn test_bump_advances_monotonically() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 4096);
        }
        let a = allocator.alloc(Layout::from_size_align(8, 8).unwrap());
        let b = allocator.alloc(Layout::from_size_align(8, 8).unwrap());
        assert!(!a.is_null() && !b.is_null());
        assert!(b as usize >= a as usize + 8);
        assert_eq!(allocator.cursor(), b as usize + 8);
    }

    #[test_case]
    fn test_bump_dealloc_is_a_noop() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 4096);
        }
        let a = allocator.alloc(Layout::from_size_align(32, 8).unwrap());
        let cursor_after_a = allocator.cursor();
        let b = allocator.alloc(Layout::from_size_align(32, 8).unwrap());
        // The buffer is owned by the static allocator, so exercise dealloc
        // through the same `Locked` wrapper the global allocator uses.
        let locked = Locked::new(allocator);
        unsafe { locked.dealloc(a, Layout::from_size_align(32, 8).unwrap()) };
        assert_eq!(
            locked.lock().cursor(),
            cursor_after_a + 32,
            "bump dealloc must not move the cursor"
        );
        assert_ne!(a, b);
    }

    #[test_case]
    fn test_bump_exhaustion_returns_null() {
        let mut allocator = BumpAllocator::empty();
        unsafe {
            allocator.init(0x1000, 64);
        }
        assert!(
            !allocator
                .alloc(Layout::from_size_align(64, 8).unwrap())
                .is_null()
        );
        assert!(
            allocator
                .alloc(Layout::from_size_align(8, 8).unwrap())
                .is_null()
        );
    }
}
