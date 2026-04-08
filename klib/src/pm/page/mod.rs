use core::{
    cell::UnsafeCell,
    mem,
    ptr::{self},
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use super::super::{
    sync::TicketLock,
    vm::{MemoryRegion, PAGE_MASK, PAGE_SIZE, align_up},
};

pub mod table_allocator;

const MAX_ORDER: usize = 11;
const FREE_FLAG: u8 = 1 << 7;

#[repr(C)]
#[derive(Debug)]
struct FreeBlock {
    next: *mut FreeBlock,
    prev: *mut FreeBlock,
    zone: *mut Zone,
    page_index: usize,
}

#[repr(C)]
struct Zone {
    meta_array: *mut u8,
    data_base: usize,
    total_pages: usize,
    next: *mut Zone,
}

#[repr(C)]
#[derive(Debug)]
pub struct PageAllocator {
    lock: TicketLock,
    zone_head: *mut Zone,
    free_area: UnsafeCell<[*mut FreeBlock; MAX_ORDER]>,
    total_pages: AtomicUsize,
    allocated_pages: AtomicUsize,
}

unsafe impl Send for PageAllocator {}
unsafe impl Sync for PageAllocator {}

impl PageAllocator {
    /// SAFETY: must be page-aligned, non-overlapping, and usable memory
    pub unsafe fn init(ranges: &[MemoryRegion]) -> Self {
        let mut pa = Self {
            lock: TicketLock::new(),
            zone_head: ptr::null_mut(),
            free_area: UnsafeCell::new([ptr::null_mut(); MAX_ORDER]),
            total_pages: AtomicUsize::new(0),
            allocated_pages: AtomicUsize::new(0),
        };
        for &r in ranges {
            if r.size == 0 || (r.base & PAGE_MASK) != 0 || (r.size & PAGE_MASK) != 0 {
                panic!("region {:?} not page aligned", r);
            }
            pa.add_range(r);
        }
        pa
    }

    pub fn add_range(&mut self, region: MemoryRegion) {
        assert_ne!(region.size, 0);

        self.lock.lock();

        unsafe {
            let total_pages_reg = region.size / PAGE_SIZE;
            if total_pages_reg == 0 {
                self.lock.unlock();
                return;
            }

            let mut reserved_pages = 0usize;
            loop {
                let usable = total_pages_reg
                    .checked_sub(reserved_pages)
                    .expect("reserved > total");

                let meta_bytes = usable;
                let needed_bytes = meta_bytes.checked_add(size_of::<Zone>()).unwrap();
                let needed_reserved = (needed_bytes + PAGE_SIZE - 1) / PAGE_SIZE;

                if needed_reserved == reserved_pages {
                    break;
                }
                reserved_pages = needed_reserved;
            }

            let usable_pages = total_pages_reg - reserved_pages;
            assert!(usable_pages > 0);

            let meta_base = region.base;
            let reserved_bytes = reserved_pages * PAGE_SIZE;
            let data_base = region.base + reserved_bytes;

            let meta_ptr = meta_base as *mut u8;
            let zone_slot_start = meta_base + reserved_bytes - size_of::<Zone>();
            let zone_ptr = zone_slot_start as *mut Zone;

            ptr::write_bytes(meta_ptr, 0, usable_pages);

            ptr::write(
                zone_ptr,
                Zone {
                    meta_array: meta_ptr,
                    data_base,
                    total_pages: usable_pages,
                    next: self.zone_head,
                },
            );

            self.zone_head = zone_ptr;
            self.total_pages.fetch_add(usable_pages, Ordering::Relaxed);

            let free_area = &mut *self.free_area.get();
            let mut current_index = 0;

            while current_index < usable_pages {
                let remaining = usable_pages - current_index;

                // power of two
                let align_order = if current_index == 0 {
                    MAX_ORDER - 1
                } else {
                    current_index.trailing_zeros() as usize
                };

                let size_order = remaining.ilog2() as usize;
                let order = align_order.min(size_order).min(MAX_ORDER - 1);

                *meta_ptr.add(current_index) = (order as u8) | FREE_FLAG;

                let block = (data_base + current_index * PAGE_SIZE) as *mut FreeBlock;
                (*block).zone = zone_ptr;
                (*block).page_index = current_index;

                let old_head = free_area[order];
                (*block).next = old_head;
                (*block).prev = ptr::null_mut();

                if !old_head.is_null() {
                    (*old_head).prev = block;
                }

                free_area[order] = block;
                current_index += (1 << order);
            }
        }

        self.lock.unlock()
    }

    pub fn alloc_page(&self) -> *mut u8 {
        self.alloc_pages(0)
    }

    // allocate 2^order pages (contiguous)
    pub fn alloc_pages(&self, target_order: usize) -> *mut u8 {
        if target_order >= MAX_ORDER {
            return ptr::null_mut();
        }

        self.lock.lock();

        unsafe {
            let free_area = &mut *self.free_area.get();

            for current_order in target_order..MAX_ORDER {
                let head = free_area[current_order];

                if !head.is_null() {
                    let next = (*head).next;
                    free_area[current_order] = next;

                    if !next.is_null() {
                        (*next).prev = ptr::null_mut();
                    }

                    let zone = (*head).zone;
                    let page_i = (*head).page_index;
                    let meta_array = (*zone).meta_array;

                    *meta_array.add(page_i) = target_order as u8;

                    // split
                    let mut split_order = current_order;
                    while split_order > target_order {
                        split_order -= 1;

                        let split_i = page_i + (1 << split_order);
                        let split_meta = meta_array.add(split_i);

                        *split_meta = (split_order as u8) | FREE_FLAG;

                        let block = ((*zone).data_base + split_i * PAGE_SIZE) as *mut FreeBlock;
                        (*block).zone = zone;
                        (*block).page_index = split_i;

                        let old_head = free_area[split_order];
                        (*block).next = old_head;
                        (*block).prev = ptr::null_mut();

                        if !old_head.is_null() {
                            (*old_head).prev = block;
                        }

                        free_area[split_order] = block;
                    }

                    self.allocated_pages
                        .fetch_add(1 << target_order, Ordering::Relaxed);
                    self.lock.unlock();

                    return ((*zone).data_base + page_i * PAGE_SIZE) as *mut u8;
                }
            }
        }

        self.lock.unlock();
        ptr::null_mut()
    }

    pub fn free_pages(&self, page_ptr: *mut u8) {
        if page_ptr.is_null() {
            return;
        }

        let addr = page_ptr as usize;
        self.lock.lock();

        unsafe {
            let mut zone_ptr = self.zone_head;
            let mut handled = false;

            while !zone_ptr.is_null() {
                let zone = &mut *zone_ptr;
                let data_start = zone.data_base;
                let data_end = data_start + zone.total_pages * PAGE_SIZE;

                if addr >= data_start && addr < data_end {
                    let off = addr - data_start;

                    assert_eq!(off % PAGE_SIZE, 0, "pointer {:#x} not page-aligned", addr);

                    let index = off / PAGE_SIZE;
                    let meta_val = *zone.meta_array.add(index);

                    assert_eq!(meta_val & FREE_FLAG, 0, "double free for {:#x}", addr);

                    let order = (meta_val & !FREE_FLAG) as usize;
                    let pages = 1 << order;

                    self.allocated_pages.fetch_sub(pages, Ordering::Relaxed);
                    self.free_block_in_zone(zone_ptr, index, order);

                    handled = true;
                    break;
                }
                zone_ptr = zone.next;
            }

            assert!(handled, "pointer {:#x} isn't owned by this allocator", addr);
        }

        self.lock.unlock();
    }

    unsafe fn free_block_in_zone(
        &self,
        zone_ptr: *mut Zone,
        mut page_index: usize,
        mut order: usize,
    ) {
        let zone = unsafe { &mut *zone_ptr };
        let free_area = unsafe { &mut *self.free_area.get() };

        while order < MAX_ORDER - 1 {
            // the magic buddy XOR
            let index = page_index ^ (1 << order);

            if index >= zone.total_pages {
                break;
            }

            let meta_val = unsafe { *zone.meta_array.add(index) };

            if (meta_val & FREE_FLAG) == 0 || (meta_val & !FREE_FLAG) != order as u8 {
                break;
            }

            let block = (zone.data_base + index * PAGE_SIZE) as *mut FreeBlock;
            let next = unsafe { (*block).next };
            let prev = unsafe { (*block).prev };

            if !prev.is_null() {
                unsafe { (*prev).next = next };
            } else {
                free_area[order] = next;
            }

            if !next.is_null() {
                unsafe { (*next).prev = prev };
            }

            // merge
            page_index &= !(1 << order);
            order += 1;
        }

        unsafe { *zone.meta_array.add(page_index) = (order as u8) | FREE_FLAG };

        let block = (zone.data_base + page_index * PAGE_SIZE) as *mut FreeBlock;
        unsafe { (*block).zone = zone_ptr };
        unsafe { (*block).page_index = page_index };

        let old_head = free_area[order];
        unsafe { (*block).next = old_head };
        unsafe { (*block).prev = ptr::null_mut() };

        if !old_head.is_null() {
            unsafe { (*old_head).prev = block };
        }

        free_area[order] = block;
    }

    pub fn total_pages(&self) -> usize {
        self.total_pages.load(Ordering::Relaxed)
    }

    pub fn allocated_pages(&self) -> usize {
        self.allocated_pages.load(Ordering::Relaxed)
    }
}
