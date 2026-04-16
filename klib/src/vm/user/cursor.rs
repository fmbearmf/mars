extern crate alloc;

use super::super::{
    PAGE_SIZE, TABLE_ENTRIES, TTENATIVE, TTable, page_allocator::PhysicalPageAllocator,
};
use super::{PAGE_DESCRIPTORS, PtState, PteMeta, Status, address_space::AddressSpace, entry_index};
use crate::pm::page::mapper::AddressTranslator;
use crate::{
    pm::page::mapper::{TableAllocator, map_page, unmap_page},
    sync::{RwLockReadGuard, RwLockWriteGuard},
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::{boxed::Box, vec::Vec};
use core::range::Range;

pub struct Cursor<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> {
    pub addr_space: &'a AddressSpace<'a, T, P, A>,
    pub range: Range<usize>,

    pub read_guards: Vec<(usize, RwLockReadGuard<'static, PtState>)>,
    pub write_guard: Option<(usize, RwLockWriteGuard<'static, PtState>)>,

    pub covering_pa: usize,
    pub covering_level: usize,

    pub _phantom: core::marker::PhantomData<A>,
}

impl<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> Cursor<'a, T, P, A> {
    pub fn map(
        &mut self,
        target_pa_start: u64,
        perm: AccessPermission,
        share: Shareability,
        uxn: bool,
        pxn: bool,
        attr_index: u64,
    ) {
        let mut current_pa = target_pa_start;

        for va in (self.range.start..self.range.end).step_by(PAGE_SIZE) {
            unsafe {
                map_page::<_, A>(
                    &mut *self.addr_space.root.as_ptr(),
                    current_pa as usize,
                    va,
                    perm,
                    share,
                    uxn,
                    pxn,
                    attr_index,
                    &self.addr_space.allocator,
                );
            }

            self.update_leaf_meta(
                va,
                Status::Mapped {
                    pa: current_pa as usize,
                    perm,
                },
            );
            current_pa += PAGE_SIZE as u64;
        }
    }

    pub fn unmap(&mut self) {
        for va in (self.range.start..self.range.end).step_by(PAGE_SIZE) {
            unsafe {
                unmap_page::<_, A>(
                    &mut *self.addr_space.root.as_ptr(),
                    va,
                    &self.addr_space.allocator,
                );
            }

            self.update_leaf_meta(va, Status::Invalid);
        }
    }

    pub fn mark(&mut self, status: Status) {
        for va in (self.range.start..self.range.end).step_by(PAGE_SIZE) {
            // some marking occurs without an existing mapping in the page tables.
            // therefore the hierarchy must be ensured to exist.
            self.ensure_leaf_table(va);
            self.update_leaf_meta(va, status);
        }
    }

    pub fn query(&self, va: usize) -> Status {
        if !self.range.contains(&va) {
            return Status::Invalid;
        }

        let mut current_pa = self.covering_pa;
        let mut current_lvl = self.covering_level;

        loop {
            let i = entry_index(va, current_lvl);
            let table_ptr: *mut TTable<TABLE_ENTRIES> = A::phys_to_dmap(current_pa as u64);
            let pte = unsafe { (*table_ptr).entries[i] };

            if !pte.is_valid() {
                return Status::Invalid;
            }

            if current_lvl == 0 {
                let desc = PAGE_DESCRIPTORS.get_page_descriptor(current_pa as _);
                let state = desc.lock.read();
                return state
                    .meta
                    .as_ref()
                    .map(|m| m[i].status)
                    .unwrap_or(Status::Invalid);
            }

            current_pa = pte.address() as _;
            current_lvl -= 1;
        }
    }

    fn ensure_leaf_table(&self, va: usize) {
        let mut current_pa = self.covering_pa;
        let mut current_lvl = self.covering_level;

        while current_lvl > 0 {
            let i = entry_index(va, current_lvl);
            let table_ptr: *mut TTable<TABLE_ENTRIES> = A::phys_to_dmap(current_pa as _);
            let mut pte = unsafe { (*table_ptr).entries[i] };

            if !pte.is_valid() {
                let new_table = self.addr_space.allocator.alloc_table();
                let new_pa = A::dmap_to_phys(new_table.as_ptr());

                pte = TTENATIVE::new_table(new_pa);
                unsafe { (*table_ptr).entries[i] = pte };
            }

            current_pa = pte.address() as _;
            current_lvl -= 1;
        }
    }

    fn update_leaf_meta(&self, va: usize, status: Status) {
        let mut current_pa = self.covering_pa;
        let mut current_lvl = self.covering_level;

        while current_lvl > 0 {
            let i = entry_index(va, current_lvl);
            let table_ptr: *mut TTable<TABLE_ENTRIES> = A::phys_to_dmap(current_pa as _);
            let pte = unsafe { (*table_ptr).entries[i] };

            if pte.is_valid() && pte.is_table() {
                current_pa = pte.address() as _;
                current_lvl -= 1;
            } else {
                return;
            }
        }

        let i = entry_index(va, 0);
        let desc = PAGE_DESCRIPTORS.get_page_descriptor(current_pa as usize);
        let mut state = desc.lock.write();

        if state.meta.is_none() {
            state.meta = Some(Box::new([PteMeta::default(); TABLE_ENTRIES]));
        }

        if let Some(meta) = &mut state.meta {
            meta[i].status = status;
        }
    }
}

impl<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> Drop
    for Cursor<'a, T, P, A>
{
    fn drop(&mut self) {
        self.write_guard.take();

        while let Some(_) = self.read_guards.pop() {}
    }
}
