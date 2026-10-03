use core::{alloc::GlobalAlloc, range::Range};

use alloc::{boxed::Box, vec::Vec};
use hal::paging::{
    Access, AddressSpaceContext, GEOMETRY, MappingOptions, MemoryType as MappingMemoryType,
    PageTable,
};
use klib::{
    allocator_support::KernelAddressTranslator,
    pm::page::{
        PageAllocator,
        mapper::{AddressTranslator, MemoryProvider, TableAllocator, map_page},
    },
    rangekeeper,
    sync::RwLock,
    vm::{
        KALLOCATOR, PAGE_SIZE, VmError, align_down, align_up,
        user::{PageDescriptor, PtState},
    },
};
use log::trace;
use uefi::boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS};

struct BootTempAllocator<'a>(&'a (dyn GlobalAlloc + Send + Sync));

impl TableAllocator for BootTempAllocator<'_> {
    fn alloc_table(&self) -> Result<PageTable, VmError> {
        let layout = GEOMETRY.table_layout();
        let pointer = unsafe { self.0.alloc(layout) };
        if pointer.is_null() {
            return Err(VmError::OutOfMemory);
        }
        let table = unsafe { PageTable::from_ptr(pointer) };
        unsafe { table.zero() };
        Ok(table)
    }
    fn free_table(&self, table: PageTable) {
        unsafe { self.0.dealloc(table.as_ptr(), GEOMETRY.table_layout()) };
    }
}

struct IdentityTranslator;
impl AddressTranslator for IdentityTranslator {
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        virt as usize
    }
    fn phys_to_dmap(&self, physical: usize) -> *mut u8 {
        physical as *mut u8
    }
}

pub fn create_page_descriptors() -> (Box<[PageDescriptor]>, Range<usize>) {
    let allocator = KALLOCATOR.page_alloc();
    let start = KernelAddressTranslator.dmap_to_phys(allocator.min_address() as *mut u8);
    let end = KernelAddressTranslator.dmap_to_phys(allocator.max_address() as *mut u8);
    let pages = (end - start) / PAGE_SIZE;
    let mut descriptors = Box::<[PageDescriptor]>::new_uninit_slice(pages);
    for slot in descriptors.iter_mut() {
        slot.write(PageDescriptor {
            lock: RwLock::new(PtState { meta: None }),
        });
    }
    (unsafe { descriptors.assume_init() }, Range { start, end })
}

/// give the allocator an interim physical region while the identity map is active
pub fn populate_alloc_stage0() {
    let allocator = unsafe { KALLOCATOR.page_alloc_mut() };
    rangekeeper::for_each_range(|range| {
        log::info!(
            "rangekeeper range: {:#010x}..{:#010x}",
            range.start,
            range.end
        );
    });
    let range = rangekeeper::largest_range().expect("no usable boot memory");
    assert!(
        range.end - range.start >= 4 * PAGE_SIZE,
        "insufficient boot memory"
    );
    trace!("page allocator push stage0: {range:#x?}");
    allocator.add_range(&range);
}

/// add the remaining usable regions after the allocator has entered the dmap
pub fn populate_alloc_stage1() {
    let allocator = unsafe { KALLOCATOR.page_alloc_mut() };
    rangekeeper::for_each_range(|range| flush(allocator, range.start, range.end));
}

fn flush(allocator: &mut PageAllocator, start: usize, end: usize) {
    let range = Range { start, end };
    if let Some(overlap) = allocator.overlapping_range(&range) {
        if overlap.start > start {
            add_subrange(allocator, start, overlap.start);
        }
        if overlap.end < end {
            add_subrange(allocator, overlap.end, end);
        }
    } else {
        add_subrange(allocator, start, end);
    }
}

fn add_subrange(allocator: &mut PageAllocator, start: usize, end: usize) {
    let start = KernelAddressTranslator.phys_to_dmap(align_up(start, PAGE_SIZE)) as usize;
    let end = KernelAddressTranslator.phys_to_dmap(align_down(end, PAGE_SIZE)) as usize;
    if end > start && end - start >= 4 * PAGE_SIZE {
        allocator.add_range(&Range { start, end });
    }
}

fn descriptor_options(descriptor: &MemoryDescriptor) -> MappingOptions {
    let device = matches!(
        descriptor.ty,
        MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE
    );
    let memory_type = if device {
        MappingMemoryType::Device
    } else if descriptor.att.contains(MemoryAttribute::WRITE_BACK) {
        MappingMemoryType::Normal
    } else if descriptor.att.contains(MemoryAttribute::WRITE_THROUGH) {
        MappingMemoryType::WriteThrough
    } else {
        MappingMemoryType::Uncached
    };
    let writable = matches!(
        descriptor.ty,
        MemoryType::CONVENTIONAL
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::BOOT_SERVICES_CODE
            | MemoryType::MMIO
            | MemoryType::MMIO_PORT_SPACE
            | MemoryType::RUNTIME_SERVICES_DATA
            | MemoryType::ACPI_NON_VOLATILE
    ) && !descriptor.att.contains(MemoryAttribute::READ_ONLY);
    // dmap never provides an executable alias, including firmware and kernel code
    MappingOptions {
        access: if writable {
            Access::KernelReadWrite
        } else {
            Access::KernelReadOnly
        },
        executable: false,
        memory_type,
    }
}

/// safety:
/// called on the sole running cpu ('bsp') while boot page tables and the identity allocator are alive.
/// the returned context owns its cloned tables and must outlive every cpu using it
pub unsafe fn switch_to_new_page_tables<'a>(
    descriptors: impl Iterator<Item = &'a MemoryDescriptor>,
    allocator: &(dyn GlobalAlloc + Send + Sync),
) -> Result<AddressSpaceContext, VmError> {
    let tables = BootTempAllocator(allocator);
    let memory = MemoryProvider::new(&tables, &IdentityTranslator);
    let context = unsafe { AddressSpaceContext::current().clone(&memory) }?;
    if let Err(error) = unsafe { map_all_dmap(context, descriptors, &tables, &memory) } {
        unsafe { context.destroy(&memory) };
        return Err(error);
    }
    unsafe { context.activate() };
    Ok(context)
}

unsafe fn map_all_dmap<'a>(
    context: AddressSpaceContext,
    descriptors: impl Iterator<Item = &'a MemoryDescriptor>,
    tables: &BootTempAllocator<'_>,
    memory: &MemoryProvider<'_>,
) -> Result<(), VmError> {
    let mut ranges = Vec::new();
    let mut boundaries = Vec::new();
    for descriptor in descriptors {
        if matches!(descriptor.ty, MemoryType::RESERVED | MemoryType::UNUSABLE) {
            continue;
        }
        let start = usize::try_from(descriptor.phys_start).map_err(|_| VmError::InvalidAddress)?;
        let size = usize::try_from(descriptor.page_count)
            .map_err(|_| VmError::InvalidSize)?
            .checked_mul(UEFI_PS)
            .ok_or(VmError::InvalidSize)?;
        if size == 0 {
            continue;
        }
        let end = start.checked_add(size).ok_or(VmError::InvalidAddress)?;
        let end = end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(VmError::InvalidAddress)?;
        let start = align_down(start, PAGE_SIZE);
        let end = align_down(end, PAGE_SIZE);
        ranges.push((start, end, descriptor_options(descriptor)));
        boundaries.extend([start, end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for span in boundaries.windows(2) {
        let mut options: Option<MappingOptions> = None;
        for &(start, end, candidate) in &ranges {
            if start <= span[0] && span[1] <= end {
                if let Some(current) = &mut options {
                    if current.memory_type != candidate.memory_type {
                        return Err(VmError::Unsupported);
                    }
                    if !candidate.access.writable() {
                        current.access = Access::KernelReadOnly;
                    }
                } else {
                    options = Some(candidate);
                }
            }
        }
        let Some(options) = options else { continue };
        for physical in (span[0]..span[1]).step_by(PAGE_SIZE) {
            let virtual_address = KernelAddressTranslator.phys_to_dmap(physical) as usize;
            let root = context.table_for(virtual_address, memory)?;
            unsafe {
                map_page(
                    root,
                    physical,
                    virtual_address,
                    options,
                    tables,
                    &IdentityTranslator,
                )
            }?;
        }
    }
    Ok(())
}
