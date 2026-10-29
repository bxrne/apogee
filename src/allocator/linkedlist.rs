//! Free-list (linked-list) allocator.
//!
//! Free regions are threaded together in a singly linked list kept in
//! ascending address order. The list links live inside the free regions
//! themselves, so the allocator needs no auxiliary storage.
//!
//! Allocation is first-fit: walk the list, take the first region that can
//! satisfy the requested size and alignment, and return the aligned address
//! inside it. Space before/after the returned block is put back on the free
//! list when it is large enough to hold a [`ListNode`].
//!
//! Deallocation inserts the region in address order and coalesces it with
//! its neighbours, so fragmentation is bounded by the threshold below rather
//! than by allocation history.

use super::{Locked, align_up};
use alloc::alloc::{GlobalAlloc, Layout};
use core::mem;
use core::ptr;

/// Header written at the start of every free region.
///
/// The node is stored *in* the region it describes; [`size`](Self::size)
/// covers the whole region including this header.
pub struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    pub const fn new(size: usize) -> Self {
        ListNode { size, next: None }
    }

    /// Address of the node / start of the free region.
    pub fn start_addr(&self) -> usize {
        self as *const ListNode as usize
    }

    /// One-past-the-end address of the free region.
    pub fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Removes and returns the `next` link, leaving `self.next` empty.
    pub fn take_next(&mut self) -> Option<&'static mut ListNode> {
        self.next.take()
    }

    pub fn set_next(&mut self, next: Option<&'static mut ListNode>) {
        self.next = next;
    }

    /// Peek at the next free region without unlinking it.
    pub fn next(&self) -> Option<&ListNode> {
        self.next.as_deref()
    }
}

/// Smallest region we can track. Anything below this is either absorbed into
/// the neighbouring allocation or leaked, because it cannot hold a header.
const MIN_BLOCK: usize = mem::size_of::<ListNode>();

/// Rounded-up size for a layout.
///
/// Kept in sync between `alloc` and `dealloc`: both must agree on the size of
/// the region a pointer describes, otherwise the free list is corrupted.
///
/// The result is rounded to the alignment of [`ListNode`] so that the region
/// following an allocation still starts on a valid header boundary even for
/// odd sizes such as 17 bytes.
fn allocation_size(layout: &Layout) -> usize {
    align_up(layout.size().max(MIN_BLOCK), mem::align_of::<ListNode>())
}

/// Alignment used for every allocation.
///
/// Raising the alignment to that of [`ListNode`] guarantees that any pointer we
/// hand out is aligned well enough to host a free-list header when it is
/// returned to us.
fn allocation_align(layout: &Layout) -> usize {
    layout.align().max(mem::align_of::<ListNode>())
}

/// Linked-list backed allocator.
///
/// Unlike a bump allocator this reuses freed regions and coalesces adjacent
/// ones. It is not a fast allocator: `alloc` and `dealloc` are `O(n)` in the
/// number of free regions. It is used here as the fallback behind the
/// fixed-size-block allocator and as the reference implementation for the
/// allocator tests.
pub struct LinkedListAllocator {
    /// Head of the address-ordered free list.
    head: Option<&'static mut ListNode>,
    heap_start: usize,
    heap_end: usize,
}

impl LinkedListAllocator {
    /// Creates an empty allocator. Call [`init`](Self::init) before using it.
    pub const fn empty() -> Self {
        LinkedListAllocator {
            head: None,
            heap_start: 0,
            heap_end: 0,
        }
    }

    /// Initializes the allocator with the given heap bounds.
    ///
    /// The whole range is put on the free list as a single region.
    ///
    /// # Safety
    /// The caller must guarantee that `[heap_start, heap_start + heap_size)`
    /// is valid, unused memory and that this function is called only once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.head = None;
        self.heap_start = heap_start;
        self.heap_end = heap_start.saturating_add(heap_size);

        // A ListNode must be aligned; drop any leading padding that cannot be
        // represented as a free region of its own.
        let start = align_up(heap_start, mem::align_of::<ListNode>());
        let size = self.heap_end.saturating_sub(start);
        if size >= MIN_BLOCK {
            // SAFETY: the caller guarantees `[heap_start, heap_end)` is unused.
            unsafe { self.add_free_region(start, size) };
        }
    }

    /// Total size of the heap managed by this allocator.
    pub fn heap_size(&self) -> usize {
        self.heap_end.saturating_sub(self.heap_start)
    }

    /// Number of free regions currently on the list (for tests / diagnostics).
    pub fn free_region_count(&self) -> usize {
        let mut count = 0;
        let mut current = self.head.as_deref();
        while let Some(node) = current {
            count += 1;
            current = node.next.as_deref();
        }
        count
    }

    /// Total number of free bytes currently tracked by the list.
    pub fn free_bytes(&self) -> usize {
        let mut bytes = 0;
        let mut current = self.head.as_deref();
        while let Some(node) = current {
            bytes += node.size();
            current = node.next.as_deref();
        }
        bytes
    }

    /// First-fit allocation.
    ///
    /// Returns null when no free region can satisfy `layout`.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = allocation_size(&layout);
        let align = allocation_align(&layout);

        match self.find_region(size, align) {
            Some(addr) => addr as *mut u8,
            None => ptr::null_mut(),
        }
    }

    /// Return a block to the free list and coalesce it with free neighbours.
    ///
    /// # Safety
    /// `ptr` must have been returned by a previous [`alloc`](Self::alloc) with
    /// the same `layout`, and must not have been freed already.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let size = allocation_size(&layout);
        let addr = ptr as usize;

        debug_assert!(
            addr >= self.heap_start && addr.saturating_add(size) <= self.heap_end,
            "dealloc: pointer outside the heap"
        );
        debug_assert_eq!(
            align_up(addr, mem::align_of::<ListNode>()),
            addr,
            "dealloc: pointer not aligned for a free-list node"
        );

        // SAFETY: forwarded from the caller's contract; the region is no
        // longer in use.
        unsafe { self.add_free_region(addr, size) };
    }

    /// Find a region that fits `size`/`align`, unlink it, put the leftovers
    /// back, and return the address the caller should use.
    fn find_region(&mut self, size: usize, align: usize) -> Option<usize> {
        let mut slot = &mut self.head;

        loop {
            // Inspect the current node without keeping a borrow alive across
            // the mutation below.
            let fit = {
                let node = slot.as_deref_mut()?;
                let region_start = node.start_addr();
                let region_end = node.end_addr();
                let alloc_start = align_up(region_start, align);

                match alloc_start.checked_add(size) {
                    Some(alloc_end) if alloc_end <= region_end => {
                        Some((region_start, alloc_start, alloc_end, region_end))
                    }
                    _ => None,
                }
            };

            match fit {
                Some((region_start, alloc_start, alloc_end, region_end)) => {
                    // Unlink the chosen region *without* discarding the rest of
                    // the list: the successor is stored in the region itself, so
                    // reconnect the slot to it before the region is reused.
                    let removed = slot.take().expect("region checked above");
                    let successor = removed.next.take();
                    *slot = successor;

                    let front = alloc_start - region_start;
                    let back = region_end - alloc_end;

                    // SAFETY: both slices are inside the region that was just
                    // removed from the free list.
                    unsafe {
                        if front >= MIN_BLOCK {
                            self.add_free_region(region_start, front);
                        }
                        if back >= MIN_BLOCK {
                            self.add_free_region(alloc_end, back);
                        }
                    }

                    return Some(alloc_start);
                }
                None => {
                    let node = slot.as_deref_mut().expect("checked above");
                    slot = &mut node.next;
                }
            }
        }
    }

    /// Insert a free region in address order, then coalesce neighbours.
    ///
    /// # Safety
    /// `[addr, addr + size)` must be unused, aligned for [`ListNode`], at least
    /// [`MIN_BLOCK`] bytes, and inside the managed heap.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        debug_assert!(size >= MIN_BLOCK, "free region smaller than a node");
        debug_assert_eq!(
            align_up(addr, mem::align_of::<ListNode>()),
            addr,
            "free region misaligned for a node"
        );
        debug_assert!(
            addr >= self.heap_start && addr.saturating_add(size) <= self.heap_end,
            "free region outside the heap"
        );

        // Walk to the first region that starts after `addr`. Raw pointers are
        // used for the traversal because mutating the list through `&mut
        // Option<...>` aliases the node borrows; the nodes themselves live in
        // the heap for the lifetime of the allocator, so their addresses are
        // stable.
        let mut slot: *mut Option<&'static mut ListNode> = &mut self.head;
        loop {
            // SAFETY: `slot` always points at a `next` field (or the head)
            // owned by `self`.
            let entry = unsafe { &mut *slot };
            match entry {
                Some(node) if node.start_addr() < addr => {
                    debug_assert!(
                        addr >= node.end_addr(),
                        "free region overlaps an existing region"
                    );
                    slot = &mut node.next as *mut Option<&'static mut ListNode>;
                }
                _ => break,
            }
        }

        // Splice a header for the new region into the list.
        // SAFETY: the caller guarantees the region is unused, large enough to
        // hold a ListNode, and inside the heap.
        unsafe {
            let next = (*slot).take();
            let node = addr as *mut ListNode;
            node.write(ListNode { size, next });
            *slot = Some(&mut *node);
        }

        // SAFETY: the list is now address-ordered and every node is valid.
        unsafe { self.coalesce() };
    }

    /// Merge every pair of adjacent free regions.
    ///
    /// Runs until the node has no adjacent successor, so a region that bridges
    /// two others collapses all three in a single pass.
    unsafe fn coalesce(&mut self) {
        let mut slot: *mut Option<&'static mut ListNode> = &mut self.head;

        loop {
            // SAFETY: `slot` always points at a `next` field (or the head)
            // owned by `self`, and nodes are stable while they are on the list.
            let entry = unsafe { &mut *slot };
            let Some(node) = entry else { break };

            let next_bounds = node.next.as_deref().map(|n| (n.start_addr(), n.size()));
            let merge = matches!(next_bounds, Some((start, _)) if start == node.end_addr());

            if merge {
                let next = node.next.take().expect("merge checked above");
                node.size += next.size();
                node.next = next.next.take();
                // Loop again on the same node: the merged region may now touch
                // the region after it.
            } else {
                slot = &mut node.next as *mut Option<&'static mut ListNode>;
            }
        }
    }
}

unsafe impl GlobalAlloc for Locked<LinkedListAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded contract from `GlobalAlloc::dealloc`.
        unsafe { self.lock().dealloc(ptr, layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HEAP_SIZE: usize = 4096;

    #[repr(align(4096))]
    struct AlignedHeap([u8; TEST_HEAP_SIZE]);

    static mut TEST_HEAP: AlignedHeap = AlignedHeap([0; TEST_HEAP_SIZE]);

    fn test_allocator() -> (LinkedListAllocator, usize) {
        let mut allocator = LinkedListAllocator::empty();
        // SAFETY: tests run sequentially and each one re-initialises the
        // allocator over the same untouched buffer.
        let start = unsafe { core::ptr::addr_of_mut!(TEST_HEAP.0).cast::<u8>() as usize };
        unsafe { allocator.init(start, TEST_HEAP_SIZE) };
        (allocator, start)
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test_case]
    fn test_list_node_layout() {
        static mut NODE: ListNode = ListNode::new(64);
        // SAFETY: single-threaded test; the node is only used here.
        let node = unsafe { &*core::ptr::addr_of!(NODE) };
        assert_eq!(node.size(), 64);
        assert_eq!(node.end_addr() - node.start_addr(), 64);
    }

    #[test_case]
    fn test_init_creates_one_free_region() {
        let (allocator, start) = test_allocator();
        assert_eq!(allocator.free_region_count(), 1);
        let head = allocator.head.as_ref().unwrap();
        assert_eq!(head.start_addr(), start);
        assert_eq!(head.end_addr(), start + TEST_HEAP_SIZE);
    }

    #[test_case]
    fn test_alloc_is_aligned() {
        let (mut allocator, _) = test_allocator();
        for align in [1, 8, 16, 64, 256, 4096] {
            let l = layout(8, align);
            let ptr = allocator.alloc(l);
            assert!(!ptr.is_null(), "allocation with align {} failed", align);
            assert_eq!(ptr as usize % align, 0, "alignment {} not honoured", align);
            assert_eq!(
                ptr as usize % mem::align_of::<ListNode>(),
                0,
                "allocation must be usable as a free node later"
            );
            // Give the block back so the next alignment also starts at the
            // beginning of the heap.
            unsafe { allocator.dealloc(ptr, l) };
        }
    }

    #[test_case]
    fn test_allocations_do_not_overlap() {
        let (mut allocator, _) = test_allocator();
        let a = allocator.alloc(layout(64, 8)) as usize;
        let b = allocator.alloc(layout(64, 8)) as usize;
        assert!(a != 0 && b != 0);
        assert!(b >= a + 64 || a >= b + 64);
    }

    #[test_case]
    fn test_dealloc_reuses_the_freed_region() {
        let (mut allocator, _) = test_allocator();
        let first = allocator.alloc(layout(128, 8));
        assert!(!first.is_null());
        // Force a second region so the first is not simply the tail.
        let _second = allocator.alloc(layout(128, 8));
        unsafe { allocator.dealloc(first, layout(128, 8)) };
        let reused = allocator.alloc(layout(128, 8));
        assert_eq!(reused, first, "freed region should be handed out again");
    }

    #[test_case]
    fn test_adjacent_regions_coalesce() {
        let (mut allocator, _) = test_allocator();
        let a = allocator.alloc(layout(256, 8));
        let b = allocator.alloc(layout(256, 8));
        let c = allocator.alloc(layout(256, 8));
        assert!(!a.is_null() && !b.is_null() && !c.is_null());

        // Free out of order so both coalescing directions are exercised.
        unsafe {
            allocator.dealloc(b, layout(256, 8));
            allocator.dealloc(a, layout(256, 8));
            allocator.dealloc(c, layout(256, 8));
        }

        // The three holes plus the tail should be one region again.
        assert_eq!(allocator.free_region_count(), 1);
        let large = allocator.alloc(layout(768, 8));
        assert_eq!(large, a, "coalesced region should start at the first hole");
    }

    #[test_case]
    fn test_small_layouts_are_rounded_up_and_reusable() {
        let (mut allocator, _) = test_allocator();
        let ptr = allocator.alloc(layout(1, 1));
        assert!(!ptr.is_null());
        unsafe { allocator.dealloc(ptr, layout(1, 1)) };
        let reused = allocator.alloc(layout(1, 1));
        assert_eq!(reused, ptr);
    }

    #[test_case]
    fn test_odd_sizes_keep_free_regions_aligned() {
        let (mut allocator, _) = test_allocator();
        // 17 rounds up to 24 bytes; the region after it must still start on a
        // valid header boundary (8-byte aligned) for the free list to work.
        let a = allocator.alloc(layout(17, 16));
        assert!(!a.is_null());
        assert_eq!(a as usize % 16, 0);

        // The alignment padding before the next block may be smaller than a
        // header and is therefore lost, but the allocation must still work and
        // the surrounding regions must stay usable.
        let b = allocator.alloc(layout(17, 16));
        assert!(!b.is_null());
        assert_eq!(b as usize % 16, 0);
        assert!(b as usize >= a as usize + 24);

        unsafe { allocator.dealloc(a, layout(17, 16)) };
        unsafe { allocator.dealloc(b, layout(17, 16)) };
        assert!(allocator.free_region_count() >= 1);
        assert!(!allocator.alloc(layout(1024, 8)).is_null());
    }

    #[test_case]
    fn test_exhaustion_returns_null() {
        let (mut allocator, _) = test_allocator();
        let too_big = allocator.alloc(layout(TEST_HEAP_SIZE * 2, 8));
        assert!(too_big.is_null());

        while !allocator.alloc(layout(64, 8)).is_null() {}
        // The heap is fragmented/full now; the next allocation must still fail
        // cleanly rather than panic or wrap around.
        assert!(allocator.alloc(layout(64, 8)).is_null());
    }

    #[test_case]
    fn test_unlink_keeps_successor_regions() {
        let (mut allocator, _) = test_allocator();
        // The first allocation splits the single region into [block][tail].
        let a = allocator.alloc(layout(128, 8));
        assert!(!a.is_null());
        assert_eq!(allocator.free_region_count(), 1);

        // Splitting off the next block must not discard the rest of the list.
        let b = allocator.alloc(layout(128, 8));
        assert!(!b.is_null());
        assert_eq!(b as usize, a as usize + 128);

        // A large request must still see whatever is left of the original
        // region; if the unlink dropped the successor, this fails.
        let rest = allocator.alloc(layout(TEST_HEAP_SIZE - 256 - MIN_BLOCK, 8));
        assert!(!rest.is_null());
        assert_eq!(rest as usize, b as usize + 128);
    }

    #[test_case]
    fn test_free_list_keeps_full_heap_after_release() {
        let (mut allocator, start) = test_allocator();
        let mut blocks = [core::ptr::null_mut(); 16];
        for block in blocks.iter_mut() {
            *block = allocator.alloc(layout(128, 8));
            assert!(!block.is_null());
        }
        for block in blocks.iter() {
            unsafe { allocator.dealloc(*block, layout(128, 8)) };
        }
        assert_eq!(allocator.free_region_count(), 1);
        assert_eq!(
            allocator.free_bytes(),
            TEST_HEAP_SIZE,
            "every byte must be back on the free list"
        );
        let whole = allocator.alloc(layout(TEST_HEAP_SIZE - MIN_BLOCK, 8));
        assert_eq!(whole as usize, start);
    }
}
