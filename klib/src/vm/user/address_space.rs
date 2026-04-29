use core::fmt::Debug;
use core::ptr::NonNull;
use core::range::Range;

use alloc::vec::Vec;

use super::super::{TABLE_ENTRIES, TTable, page_allocator::PhysicalPageAllocator};
use super::{PAGE_DESCRIPTORS, allocator::UserAllocator, cursor::Cursor, entry_cover, entry_index};
use crate::pm::page::mapper::{AddressTranslator, TableAllocator};

pub struct AddressSpace<'a> {
    pub root: NonNull<TTable<TABLE_ENTRIES>>,
    pub max_level: usize,
    pub allocator: UserAllocator<'a>,
    translator: &'a dyn AddressTranslator,
}

unsafe impl Sync for AddressSpace<'_> {}

unsafe impl Send for AddressSpace<'_> {}

impl Debug for AddressSpace<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AddressSpace")
            .field("root", &self.root)
            .field("max_level", &self.max_level)
            .finish()
    }
}

impl<'a> AddressSpace<'a> {
    pub fn new(
        max_level: Option<usize>,
        table_allocator: &'a dyn TableAllocator,
        page_allocator: &'a dyn PhysicalPageAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        let tracked_alloc = UserAllocator(table_allocator, page_allocator, translator);
        let root = tracked_alloc.alloc_table();

        Self {
            root,
            max_level: max_level.unwrap_or(3),
            allocator: tracked_alloc,
            translator,
        }
    }

    pub const unsafe fn new_dangling(
        max_level: Option<usize>,
        table_allocator: &'a dyn TableAllocator,
        page_allocator: &'a dyn PhysicalPageAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self::from_root_table(
            max_level,
            NonNull::dangling(),
            table_allocator,
            page_allocator,
            translator,
        )
    }

    pub const fn from_root_table(
        max_level: Option<usize>,
        root: NonNull<TTable<TABLE_ENTRIES>>,
        table_allocator: &'a dyn TableAllocator,
        page_allocator: &'a dyn PhysicalPageAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        let tracked_alloc = UserAllocator(table_allocator, page_allocator, translator);

        Self {
            root,
            max_level: max_level.unwrap_or(3),
            allocator: tracked_alloc,
            translator,
        }
    }
}

impl AddressSpace<'_> {
    /// initialize a dangling address space. this may only be called once, when there are no other references to self.
    pub fn init(&self) {
        assert_eq!(self.root, NonNull::dangling(), "init called twice!");
        let table = self.allocator.alloc_table();
        let root_ref = (&self.root) as *const _ as *mut NonNull<TTable<TABLE_ENTRIES>>;

        unsafe {
            root_ref.write(table);
        }
    }

    /// initialize a dangling address space. this may only be called once, when there are no other references to self.
    pub fn init_from_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        assert_eq!(self.root, NonNull::dangling(), "init called twice!");
        let root_ref = (&self.root) as *const _ as *mut NonNull<TTable<TABLE_ENTRIES>>;

        unsafe {
            root_ref.write(table);
        }
    }

    unsafe fn drop_table(&mut self, table_ptr: NonNull<TTable<TABLE_ENTRIES>>, level: usize) {
        let table = unsafe { table_ptr.as_ref() };

        debug_assert_ne!(
            self.root,
            NonNull::dangling(),
            "drop_table called in invalid state"
        );

        for (_, pte) in table.entries.iter().enumerate() {
            if pte.is_valid() {
                if level > 0 && pte.is_table() {
                    let child = pte.address();
                    let child_ptr =
                        self.translator.phys_to_dmap(child as _) as *mut TTable<TABLE_ENTRIES>;

                    if let Some(chil_nn) = NonNull::new(child_ptr) {
                        unsafe { self.drop_table(chil_nn, level - 1) };
                    }
                } else {
                    let pa = pte.address() as usize;
                    self.allocator.free_phys_page(pa);
                }
            }
        }
    }

    pub fn lock(&self, range: Range<usize>) -> Cursor<'_> {
        let mut current_pa = self.translator.dmap_to_phys(self.root.as_ptr() as _) as usize;
        let mut current_level = self.max_level;
        let mut current_base_va = 0;
        let mut read_guards = Vec::new();

        debug_assert_ne!(
            self.root,
            NonNull::dangling(),
            "lock called in invalid state"
        );

        loop {
            if current_level == 0 {
                break;
            }

            let start_i = entry_index(range.start, current_level);
            let end_i = entry_index(range.end.saturating_sub(1), current_level);

            // ie child pt page covers the entire range
            if start_i == end_i {
                let desc = PAGE_DESCRIPTORS.get_page_descriptor(current_pa as usize);
                let guard = desc.lock.read();

                let table_ptr =
                    self.translator.phys_to_dmap(current_pa as _) as *mut TTable<TABLE_ENTRIES>;
                let pte = unsafe { &(*table_ptr).entries[start_i] };

                if pte.is_valid() && pte.is_table() {
                    let child = pte.address();
                    read_guards.push((current_pa, guard));

                    current_pa = child as _;
                    current_base_va += start_i * entry_cover(current_level);
                    current_level -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(current_pa as _);
        let write_guard = desc.lock.write();

        Cursor {
            addr_space: self,
            range,
            read_guards,
            write_guard: Some((current_pa, write_guard)),
            covering_level: current_level,
            covering_pa: current_pa,

            translator: self.translator,
        }
    }
}

impl Drop for AddressSpace<'_> {
    fn drop(&mut self) {
        unsafe {
            self.drop_table(self.root, self.max_level);
        }
    }
}
