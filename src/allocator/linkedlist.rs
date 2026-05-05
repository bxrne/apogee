use super::{Locked, align_up};
use alloc::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ptr;

pub struct ListNode {
    value: usize,
    next: Option<&'static mut ListNode>, // must live duration of heap
}

impl ListNode {
    pub const fn new(value: usize) -> Self {
        ListNode { value, next: None }
    }

    pub fn start_addr(&self) -> usize {
        self as *const ListNode as usize
    }

    pub fn end_addr(&self) -> usize {
        self.start_addr() + size_of::<ListNode>()
    }

    pub fn get_value(&self) -> usize {
        self.value
    }

    pub fn set_value(&mut self, value: usize) {
        self.value = value;
    }

    pub fn get_next(&mut self) -> Option<&mut ListNode> {
        self.next.as_deref_mut()
    }

    pub fn set_next(&mut self, next: Option<&'static mut ListNode>) {
        self.next = next;
    }
}

/// Linked-list backed allocator.
///
/// NOTE: this currently behaves like a bump allocator; the linked-list
/// free-list logic will replace the body of `alloc`/`dealloc` later.
pub struct LinkedListAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
    allocations: usize,
}

impl LinkedListAllocator {
    pub const fn empty() -> Self {
        LinkedListAllocator {
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

unsafe impl GlobalAlloc for Locked<LinkedListAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut alloc = self.lock();

        let alloc_start = align_up(alloc.next, layout.align());
        let alloc_end = match alloc_start.checked_add(layout.size()) {
            Some(end) => end,
            None => return ptr::null_mut(),
        };

        if alloc_end > alloc.heap_end {
            ptr::null_mut()
        } else {
            alloc.next = alloc_end;
            alloc.allocations += 1;
            alloc_start as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        let mut alloc = self.lock();
        alloc.allocations -= 1;
        if alloc.allocations == 0 {
            alloc.next = alloc.heap_start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static mut NODE1: ListNode = ListNode::new(42);
    static mut NODE2: ListNode = ListNode::new(84);

    #[test_case]
    fn test_list_node() {
        // Safety: this test is single-threaded and the static muts are only
        // accessed here.
        let (node1, node2) = unsafe {
            (
                &mut *core::ptr::addr_of_mut!(NODE1),
                &mut *core::ptr::addr_of_mut!(NODE2),
            )
        };

        // Test value access
        node1.set_next(Some(node2));
        assert_eq!(node1.get_value(), 42);

        // Test next pointer
        let next_value = node1.get_next().unwrap().get_value();
        assert_eq!(next_value, 84);

        // Test address calculations
        let node1_start = node1.start_addr();
        let node1_end = node1.end_addr();
        assert_eq!(node1_end - node1_start, size_of::<ListNode>());
    }
}
