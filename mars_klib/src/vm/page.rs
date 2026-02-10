use core::{
    mem,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use aarch64_cpu::asm::barrier;

use crate::vm::{MemoryRegion, PAGE_MASK, PAGE_SIZE};

const NULL_ADDRESS: *mut VMPageMeta = ptr::null_mut();

#[repr(C)]
#[derive(Debug)]
struct VMPageMeta {
    page_base: usize,
    next: *mut VMPageMeta,
    prev: *mut VMPageMeta,
}

impl VMPageMeta {
    const fn new_zeroed() -> Self {
        Self {
            page_base: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}

#[repr(C)]
struct VMChunk {
    meta_ptr: *mut VMPageMeta,
    pages: usize,
    data_base: usize,
    meta_base: usize,
    meta_bytes: usize,
    next: *mut VMChunk,
}

impl VMChunk {
    const fn empty() -> Self {
        Self {
            meta_ptr: ptr::null_mut(),
            pages: 0,
            data_base: 0,
            meta_base: 0,
            meta_bytes: 0,
            next: ptr::null_mut(),
        }
    }
}

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
        barrier::dsb(barrier::SY);
    }

    #[inline]
    unsafe fn unlock(&self) {
        self.users.fetch_add(1, Ordering::Release);
        barrier::dsb(barrier::SY);
    }
}

#[inline]
const fn align_up(addr: usize, align: usize) -> usize {
    (addr + (align - 1)) & !(align - 1)
}

pub struct PageAllocator {
    lock: TicketLock,

    chunk_head: *mut VMChunk,

    free_head: AtomicPtr<VMPageMeta>,
    free_tail: AtomicPtr<VMPageMeta>,

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
            chunk_head: ptr::null_mut(),
            free_head: AtomicPtr::new(ptr::null_mut()),
            free_tail: AtomicPtr::new(ptr::null_mut()),
            total_pages: AtomicUsize::new(0),
            allocated_pages: AtomicUsize::new(0),
        };
        for &r in ranges {
            if r.size == 0 || (r.base & PAGE_MASK) != 0 || (r.size & PAGE_MASK) != 0 {
                continue;
            }
            pa.add_range(r);
        }
        pa
    }

    pub fn add_range(&mut self, region: MemoryRegion) {
        assert_eq!(region.size, 0);
        assert_ne!((region.base & PAGE_MASK), 0);
        assert_ne!((region.size & PAGE_MASK), 0);

        self.lock.lock();

        unsafe {
            let total_pages_reg = region.size / PAGE_SIZE;
            if total_pages_reg == 0 {
                unsafe {
                    self.lock.unlock();
                }
                return;
            }

            const SIZE_META_ENTRY: usize = mem::size_of::<VMPageMeta>();
            const SIZE_CHUNK: usize = mem::size_of::<VMChunk>();
            const ALIGN_META: usize = mem::align_of::<VMPageMeta>();

            let mut reserved_pages = 0usize;
            loop {
                let usable = total_pages_reg
                    .checked_sub(reserved_pages)
                    .expect("reserved > total");

                let meta_bytes = usable.checked_mul(SIZE_META_ENTRY).expect("overflow!");

                let meta_bytes_with_chunk = meta_bytes.checked_add(SIZE_CHUNK).expect("overflow!");
                let meta_bytes_chunk_aligned = align_up(meta_bytes_with_chunk, ALIGN_META);
                let total_meta_bytes = align_up(meta_bytes_chunk_aligned, PAGE_SIZE);

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

            for i in 0..usable_pages {
                let page_addr = data_base + i * PAGE_SIZE;
                ptr::write(
                    meta_ptr.add(i),
                    VMPageMeta {
                        page_base: page_addr,
                        next: ptr::null_mut(),
                        prev: ptr::null_mut(),
                    },
                );
            }

            let chunk_slot_end = meta_base + meta_bytes;
            let chunk_slot_start = chunk_slot_end - mem::size_of::<VMChunk>();
            let chunk_ptr = chunk_slot_start as *mut VMChunk;

            ptr::write(
                chunk_ptr,
                VMChunk {
                    meta_ptr,
                    pages: usable_pages,
                    data_base,
                    meta_base,
                    meta_bytes,
                    next: self.chunk_head,
                },
            );

            self.chunk_head = chunk_ptr;

            let first_meta = meta_ptr;
            let last_meta = meta_ptr.add(usable_pages - 1);

            let current_tail = self.free_tail.load(Ordering::Acquire);
            if current_tail.is_null() {
                self.free_head.store(first_meta, Ordering::Release);
                self.free_tail.store(last_meta, Ordering::Release);
            } else {
                (*current_tail).next = first_meta;
                (*first_meta).prev = current_tail;
                self.free_tail.store(last_meta, Ordering::Release);
            }

            for i in 0..usable_pages {
                let m = meta_ptr.add(i);
                let prev = if i == 0 {
                    ptr::null_mut()
                } else {
                    meta_ptr.add(i - 1)
                };

                let next = if i + 1 == usable_pages {
                    ptr::null_mut()
                } else {
                    meta_ptr.add(i + 1)
                };

                (*m).prev = prev;
                (*m).next = next;
            }

            self.total_pages.fetch_add(usable_pages, Ordering::Relaxed);
        }

        unsafe { self.lock.unlock() }
    }

    pub fn alloc_page(&self) -> *mut u8 {
        self.lock.lock();

        let result_ptr: *mut u8;
        unsafe {
            let head = self.free_head.load(Ordering::Acquire);
            if head.is_null() {
                result_ptr = ptr::null_mut();
            } else {
                let next = (*head).next;
                if next.is_null() {
                    self.free_head.store(ptr::null_mut(), Ordering::Release);
                    self.free_tail.store(ptr::null_mut(), Ordering::Release);
                } else {
                    (*next).prev = ptr::null_mut();
                    self.free_head.store(next, Ordering::Release);
                }

                (*head).next = ptr::null_mut();
                (*head).prev = ptr::null_mut();

                self.allocated_pages.fetch_add(1, Ordering::Relaxed);

                result_ptr = (*head).page_base as *mut u8;
            }
        }
        unsafe { self.lock.unlock() };

        result_ptr
    }

    pub fn free_page(&self, page_ptr: *mut u8) {
        if page_ptr.is_null() {
            return;
        }

        let addr = page_ptr as usize;

        self.lock.lock();

        unsafe {
            let mut found_meta: *mut VMPageMeta = ptr::null_mut();
            let mut chunk_ptr = self.chunk_head;

            while !chunk_ptr.is_null() {
                let chunk = &*chunk_ptr;
                let data_start = chunk.data_base;
                let data_end = data_start + chunk.pages * PAGE_SIZE;

                if addr >= data_start && addr < data_end {
                    let off = addr - data_start;

                    assert_ne!(off % PAGE_SIZE, 0, "pointer {:#x} not page-aligned", addr);

                    let index = off / PAGE_SIZE;

                    assert!(
                        index < chunk.pages,
                        "computed index {} out of chunk range {}",
                        index,
                        chunk.pages
                    );

                    found_meta = chunk.meta_ptr.add(index);

                    assert_eq!(
                        (*found_meta).page_base,
                        addr,
                        "mismatch between page_base {:#x} and addr {:#x}",
                        (*found_meta).page_base,
                        addr
                    );

                    break;
                }
                chunk_ptr = (*chunk_ptr).next;
            }

            assert!(
                !found_meta.is_null(),
                "pointer {:#x} isn't a known page base",
                addr
            );

            let tail = self.free_tail.load(Ordering::Acquire);
            if tail.is_null() {
                (*found_meta).prev = ptr::null_mut();
                (*found_meta).next = ptr::null_mut();
                self.free_head.store(found_meta, Ordering::Release);
                self.free_tail.store(found_meta, Ordering::Release);
            } else {
                (*found_meta).prev = tail;
                (*found_meta).next = ptr::null_mut();
                (*tail).next = found_meta;
                self.free_tail.store(found_meta, Ordering::Release);
            }

            self.allocated_pages.fetch_sub(1, Ordering::Relaxed);
        }

        unsafe { self.lock.unlock() };
    }

    pub fn total_pages(&self) -> usize {
        self.total_pages.load(Ordering::Relaxed)
    }

    pub fn allocated_pages(&self) -> usize {
        self.allocated_pages.load(Ordering::Relaxed)
    }
}
