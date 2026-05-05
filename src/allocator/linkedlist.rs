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

    /// Removes and returns the `next` link, leaving `self.next` empty.
    pub fn take_next(&mut self) -> Option<&'static mut ListNode> {
        self.next.take()
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

    /// Inherent allocation routine. Returns null on failure.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let alloc_start = align_up(self.next, layout.align());
        let alloc_end = match alloc_start.checked_add(layout.size()) {
            Some(end) => end,
            None => return ptr::null_mut(),
        };

        if alloc_end > self.heap_end {
            ptr::null_mut()
        } else {
            self.next = alloc_end;
            self.allocations += 1;
            alloc_start as *mut u8
        }
    }

    /// Inherent deallocation routine.
    ///
    /// # Safety
    /// `ptr` must have been returned from a previous call to [`alloc`] with
    /// the same `layout` and must not have been freed already.
    pub unsafe fn dealloc(&mut self, _ptr: *mut u8, _layout: Layout) {
        self.allocations -= 1;
        if self.allocations == 0 {
            self.next = self.heap_start;
        }
    }


    // Removes the last node from the linked list and returns its value. Returns `None` if the list
    // is empty.
    pub fn pop_value(&mut self) -> Option<usize> {
        let mut current = self.heap_start as *mut ListNode;
        let mut prev: Option<&mut ListNode> = None;

        while let Some(next) = unsafe { (*current).get_next() } {
            prev = Some(unsafe { &mut *current });
            current = next as *mut ListNode;
        }

        if let Some(prev_node) = prev {
            let value = unsafe { (*current).get_value() };
            prev_node.set_next(None);
            Some(value)
        } else {
            None
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
