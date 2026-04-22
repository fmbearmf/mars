use core::{
    alloc::{GlobalAlloc, Layout},
    cell::UnsafeCell,
    mem, ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

use derivative::Derivative;

use crate::{pm::page::mapper::AddressTranslator, vm::page_allocator::DmapPageAllocator};

use super::{
    super::{pm::page::PageAllocator, sync::TicketLock},
    PAGE_SIZE, VmError, align_down, align_up,
    page_allocator::PhysicalPageAllocator,
};

const fn build_class_sizes() -> [usize; 9] {
    [
        usize::pow(2, 3),
        usize::pow(2, 4),
        usize::pow(2, 5),
        usize::pow(2, 6),
        usize::pow(2, 7),
        usize::pow(2, 8),
        usize::pow(2, 9),
        usize::pow(2, 10),
        usize::pow(2, 11),
    ]
}

const CLASS_SIZES: [usize; 9] = build_class_sizes();

#[inline]
fn size_class_index(size: usize) -> Option<usize> {
    for (i, &s) in CLASS_SIZES.iter().enumerate() {
        if size <= s {
            return Some(i);
        }
    }
    None
}

#[repr(C)]
struct Header {
    next: *mut Header,
    prev: *mut Header,
    free_list: *mut u8,
    free_count: u16,
    size_class_i: u16,
}

struct Cache {
    _size_class: usize,
    plist: *mut Header,
}

const fn build_caches() -> [Cache; 9] {
    [
        Cache {
            _size_class: 8,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 16,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 32,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 64,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 128,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 256,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 512,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 1024,
            plist: ptr::null_mut(),
        },
        Cache {
            _size_class: 2048,
            plist: ptr::null_mut(),
        },
    ]
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct SlabAllocator<'a> {
    page_alloc: AtomicPtr<PageAllocator<'static>>,
    used_bytes: AtomicUsize,
    caches: UnsafeCell<[Cache; 9]>,
    lock: TicketLock,
    is_dmap: AtomicBool,

    #[derivative(Debug = "ignore")]
    translator: &'a dyn AddressTranslator,
}

unsafe impl Send for SlabAllocator<'_> {}
unsafe impl Sync for SlabAllocator<'_> {}

impl<'a> SlabAllocator<'a> {
    pub const fn new(
        page_alloc: &'static PageAllocator<'a>,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self {
            page_alloc: AtomicPtr::new(page_alloc as *const _ as *mut _),
            used_bytes: AtomicUsize::new(0),
            caches: UnsafeCell::new(build_caches()),
            lock: TicketLock::new(),
            is_dmap: AtomicBool::new(false),
            translator,
        }
    }

    pub fn page_alloc(&self) -> &'static PageAllocator<'static> {
        let ptr = self.page_alloc.load(Ordering::Acquire);
        assert!(!ptr.is_null(), "slab allocation used before init()");
        unsafe { &*ptr }
    }

    pub unsafe fn page_alloc_mut(&self) -> &'static mut PageAllocator<'static> {
        let ptr = self.page_alloc.load(Ordering::Acquire);
        assert!(!ptr.is_null(), "slab allocation used before init()");
        unsafe { &mut *ptr }
    }

    unsafe fn alloc_impl(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(layout.align());
        let page_alloc = self.page_alloc();

        let ptr = if let Some(i) = size_class_index(size) {
            self.lock.lock();
            let caches = unsafe { &mut *self.caches.get() };
            let cache = &mut caches[i];

            if !cache.plist.is_null() {
                let header = unsafe { &mut *cache.plist };
                let obj = header.free_list;
                header.free_list = unsafe { *(obj as *mut *mut u8) };
                header.free_count -= 1;

                if header.free_count == 0 {
                    cache.plist = header.next;
                    if !header.next.is_null() {
                        unsafe { (*header.next).prev = ptr::null_mut() };
                    }

                    header.next = ptr::null_mut();
                    header.prev = ptr::null_mut();
                }

                self.lock.unlock();
                obj
            } else {
                self.lock.unlock();

                // new page
                let page: usize = if self.is_dmap.load(Ordering::Relaxed) {
                    page_alloc.alloc_dmap_page().expect("dmap page alloc fail")
                } else {
                    page_alloc.alloc_phys_page().expect("phys page alloc fail")
                };
                let page = page as *mut u8;

                if page.is_null() {
                    panic!("OOM");
                    return ptr::null_mut();
                }

                let size_class = CLASS_SIZES[i];
                let header_size = mem::size_of::<Header>();
                let start_off = align_up(header_size, size_class);
                let cap = (PAGE_SIZE - start_off) / size_class;

                let header = page as *mut Header;
                let first_obj = unsafe { page.add(start_off) };
                let mut prev = first_obj;

                for i in 1..cap {
                    let next = unsafe { page.add(start_off + i * size_class) };
                    unsafe { *(prev as *mut *mut u8) = next };
                    prev = next;
                }

                unsafe { *(prev as *mut *mut u8) = ptr::null_mut() };

                unsafe { (*header).free_list = *(first_obj as *mut *mut u8) };
                unsafe { (*header).free_count = cap as u16 - 1 };
                unsafe { (*header).size_class_i = i as u16 };

                if cap > 1 {
                    self.lock.lock();
                    let caches = unsafe { &mut *self.caches.get() };
                    let cache = &mut caches[i];

                    unsafe { (*header).next = cache.plist };
                    unsafe { (*header).prev = ptr::null_mut() };

                    if !cache.plist.is_null() {
                        unsafe { (*cache.plist).prev = header };
                    }

                    cache.plist = header;
                    self.lock.unlock();
                } else {
                    unsafe { (*header).next = ptr::null_mut() };
                    unsafe { (*header).prev = ptr::null_mut() };
                }

                first_obj
            }
        } else {
            let total_size = layout.size().max(layout.align());
            let req = align_up(total_size, PAGE_SIZE);

            let req_pages = req / PAGE_SIZE;

            let order = req_pages.next_power_of_two().trailing_zeros() as usize;

            if layout.align() <= PAGE_SIZE {
                page_alloc.alloc_pages(order)
            } else {
                let align = layout.align();
                let ptr = page_alloc.alloc_pages(order + 1);
                if ptr.is_null() {
                    return ptr::null_mut();
                };

                let aligned = align_up(ptr as usize + mem::size_of::<usize>(), align);
                unsafe { (aligned as *mut usize).sub(1).write(ptr as usize) };
                aligned as *mut u8
            }
        };

        if !ptr.is_null() {
            self.used_bytes.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn free_impl(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }

        let page_alloc = self.page_alloc();
        let size = layout.size().max(layout.align());

        if size > 2048 {
            if layout.align() <= PAGE_SIZE {
                page_alloc.free_pages(ptr);
            } else {
                let ptr = unsafe { (ptr as *mut usize).sub(1).read() as *mut u8 };
                page_alloc.free_pages(ptr);
            }
        } else {
            let page_start = align_down(ptr as usize, PAGE_SIZE);
            let header = page_start as *mut Header;

            self.lock.lock();

            let i = unsafe { (*header).size_class_i as usize };
            let caches = unsafe { &mut *self.caches.get() };
            let cache = &mut caches[i];

            unsafe { *(ptr as *mut *mut u8) = (*header).free_list };
            unsafe { (*header).free_list = ptr };

            let was_full = unsafe { (*header).free_count == 0 };
            unsafe { (*header).free_count += 1 };

            let size_class = CLASS_SIZES[i];
            let cap = (PAGE_SIZE - align_up(mem::size_of::<Header>(), size_class)) / size_class;

            if unsafe { (*header).free_count == cap as u16 } {
                if !was_full {
                    let next = unsafe { (*header).next };
                    let prev = unsafe { (*header).prev };
                    if !prev.is_null() {
                        unsafe { (*prev).next = next };
                    } else {
                        cache.plist = next;
                    }
                    if !next.is_null() {
                        unsafe { (*next).prev = prev };
                    }
                }

                self.lock.unlock();
                page_alloc.free_pages(page_start as *mut u8);
            } else {
                if was_full {
                    unsafe { (*header).next = cache.plist };
                    if !cache.plist.is_null() {
                        unsafe { (*cache.plist).prev = header };
                    }
                    cache.plist = header;
                }
                self.lock.unlock();
            }
        }

        self.used_bytes.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    pub unsafe fn transition_dmap(&self) {
        self.lock.lock();

        debug_assert_eq!(
            self.is_dmap.load(Ordering::Relaxed),
            false,
            "`transition_dmap` called twice"
        );

        unsafe { self.page_alloc_mut().transition_dmap() };

        // null ptrs need to stay null
        let ptr_to_dmap = |ptr: *mut u8| -> *mut u8 {
            if ptr.is_null() {
                ptr
            } else {
                self.translator.phys_to_dmap(ptr as _) as _
            }
        };

        let old_pa = self.page_alloc.load(Ordering::Acquire);
        let new_pa = self.translator.phys_to_dmap(old_pa as _) as *mut PageAllocator<'static>;
        self.page_alloc.store(new_pa, Ordering::Release);

        let caches = unsafe { &mut *self.caches.get() };
        for cache in caches.iter_mut() {
            cache.plist = ptr_to_dmap(cache.plist as _) as *mut Header;

            let mut current_hdr_ptr = cache.plist;
            while !current_hdr_ptr.is_null() {
                let header = unsafe { &mut *current_hdr_ptr };

                header.next = ptr_to_dmap(header.next as _) as *mut Header;
                header.prev = ptr_to_dmap(header.prev as _) as *mut Header;

                let old_free_list = header.free_list;
                header.free_list = ptr_to_dmap(old_free_list as _);

                let mut current_obj = header.free_list;
                while !current_obj.is_null() {
                    let next_ptr = current_obj as *mut *mut u8;

                    // the physical address of the next object
                    let phys_next = unsafe { *next_ptr };
                    let dmap_next = ptr_to_dmap(phys_next as _);

                    unsafe { *next_ptr = dmap_next };

                    current_obj = dmap_next;
                }

                current_hdr_ptr = header.next;
            }
        }

        self.is_dmap.store(true, Ordering::SeqCst);

        self.lock.unlock();
    }

    pub fn free_page(&self, va: usize) {
        self.page_alloc().free_pages(va as *mut u8);
    }

    pub fn capacity(&self) -> usize {
        self.page_alloc().total_pages() * PAGE_SIZE
    }

    /// # of bytes allocated for heap objects. excludes overhead.
    pub fn heap_usage(&self) -> usize {
        self.used_bytes.load(Ordering::Relaxed)
    }

    /// pages currently allocated by the underlying page allocator. includes slab overhead and non-slab pages.
    pub fn page_usage(&self) -> usize {
        self.page_alloc().allocated_pages() * PAGE_SIZE
    }
}

impl PhysicalPageAllocator for SlabAllocator<'_> {
    fn alloc_phys_page(&self) -> Result<usize, VmError> {
        self.page_alloc().alloc_phys_page()
    }

    fn free_phys_page(&self, pa: usize) {
        self.page_alloc().free_phys_page(pa);
    }
}

impl DmapPageAllocator for SlabAllocator<'_> {
    fn alloc_dmap_page(&self) -> Result<usize, VmError> {
        self.page_alloc().alloc_dmap_page()
    }
    fn free_dmap_page(&self, pa: usize) {
        self.page_alloc().free_dmap_page(pa)
    }
}

unsafe impl GlobalAlloc for SlabAllocator<'_> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.alloc_impl(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.free_impl(ptr, layout) }
    }
}
