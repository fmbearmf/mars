use core::{
    mem,
    ptr::{self},
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use crate::vm::{MemoryRegion, PAGE_MASK, PAGE_SIZE, align_up};
use aarch64_cpu::asm::barrier;

pub mod table_allocator;

const NULL_ADDRESS: *mut VMPageMeta = ptr::null_mut();
const MAX_ORDER: usize = 11;

#[repr(C)]
#[derive(Debug)]
struct VMPageMeta {
    next: *mut VMPageMeta,
    prev: *mut VMPageMeta,
    order: u32,
    is_free: u32,
}

#[repr(C)]
struct VMZone {
    meta_array: *mut VMPageMeta,
    data_base: usize,
    total_pages: usize,
    free_area: [*mut VMPageMeta; MAX_ORDER],
    next: *mut VMZone,
}

#[repr(align(64))]
#[derive(Debug)]
struct TicketLock {
    ticket: AtomicUsize,
    users: AtomicUsize,
}

impl TicketLock {
    const fn new() -> Self {
        Self {
            ticket: AtomicUsize::new(0),
            users: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn lock(&self) {
        let ticket = self.ticket.fetch_add(1, Ordering::Relaxed);
        while self.users.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn unlock(&self) {
        self.users.fetch_add(1, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct PageAllocator {
    lock: TicketLock,
    zone_head: *mut VMZone,
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

            const SIZE_META_ENTRY: usize = mem::size_of::<VMPageMeta>();
            const SIZE_ZONE: usize = mem::size_of::<VMZone>();
            const ALIGN_META: usize = mem::align_of::<VMPageMeta>();

            let mut reserved_pages = 0usize;
            loop {
                let usable = total_pages_reg
                    .checked_sub(reserved_pages)
                    .expect("reserved > total");

                let meta_bytes = usable.checked_mul(SIZE_META_ENTRY).unwrap();

                let meta_bytes_with_zone = meta_bytes.checked_add(SIZE_ZONE).unwrap();
                let meta_bytes_aligned = align_up(meta_bytes_with_zone, ALIGN_META);
                let total_meta_bytes = align_up(meta_bytes_aligned, PAGE_SIZE);

                let needed_reserved = total_meta_bytes / PAGE_SIZE;
                if needed_reserved == reserved_pages {
                    break;
                }

                assert!(needed_reserved <= total_pages_reg);
                reserved_pages = needed_reserved;
            }

            let usable_pages = total_pages_reg - reserved_pages;
            assert!(usable_pages > 0);

            let meta_base = region.base;
            let meta_bytes = reserved_pages * PAGE_SIZE;
            let data_base = region.base + meta_bytes;

            let meta_ptr = meta_base as *mut VMPageMeta;
            let zone_slot_start = meta_base + meta_bytes - SIZE_ZONE;
            let zone_ptr = zone_slot_start as *mut VMZone;

            for i in 0..usable_pages {
                ptr::write(
                    meta_ptr.add(i),
                    VMPageMeta {
                        next: ptr::null_mut(),
                        prev: ptr::null_mut(),
                        order: 0,
                        is_free: 0,
                    },
                );
            }

            ptr::write(
                zone_ptr,
                VMZone {
                    meta_array: meta_ptr,
                    data_base,
                    total_pages: usable_pages,
                    free_area: [ptr::null_mut(); MAX_ORDER],
                    next: self.zone_head,
                },
            );

            self.zone_head = zone_ptr;
            self.total_pages.fetch_add(usable_pages, Ordering::Relaxed);

            let mut current_index = 0;
            while current_index < usable_pages {
                let mut order = 0;
                while order < MAX_ORDER - 1 {
                    let next_order = order + 1;
                    let next_pages = 1 << next_order;

                    if current_index & next_pages == 0 && current_index + next_pages <= usable_pages
                    {
                        order = next_order;
                    } else {
                        break;
                    }
                }

                let meta = meta_ptr.add(current_index);
                (*meta).order = order as u32;
                (*meta).is_free = 1;

                let old_head = (*zone_ptr).free_area[order];
                (*meta).next = old_head;
                (*meta).prev = ptr::null_mut();

                if !old_head.is_null() {
                    (*old_head).prev = meta;
                }

                (*zone_ptr).free_area[order] = meta;

                current_index += 1 << order;
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
            let mut zone_ptr = self.zone_head;
            while !zone_ptr.is_null() {
                let zone = &mut *zone_ptr;

                for current_order in target_order..MAX_ORDER {
                    let head = zone.free_area[current_order];
                    if !head.is_null() {
                        let next = (*head).next;

                        zone.free_area[current_order] = next;

                        if !next.is_null() {
                            (*next).prev = ptr::null_mut();
                        }

                        (*head).is_free = 0;
                        (*head).next = ptr::null_mut();
                        (*head).prev = ptr::null_mut();

                        let page_index = head.offset_from(zone.meta_array) as usize;

                        let mut split_order = current_order;
                        while split_order > target_order {
                            split_order -= 1;

                            let index = page_index + (1 << split_order);
                            let meta = zone.meta_array.add(index);

                            (*meta).order = split_order as u32;
                            (*meta).is_free = 1;

                            let old_head = zone.free_area[split_order];

                            (*meta).next = old_head;
                            (*meta).prev = ptr::null_mut();

                            if !old_head.is_null() {
                                (*old_head).prev = meta;
                            }

                            zone.free_area[split_order] = meta;
                        }

                        (*head).order = target_order as u32;

                        let allocated = 1 << target_order;
                        self.allocated_pages.fetch_add(allocated, Ordering::Relaxed);

                        self.lock.unlock();
                        return (zone.data_base + page_index * PAGE_SIZE) as *mut u8;
                    }
                }
                zone_ptr = zone.next;
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
                    let meta = zone.meta_array.add(index);

                    assert_eq!((*meta).is_free, 0, "double free for {:#x}", addr);

                    let order = (*meta).order as usize;
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
        zone_ptr: *mut VMZone,
        mut page_index: usize,
        mut order: usize,
    ) {
        let zone = &mut *zone_ptr;
        while order < MAX_ORDER - 1 {
            // the magic buddy XOR
            let index = page_index ^ (1 << order);

            if index >= zone.total_pages {
                break;
            }

            let meta = zone.meta_array.add(index);

            if (*meta).is_free == 0 || (*meta).order != order as u32 {
                break;
            }

            let next = (*meta).next;
            let prev = (*meta).prev;

            if !prev.is_null() {
                (*prev).next = next;
            } else {
                zone.free_area[order] = next;
            }

            if !next.is_null() {
                (*next).prev = prev;
            }

            (*meta).is_free = 0;
            (*meta).next = ptr::null_mut();
            (*meta).prev = ptr::null_mut();

            // merge
            page_index = page_index & !(1 << order);
            order += 1;
        }

        let meta = zone.meta_array.add(page_index);
        (*meta).order = order as u32;
        (*meta).is_free = 1;

        let old_head = zone.free_area[order];
        (*meta).next = old_head;
        (*meta).prev = ptr::null_mut();
        if !old_head.is_null() {
            (*old_head).prev = meta;
        }
        zone.free_area[order] = meta;
    }

    pub fn total_pages(&self) -> usize {
        self.total_pages.load(Ordering::Relaxed)
    }

    pub fn allocated_pages(&self) -> usize {
        self.allocated_pages.load(Ordering::Relaxed)
    }
}
