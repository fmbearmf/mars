use core::alloc::GlobalAlloc;
use hal::paging::{GEOMETRY, PageTable};

use crate::{pm::page::mapper::TableAllocator, vm::KALLOCATOR};

use super::VmError;

pub trait PhysicalPageAllocator: Send + Sync {
    fn alloc_phys_page(&self) -> Result<usize, VmError>;
    fn free_phys_page(&self, pa: usize);
}

pub trait DmapPageAllocator {
    fn alloc_dmap_page(&self) -> Result<usize, VmError>;
    fn free_dmap_page(&self, pa: usize);
}

#[derive(Debug)]
pub struct KernelPTAllocator;

impl TableAllocator for KernelPTAllocator {
    fn alloc_table(&self) -> Result<PageTable, VmError> {
        let layout = GEOMETRY.table_layout();
        let pointer = unsafe { KALLOCATOR.alloc(layout) };
        if pointer.is_null() {
            return Err(VmError::OutOfMemory);
        }
        let table = unsafe { PageTable::from_ptr(pointer) };
        unsafe { table.zero() };
        Ok(table)
    }

    fn free_table(&self, table: PageTable) {
        unsafe { KALLOCATOR.dealloc(table.as_ptr(), GEOMETRY.table_layout()) };
    }
}
