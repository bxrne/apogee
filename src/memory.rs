use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PhysFrame, Size4KiB,
    },
};

#[cfg(test)]
use x86_64::structures::paging::PageTableFlags;

use bootloader::bootinfo::{MemoryMap, MemoryRegion, MemoryRegionType};

/// Initialize a new OffsetPageTable.
///
/// # Safety
/// The caller must guarantee that the complete physical memory is mapped to
/// virtual memory at `physical_memory_offset`, and that this function is only
/// called once to avoid aliasing `&mut` references (undefined behaviour).
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    unsafe {
        let level_4_table = active_level_4_table(physical_memory_offset);
        OffsetPageTable::new(level_4_table, physical_memory_offset)
    }
}

/// Returns a mutable reference to the active level 4 page table.
///
/// # Safety
/// Same requirements as [`init`].
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    unsafe { &mut *page_table_ptr }
}

/// Creates an example mapping for the given page to frame `0xb8000`.
pub fn create_example_mapping(
    page: Page,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    use x86_64::structures::paging::PageTableFlags as Flags;

    let frame = PhysFrame::containing_address(PhysAddr::new(0xb8000));
    let flags = Flags::PRESENT | Flags::WRITABLE;

    let map_to_result = unsafe {
        // FIXME: this is not safe; it is done only for testing
        mapper.map_to(page, frame, flags, frame_allocator)
    };
    map_to_result.expect("map_to failed").flush();
}

/// A FrameAllocator that returns usable frames from the bootloader's memory map.
///
/// Frames are handed out in ascending address order and never reused, so
/// allocation is monotonic. The allocator keeps a cursor over the usable
/// regions instead of rescanning the memory map on every allocation, which
/// makes handing out frames O(1) amortized.
pub struct MonotonicFrameAllocator {
    memory_map: &'static MemoryMap,
    /// Position of the next candidate frame in the flattened sequence of
    /// usable frames, ordered by ascending address.
    cursor: FrameCursor,
}

/// Position of the next candidate frame in the flattened sequence of usable
/// frames, ordered by ascending address.
///
/// The cursor only ever moves forward, so frames are handed out in ascending
/// address order without ever rescanning the ones that were already handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameCursor {
    /// Index of the usable region the cursor is currently filling.
    region: usize,
    /// Offset of the next candidate frame within the current region.
    offset: usize,
}

impl FrameCursor {
    const fn new() -> Self {
        FrameCursor {
            region: 0,
            offset: 0,
        }
    }
}

impl MonotonicFrameAllocator {
    /// Create a FrameAllocator from the passed memory map.
    ///
    /// # Safety
    /// The caller must guarantee that all frames marked `USABLE` in the memory
    /// map are really unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        MonotonicFrameAllocator {
            memory_map,
            cursor: FrameCursor::new(),
        }
    }

    /// Returns the next usable frame in ascending address order, or `None`
    /// once the usable frames are exhausted.
    fn next_frame(&mut self) -> Option<PhysFrame> {
        let regions: &[MemoryRegion] = self.memory_map;
        let mut region = self.cursor.region;
        let mut offset = self.cursor.offset;

        loop {
            let mut selected: Option<&MemoryRegion> = None;
            let mut usable_ordinal = 0;
            for candidate in regions.iter() {
                if candidate.region_type != MemoryRegionType::Usable {
                    continue;
                }
                if usable_ordinal == region {
                    selected = Some(candidate);
                    break;
                }
                usable_ordinal += 1;
            }
            let Some(current) = selected else {
                return None;
            };

            let start = current.range.start_addr();
            let end = current.range.end_addr();
            let frame_count = ((end - start) / 4096) as usize;
            if offset < frame_count {
                let phys =
                    PhysFrame::containing_address(PhysAddr::new(start + (offset * 4096) as u64));
                self.cursor = FrameCursor {
                    region,
                    offset: offset + 1,
                };
                return Some(phys);
            }

            // The current region is exhausted; move on to the next usable one.
            region += 1;
            offset = 0;
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for MonotonicFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        self.next_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_page_containing_address() {
        let addr = VirtAddr::new(0x8000);
        let page: Page<Size4KiB> = Page::containing_address(addr);
        assert_eq!(page.start_address(), VirtAddr::new(0x8000));
    }

    #[test_case]
    fn test_page_range_inclusive() {
        let start: Page<Size4KiB> = Page::containing_address(VirtAddr::new(0x1000));
        let end: Page<Size4KiB> = Page::containing_address(VirtAddr::new(0x3000));

        let mut count = 0;
        for _ in Page::range_inclusive(start, end) {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test_case]
    fn test_phys_frame_containing_address() {
        let addr = PhysAddr::new(0xb8000);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(addr);
        assert_eq!(frame.start_address(), PhysAddr::new(0xb8000));
    }

    #[test_case]
    fn test_page_table_flags_present_writable() {
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        assert!(flags.contains(PageTableFlags::PRESENT));
        assert!(flags.contains(PageTableFlags::WRITABLE));
    }

    #[test_case]
    fn test_vga_frame_address() {
        let vga_addr = PhysAddr::new(0xb8000);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(vga_addr);
        assert_eq!(frame.start_address(), vga_addr);
    }
}
