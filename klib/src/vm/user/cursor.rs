use alloc::vec::Vec;
use core::{mem::ManuallyDrop, ops::Range};
use hal::paging::{
    Entry, GEOMETRY, Level, MappingOptions, PageTable, supports_mapping, validate_mapping,
};

use super::{AddressSpace, PAGE_DESCRIPTORS, PtState, Status, new_meta};
use crate::{
    pm::page::mapper::{AddressTranslator, TableAllocator},
    sync::{RwLockReadGuard, RwLockWriteGuard},
    vm::{PAGE_MASK, PAGE_SIZE, VmError},
};

#[derive(Copy, Clone)]
enum Change {
    Reserve(MappingOptions),
    Map {
        physical: usize,
        virtual_start: usize,
        options: MappingOptions,
    },
    Clear,
    Protect(MappingOptions),
}

/// exclusive access to a bounded PT subtree, with its ancestors pinned
pub struct Cursor<'a> {
    space: &'a AddressSpace<'a>,
    range: Range<usize>,
    table: PageTable,
    level: Level,
    physical: usize,
    reads: Vec<RwLockReadGuard<'static, PtState>>,
    guard: ManuallyDrop<RwLockWriteGuard<'static, PtState>>,
    translator: &'a dyn AddressTranslator,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(
        space: &'a AddressSpace<'a>,
        range: Range<usize>,
        table: PageTable,
        level: Level,
        physical: usize,
        reads: Vec<RwLockReadGuard<'static, PtState>>,
        guard: RwLockWriteGuard<'static, PtState>,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self {
            space,
            range,
            table,
            level,
            physical,
            reads,
            guard: ManuallyDrop::new(guard),
            translator,
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn query(&self, address: usize) -> Result<Status, VmError> {
        if !self.range.contains(&address) {
            return Err(VmError::InvalidAddress);
        }
        let mut table = self.table;
        let mut level = self.level;
        loop {
            let index = level.index(address);
            match unsafe { table.read(index, level) } {
                Entry::Invalid => return Ok(self.metadata(table, index)),
                Entry::Mapping {
                    physical_address,
                    options,
                } => {
                    return Ok(Status::Mapped {
                        pa: physical_address + (address & (level.coverage() - 1)),
                        options,
                    });
                }
                Entry::Table { physical_address } => {
                    table = self.table_at(physical_address);
                    level = level.child().expect("child table at leaf level");
                }
            }
        }
    }

    pub fn reserve(&mut self, options: MappingOptions) -> Result<(), VmError> {
        validate_mapping(0, Level::LEAF, options)?;
        self.change(Change::Reserve(options))
    }

    /// marks software only state. resident mapping must be installed with `map`
    pub fn mark(&mut self, status: Status) -> Result<(), VmError> {
        match status {
            Status::PrivateAnonymous(options) => self.reserve(options),
            Status::Invalid => self.unmap(),
            Status::Mapped { .. } => Err(VmError::InvalidSize),
        }
    }

    /// map the borrowed backing memory, committing existing reservations.
    /// backing physical memory ownership is borrowed (as opposed to moving)
    ///
    /// safety:
    /// backing memory must remain live and permit these accesses for the mapping's lifetime
    /// the mapping must not invalidate existing rust references or executable code
    pub unsafe fn map(&mut self, physical: usize, options: MappingOptions) -> Result<(), VmError> {
        if physical & PAGE_MASK != 0 {
            return Err(VmError::InvalidAlignment);
        }
        let last = physical
            .checked_add(self.range.end - self.range.start - PAGE_SIZE)
            .ok_or(VmError::InvalidAddress)?;
        validate_mapping(physical, Level::LEAF, options)?;
        validate_mapping(last, Level::LEAF, options)?;
        self.change(Change::Map {
            physical,
            virtual_start: self.range.start,
            options,
        })
    }

    pub fn unmap(&mut self) -> Result<(), VmError> {
        self.change(Change::Clear)
    }

    pub fn protect(&mut self, options: MappingOptions) -> Result<(), VmError> {
        validate_mapping(0, Level::LEAF, options)?;
        self.change(Change::Protect(options))
    }

    fn change(&mut self, change: Change) -> Result<(), VmError> {
        let range = self.range.clone();
        self.preflight(self.table, self.level, range.clone(), change)?;
        // prep can split nodes or allocate sidecars, but preserve state.
        // all possibly fallible allocation will happen before the mapping is commited.
        // perchance.
        self.prepare(self.table, self.level, range.clone(), change)?;
        self.commit(self.table, self.level, range.clone(), change);
        if matches!(change, Change::Clear) {
            self.reap(self.table, self.level, range);
        }
        Ok(())
    }

    fn table_at(&self, physical: usize) -> PageTable {
        unsafe { PageTable::from_ptr(self.translator.phys_to_dmap(physical)) }
    }

    fn metadata(&self, table: PageTable, index: usize) -> Status {
        let physical = self.translator.dmap_to_phys(table.as_ptr());
        if physical == self.physical {
            self.guard
                .meta
                .as_ref()
                .map_or(Status::Invalid, |metadata| metadata[index].status)
        } else {
            let guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.read();
            guard
                .meta
                .as_ref()
                .map_or(Status::Invalid, |metadata| metadata[index].status)
        }
    }

    fn ensure_metadata(&mut self, table: PageTable, level: Level) -> Result<(), VmError> {
        let physical = self.translator.dmap_to_phys(table.as_ptr());
        if physical == self.physical {
            if self.guard.meta.is_none() {
                self.guard.meta = Some(new_meta(level, Status::Invalid)?);
            }
        } else {
            let mut guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.write();
            if guard.meta.is_none() {
                guard.meta = Some(new_meta(level, Status::Invalid)?);
            }
        }
        Ok(())
    }

    fn set_metadata(&mut self, table: PageTable, index: usize, status: Status) {
        let physical = self.translator.dmap_to_phys(table.as_ptr());
        if physical == self.physical {
            if let Some(metadata) = &mut self.guard.meta {
                metadata[index].status = status;
            } else {
                assert_eq!(status, Status::Invalid);
            }
        } else {
            let mut guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.write();
            if let Some(metadata) = &mut guard.meta {
                metadata[index].status = status;
            } else {
                assert_eq!(status, Status::Invalid);
            }
        }
    }

    fn span_end(address: usize, level: Level, end: usize) -> usize {
        address + (level.coverage() - (address & (level.coverage() - 1))).min(end - address)
    }

    fn preflight(
        &self,
        table: PageTable,
        level: Level,
        range: Range<usize>,
        change: Change,
    ) -> Result<(), VmError> {
        let mut address = range.start;
        while address < range.end {
            let end = Self::span_end(address, level, range.end);
            let index = level.index(address);
            match unsafe { table.read(index, level) } {
                Entry::Table { physical_address } => self.preflight(
                    self.table_at(physical_address),
                    level.child().unwrap(),
                    address..end,
                    change,
                )?,
                Entry::Mapping {
                    physical_address,
                    options,
                } => match change {
                    Change::Reserve(_) => return Err(VmError::Overlap),
                    Change::Map {
                        physical,
                        virtual_start,
                        options: requested,
                    } => {
                        let mapped = physical_address + (address & (level.coverage() - 1));
                        if mapped != physical + (address - virtual_start) || options != requested {
                            return Err(VmError::Overlap);
                        }
                    }
                    _ => {}
                },
                Entry::Invalid => {
                    if matches!(change, Change::Protect(_))
                        && self.metadata(table, index) == Status::Invalid
                    {
                        return Err(VmError::NotMapped);
                    }
                }
            }
            address = end;
        }
        Ok(())
    }

    fn target(
        &self,
        table: PageTable,
        level: Level,
        address: usize,
        end: usize,
        change: Change,
    ) -> bool {
        let entry = unsafe { table.read(level.index(address), level) };
        if matches!(entry, Entry::Table { .. }) {
            return false;
        }
        if matches!(change, Change::Clear)
            && entry == Entry::Invalid
            && self.metadata(table, level.index(address)) == Status::Invalid
        {
            return true;
        }
        if address & (level.coverage() - 1) != 0 || end - address != level.coverage() {
            return level == Level::LEAF;
        }
        if let Change::Map {
            physical,
            virtual_start,
            ..
        } = change
        {
            return supports_mapping(level)
                && (physical + (address - virtual_start)) & (level.coverage() - 1) == 0;
        }
        true
    }

    fn prepare(
        &mut self,
        table: PageTable,
        level: Level,
        range: Range<usize>,
        change: Change,
    ) -> Result<(), VmError> {
        let mut address = range.start;
        while address < range.end {
            let end = Self::span_end(address, level, range.end);
            let index = level.index(address);
            if self.target(table, level, address, end, change) {
                if matches!(change, Change::Reserve(_)) {
                    self.ensure_metadata(table, level)?;
                }
            } else {
                let child = match unsafe { table.read(index, level) } {
                    Entry::Table { physical_address } => self.table_at(physical_address),
                    entry => self.split(table, level, index, entry)?,
                };
                self.prepare(child, level.child().unwrap(), address..end, change)?;
            }
            address = end;
        }
        Ok(())
    }

    fn split(
        &mut self,
        table: PageTable,
        level: Level,
        index: usize,
        entry: Entry,
    ) -> Result<PageTable, VmError> {
        let child_level = level.child().ok_or(VmError::InvalidSize)?;
        let status = self.metadata(table, index);
        let child = self.space.allocator.alloc_table()?;
        let physical = self.translator.dmap_to_phys(child.as_ptr());
        if status != Status::Invalid {
            let metadata = match new_meta(child_level, status) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.space.allocator.free_table(child);
                    return Err(error);
                }
            };
            PAGE_DESCRIPTORS
                .get_page_descriptor(physical)
                .lock
                .write()
                .meta = Some(metadata);
        }
        if let Entry::Mapping {
            physical_address,
            options,
        } = entry
        {
            for child_index in 0..GEOMETRY.entries_at(child_level) {
                let entry = Entry::Mapping {
                    physical_address: physical_address + child_index * child_level.coverage(),
                    options,
                };
                if let Err(error) = unsafe { child.write(child_index, child_level, entry) } {
                    self.space.allocator.free_table(child);
                    return Err(error.into());
                }
            }
        }
        if let Err(error) = unsafe {
            table.write(
                index,
                level,
                Entry::Table {
                    physical_address: physical,
                },
            )
        } {
            self.space.allocator.free_table(child);
            return Err(error.into());
        }
        self.set_metadata(table, index, Status::Invalid);
        Ok(child)
    }

    fn commit(&mut self, table: PageTable, level: Level, range: Range<usize>, change: Change) {
        let mut address = range.start;
        while address < range.end {
            let end = Self::span_end(address, level, range.end);
            let index = level.index(address);
            let old = unsafe { table.read(index, level) };
            if let Entry::Table { physical_address } = old {
                self.commit(
                    self.table_at(physical_address),
                    level.child().unwrap(),
                    address..end,
                    change,
                );
            } else {
                let (entry, status) = match change {
                    Change::Reserve(options) => (Entry::Invalid, Status::PrivateAnonymous(options)),
                    Change::Map {
                        physical,
                        virtual_start,
                        options,
                    } => (
                        Entry::Mapping {
                            physical_address: physical + (address - virtual_start),
                            options,
                        },
                        Status::Invalid,
                    ),
                    Change::Clear => (Entry::Invalid, Status::Invalid),
                    Change::Protect(options) => match old {
                        Entry::Mapping {
                            physical_address, ..
                        } => (
                            Entry::Mapping {
                                physical_address,
                                options,
                            },
                            Status::Invalid,
                        ),
                        Entry::Invalid => (Entry::Invalid, Status::PrivateAnonymous(options)),
                        Entry::Table { .. } => unreachable!(),
                    },
                };
                if entry != old {
                    unsafe { table.write(index, level, entry) }
                        .expect("prepared descriptor is valid");
                }
                self.set_metadata(table, index, status);
            }
            address = end;
        }
    }

    fn empty(&self, table: PageTable, level: Level) -> bool {
        let physical = self.translator.dmap_to_phys(table.as_ptr());
        let guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.read();
        for index in 0..GEOMETRY.entries_at(level) {
            if unsafe { table.read(index, level) } != Entry::Invalid
                || guard
                    .meta
                    .as_ref()
                    .is_some_and(|metadata| metadata[index].status != Status::Invalid)
            {
                return false;
            }
        }
        true
    }

    // that of which was sowed
    fn reap(&mut self, table: PageTable, level: Level, range: Range<usize>) {
        let mut address = range.start;
        while address < range.end {
            let end = Self::span_end(address, level, range.end);
            let index = level.index(address);
            if let Entry::Table { physical_address } = unsafe { table.read(index, level) } {
                let child = self.table_at(physical_address);
                let child_level = level.child().unwrap();
                self.reap(child, child_level, address..end);
                if self.empty(child, child_level) {
                    unsafe { table.write(index, level, Entry::Invalid) }
                        .expect("invalid descriptor");
                    self.space.allocator.free_table(child);
                }
            }
            address = end;
        }
    }
}

impl Drop for Cursor<'_> {
    fn drop(&mut self) {
        // release the covering node before ancestors to prevent its reclamation
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        while let Some(guard) = self.reads.pop() {
            drop(guard);
        }
    }
}
