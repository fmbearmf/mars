extern crate alloc;

use core::fmt::Debug;
use core::ptr::NonNull;
use core::range::Range;

use alloc::vec::Vec;

use super::super::{TABLE_ENTRIES, TTable, page_allocator::PhysicalPageAllocator};
use super::{PAGE_DESCRIPTORS, allocator::UserAllocator, cursor::Cursor, entry_cover, entry_index};
use crate::pm::page::mapper::TableAllocator;

pub struct AddressSpace<'a, A: TableAllocator, P: PhysicalPageAllocator> {
    pub root: NonNull<TTable<TABLE_ENTRIES>>,
    pub max_level: usize,
    pub allocator: UserAllocator<'a, A, P>,
}

impl<'a, A: TableAllocator, P: PhysicalPageAllocator> Debug for AddressSpace<'a, A, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AddressSpace")
            .field("root", &self.root)
            .field("max_level", &self.max_level)
            .finish()
    }
}

impl<'a, A: TableAllocator, P: PhysicalPageAllocator> AddressSpace<'a, A, P> {
    pub fn new(max_level: usize, table_allocator: &'a A, page_allocator: &'a P) -> Self {
        let tracked_alloc = UserAllocator(table_allocator, page_allocator);
        let root = tracked_alloc.alloc_table();

        Self {
            root,
            max_level,
            allocator: tracked_alloc,
        }
    }

    unsafe fn drop_table(&mut self, table_ptr: NonNull<TTable<TABLE_ENTRIES>>, level: usize) {
        let table = unsafe { table_ptr.as_ref() };

        if level > 0 {
            for &pte in &table.entries {
                if pte.is_valid() && pte.is_table() {
                    let child = pte.address();
                    let child_ptr = self.allocator.phys_to_virt(child);

                    if let Some(child_nn) = NonNull::new(child_ptr) {
                        unsafe { self.drop_table(child_nn, level - 1) };
                    }
                } else {
                    let pa = pte.address();
                }
            }
        }

        self.allocator.free_table(table_ptr);
    }

    pub fn lock(&self, range: Range<usize>) -> Cursor<'_, A, P> {
        let mut current_pa = self.allocator.virt_to_phys(self.root.as_ptr());
        let mut current_level = self.max_level;
        let mut current_base_va = 0;
        let mut read_guards = Vec::new();

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

                let table_ptr = self.allocator.phys_to_virt(current_pa);
                let pte = unsafe { (*table_ptr).entries[start_i] };

                if pte.is_valid() && pte.is_table() {
                    let child = pte.address();
                    read_guards.push((current_pa, guard));

                    current_pa = child;
                    current_base_va += start_i * entry_cover(current_level);
                    current_level -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(current_pa as usize);
        let write_guard = desc.lock.write();

        Cursor {
            addr_space: self,
            range,
            read_guards,
            write_guard: Some((current_pa, write_guard)),
            covering_level: current_level,
            covering_pa: current_pa,
        }
    }
}

impl<'a, A: TableAllocator, P: PhysicalPageAllocator> Drop for AddressSpace<'a, A, P> {
    fn drop(&mut self) {
        unsafe {
            self.drop_table(self.root, self.max_level);
        }
    }
}
