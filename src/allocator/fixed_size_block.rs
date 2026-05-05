use crate::allocator::linkedlist::{LinkedListAllocator, ListNode};
use core::alloc::{GlobalAlloc, Layout};
use core::mem;

use super::Locked;

/// Block sizes (bytes) used by the slab-style allocator. Must be powers of two
/// and listed in ascending order so a linear scan picks the smallest fit.
const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

pub struct FixedSizeBlockAllocator {
    list_heads: [Option<&'static mut ListNode>; BLOCK_SIZES.len()],
    fallback_allocator: LinkedListAllocator,
}

impl FixedSizeBlockAllocator {
    pub const fn empty() -> Self {
        const EMPTY_LIST: Option<&'static mut ListNode> = None;
        FixedSizeBlockAllocator {
            list_heads: [EMPTY_LIST; BLOCK_SIZES.len()],
            fallback_allocator: LinkedListAllocator::empty(),
        }
    }

    /// Initializes the allocator with the given heap bounds.
    ///
    /// # Safety
    /// The caller must guarantee the `[heap_start, heap_start + heap_size)`
    /// range is valid, unused memory and that this function is called only once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        // SAFETY: contract is forwarded to the inner allocator.
        unsafe { self.fallback_allocator.init(heap_start, heap_size) };
    }

    /// Returns the index in `BLOCK_SIZES` of the smallest block that satisfies
    /// `layout`, or `None` if the layout is larger than every available block.
    fn list_index(layout: &Layout) -> Option<usize> {
        let required = layout.size().max(layout.align());
        BLOCK_SIZES.iter().position(|&s| s >= required)
    }

    fn fallback_alloc(&mut self, layout: Layout) -> *mut u8 {
        self.fallback_allocator.alloc(layout)
    }
}

unsafe impl GlobalAlloc for Locked<FixedSizeBlockAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut allocator = self.lock();

        match FixedSizeBlockAllocator::list_index(&layout) {
            Some(index) => match allocator.list_heads[index].take() {
                Some(node) => {
                    let next = node.take_next();
                    let addr = node.start_addr() as *mut u8;
                    allocator.list_heads[index] = next;
                    addr
                }
                None => {
                    // No cached block — allocate one of the slab's exact size.
                    let block_size = BLOCK_SIZES[index];
                    let block_align = block_size;
                    let layout = Layout::from_size_align(block_size, block_align)
                        .expect("invalid block layout");
                    allocator.fallback_alloc(layout)
                }
            },
            None => allocator.fallback_alloc(layout),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut allocator = self.lock();

        match FixedSizeBlockAllocator::list_index(&layout) {
            Some(index)
                if BLOCK_SIZES[index] >= mem::size_of::<ListNode>()
                    && BLOCK_SIZES[index] >= mem::align_of::<ListNode>() =>
            {
                let new_node_ptr = ptr as *mut ListNode;
                // SAFETY: `ptr` was returned by a prior `alloc` for the same
                // layout, so it is a valid, properly aligned, owned block of
                // at least BLOCK_SIZES[index] bytes — enough for a ListNode.
                unsafe {
                    new_node_ptr.write(ListNode::new(0));
                    let head = allocator.list_heads[index].take();
                    (*new_node_ptr).set_next(head);
                    allocator.list_heads[index] = Some(&mut *new_node_ptr);
                }
            }
            // Block smaller than a ListNode (or no slab at all): hand back to
            // the underlying allocator. Since the fallback is currently bump-
            // style, it only reclaims when its allocation count returns to
            // zero — these blocks are effectively leaked until then.
            _ => {
                // SAFETY: forwarded from `GlobalAlloc::dealloc` contract.
                unsafe { allocator.fallback_allocator.dealloc(ptr, layout) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_block_sizes_ascending_powers_of_two() {
        for window in BLOCK_SIZES.windows(2) {
            assert!(window[0] < window[1], "BLOCK_SIZES must be ascending");
        }
        for &size in BLOCK_SIZES {
            assert!(size.is_power_of_two(), "block sizes must be powers of two");
        }
    }

    #[test_case]
    fn test_list_index_smallest_fit() {
        let layout = Layout::from_size_align(1, 1).unwrap();
        assert_eq!(FixedSizeBlockAllocator::list_index(&layout), Some(0));
    }

    #[test_case]
    fn test_list_index_exact_match() {
        let layout = Layout::from_size_align(64, 1).unwrap();
        assert_eq!(FixedSizeBlockAllocator::list_index(&layout), Some(3));
    }

    #[test_case]
    fn test_list_index_uses_alignment_when_larger() {
        let layout = Layout::from_size_align(8, 64).unwrap();
        assert_eq!(FixedSizeBlockAllocator::list_index(&layout), Some(3));
    }

    #[test_case]
    fn test_list_index_too_large_returns_none() {
        let layout = Layout::from_size_align(8192, 1).unwrap();
        assert_eq!(FixedSizeBlockAllocator::list_index(&layout), None);
    }

    #[test_case]
    fn test_some_block_can_hold_list_node() {
        // At least one slab must be able to store a ListNode in place so that
        // freed blocks of that size can be threaded onto the free list.
        let fits = BLOCK_SIZES.iter().any(|&s| {
            s >= mem::size_of::<ListNode>() && s >= mem::align_of::<ListNode>()
        });
        assert!(fits, "no slab is large enough to host a ListNode");
    }

    #[test_case]
    fn test_empty_starts_with_no_cached_blocks() {
        let alloc = FixedSizeBlockAllocator::empty();
        for head in alloc.list_heads.iter() {
            assert!(head.is_none());
        }
    }
}
