use core::ptr::NonNull;
use hal::paging::{GEOMETRY, PageTable};
use klib::{
    pm::page::mapper::TableAllocator,
    vm::{VmError, align_up},
};
use uefi::{
    Status,
    boot::{self, MemoryType, PAGE_SIZE as UEFI_PS},
};

#[derive(Debug)]
pub struct UefiTableAlloc;

pub fn allocate_aligned_pages(size: usize, alignment: usize) -> Result<NonNull<u8>, Status> {
    if size == 0 || size % UEFI_PS != 0 || alignment < UEFI_PS || !alignment.is_power_of_two() {
        return Err(Status::INVALID_PARAMETER);
    }
    let pages = size / UEFI_PS;
    let extra = alignment / UEFI_PS - 1;
    let count = pages.checked_add(extra).ok_or(Status::OUT_OF_RESOURCES)?;
    let allocation =
        boot::allocate_pages(boot::AllocateType::AnyPages, MemoryType::LOADER_CODE, count)
            .map_err(|error| error.status())?;
    let address = allocation.as_ptr() as usize;
    let aligned = align_up(address, alignment);
    let prefix = (aligned - address) / UEFI_PS;
    let suffix = extra - prefix;
    if suffix != 0 {
        let tail = NonNull::new((aligned + size) as *mut u8).unwrap();
        unsafe { boot::free_pages(tail, suffix) }.expect("failed to release alignment suffix");
    }
    if prefix != 0 {
        unsafe { boot::free_pages(allocation, prefix) }
            .expect("failed to release alignment prefix");
    }
    Ok(NonNull::new(aligned as *mut u8).unwrap())
}

impl TableAllocator for UefiTableAlloc {
    fn alloc_table(&self) -> Result<PageTable, VmError> {
        let layout = GEOMETRY.table_layout();
        let allocation = allocate_aligned_pages(layout.size(), layout.align())
            .map_err(|_| VmError::OutOfMemory)?;
        let table = unsafe { PageTable::from_ptr(allocation.as_ptr()) };
        unsafe { table.zero() };
        Ok(table)
    }

    fn free_table(&self, table: PageTable) {
        let pointer = NonNull::new(table.as_ptr()).unwrap();
        unsafe { boot::free_pages(pointer, GEOMETRY.table_layout().size() / UEFI_PS) }
            .expect("failed to release bootstrap table");
    }
}
