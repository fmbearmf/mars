use alloc::vec::Vec;
use hal::paging::{
    AddressSpaceContext, Level, MappingOptions, MemoryType, PageTable, supports_mapping,
};
use klib::{
    pm::page::mapper::{AddressTranslator, map_block},
    vm::{PAGE_SIZE, VmError, align_down},
};
use uefi::{
    boot::{self, MemoryAttribute, MemoryType as UefiMemoryType},
    mem::memory_map::MemoryMap,
};

use crate::TABLE_ALLOC;

#[derive(Debug)]
pub struct UefiAddressTranslator;

// no translation needed
impl AddressTranslator for UefiAddressTranslator {
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        virt as _
    }
    fn phys_to_dmap(&self, phys: usize) -> *mut u8 {
        phys as _
    }
}

/// # safety
/// the unpublished root uses the uefi allocator and identity translation
pub unsafe fn map_identity(root: PageTable, uart_page: usize) -> Result<(), VmError> {
    let map = boot::memory_map(UefiMemoryType::LOADER_DATA).map_err(|_| VmError::OutOfMemory)?;
    let mut ranges = Vec::new();
    let mut boundaries = Vec::new();
    for descriptor in map.entries() {
        if matches!(
            descriptor.ty,
            UefiMemoryType::RESERVED | UefiMemoryType::UNUSABLE
        ) {
            continue;
        }
        let start = usize::try_from(descriptor.phys_start).map_err(|_| VmError::InvalidAddress)?;
        let bytes = usize::try_from(descriptor.page_count)
            .map_err(|_| VmError::InvalidSize)?
            .checked_mul(uefi::boot::PAGE_SIZE)
            .ok_or(VmError::InvalidSize)?;
        if bytes == 0 {
            continue;
        }
        let end = start.checked_add(bytes).ok_or(VmError::InvalidAddress)?;
        let end = end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(VmError::InvalidAddress)?;
        let start = align_down(start, PAGE_SIZE);
        let end = align_down(end, PAGE_SIZE);
        let device = matches!(
            descriptor.ty,
            UefiMemoryType::MMIO | UefiMemoryType::MMIO_PORT_SPACE
        );
        let executable = matches!(
            descriptor.ty,
            UefiMemoryType::LOADER_CODE
                | UefiMemoryType::BOOT_SERVICES_CODE
                | UefiMemoryType::RUNTIME_SERVICES_CODE
        );
        let memory_type = if device {
            MemoryType::Device
        } else if descriptor.att.contains(MemoryAttribute::WRITE_BACK) {
            MemoryType::Normal
        } else if descriptor.att.contains(MemoryAttribute::WRITE_THROUGH) {
            MemoryType::WriteThrough
        } else {
            MemoryType::Uncached
        };
        // firmware code allocations can also contain writable pe sections
        // these bootstrap aliases must be retired before entering application contexts
        let options = MappingOptions {
            executable,
            memory_type,
            ..MappingOptions::KERNEL_RW
        };
        ranges.push((start, end, options));
        boundaries.extend([start, end]);
    }
    boundaries.extend([uart_page, uart_page + PAGE_SIZE]);
    boundaries.sort_unstable();
    boundaries.dedup();
    for span in boundaries.windows(2) {
        let start = span[0];
        let end = span[1];
        let mut selected: Option<MappingOptions> = None;
        for &(first, last, options) in &ranges {
            if first <= start && end <= last {
                if let Some(current) = &mut selected {
                    if current.memory_type != options.memory_type {
                        return Err(VmError::Unsupported);
                    }
                    current.executable |= options.executable;
                } else {
                    selected = Some(options);
                }
            }
        }
        let options = if start == uart_page {
            MappingOptions::MMIO
        } else if let Some(options) = selected {
            options
        } else {
            continue;
        };
        let mut address = start;
        while address < end {
            let mut level = Level::LEAF;
            while let Some(parent) = level.parent() {
                if !supports_mapping(parent)
                    || address & (parent.coverage() - 1) != 0
                    || parent.coverage() > end - address
                {
                    break;
                }
                level = parent;
            }
            unsafe {
                map_block(
                    root,
                    address,
                    address,
                    level,
                    options,
                    &TABLE_ALLOC,
                    &UefiAddressTranslator,
                )
            }?;
            address += level.coverage();
        }
    }
    Ok(())
}

/// # safety
/// the context must map the current code and stack until the kernel takes over
pub unsafe fn mmu_init(context: AddressSpaceContext) {
    unsafe { context.activate() };
}
