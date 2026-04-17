extern crate alloc;

use core::{ptr, range::Range, slice};

use crate::{KALLOCATOR, allocator::KernelAddressTranslator};
use alloc::boxed::Box;
use klib::{
    pm::page::mapper::AddressTranslator,
    sync::RwLock,
    vm::{
        PAGE_SIZE, align_up,
        user::{PageDescriptor, PtState},
    },
};
use uefi::{
    boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMeta, MemoryMapMut, MemoryMapOwned, MemoryMapRefMut},
};

/// check whether an entry is acceptable normal memory
fn is_normal_desc(desc: &MemoryDescriptor) -> bool {
    let att_ok = !desc.att.contains(MemoryAttribute::RUNTIME);

    let ty_ok = match desc.ty {
        MemoryType::BOOT_SERVICES_CODE
        | MemoryType::BOOT_SERVICES_DATA
        | MemoryType::CONVENTIONAL
        | MemoryType::LOADER_DATA => true,
        _ => false,
    };

    att_ok && ty_ok
}

fn can_merge(a: &MemoryDescriptor, b: &MemoryDescriptor) -> bool {
    if a.phys_start + (a.page_count * UEFI_PS as u64) != b.phys_start {
        return false;
    }

    a.ty == b.ty && a.att == b.att
}

/// relocate memory map into the first usable region,
/// opportunistically merge memory regions in-place,
/// and finally return the relocated memory map
pub fn consume_and_process_mmap(map: MemoryMapOwned) -> MemoryMapRefMut<'static> {
    let meta = map.meta();
    let desc_size = meta.desc_size;

    let mut final_count = 0;
    let mut first_normal_start: Option<u64> = None;
    let mut last_processed: Option<MemoryDescriptor> = None;

    for desc in map.entries().filter(|d| d.ty != MemoryType::LOADER_CODE) {
        let mut current = *desc;

        if is_normal_desc(&current) {
            current.ty = MemoryType::CONVENTIONAL;
            if first_normal_start.is_none() {
                first_normal_start = Some(current.phys_start);
            }
        }

        if let Some(ref mut last) = last_processed {
            if can_merge(last, &current) {
                last.page_count += current.page_count;
            } else {
                final_count += 1;
                last_processed = Some(current);
            }
        } else {
            last_processed = Some(current);
        }
    }

    if last_processed.is_some() {
        final_count += 1;
    }

    let dest_pa = first_normal_start.expect("no suitable memory found");

    let map_bytes = final_count * desc_size;
    let map_pages = align_up(map_bytes, UEFI_PS) / UEFI_PS;
    let dest_ptr = dest_pa as *mut u8;

    let mut write_i = 0;
    let mut merged: Option<MemoryDescriptor> = None;

    let mut punch = |mut desc: MemoryDescriptor| {
        if desc.phys_start <= dest_pa
            && (desc.phys_start + desc.page_count * UEFI_PS as u64) > dest_pa
        {
            let offset_pages = (dest_pa - desc.phys_start) / UEFI_PS as u64;
            let total_needed = offset_pages + map_pages as u64;

            if desc.page_count > total_needed {
                desc.phys_start += total_needed * UEFI_PS as u64;
                desc.page_count -= total_needed;
            } else {
                // entirely consumed
                return;
            }
        }

        let ptr = unsafe { dest_ptr.add(write_i * desc_size) as *mut MemoryDescriptor };
        unsafe {
            ptr::write(ptr, desc);
        };
        write_i += 1;
    };

    for desc in map
        .entries()
        .filter(|desc| desc.ty != MemoryType::LOADER_CODE)
    {
        let mut current = *desc;

        if is_normal_desc(&current) {
            current.ty = MemoryType::CONVENTIONAL;
        }

        if let Some(ref mut last) = merged {
            if can_merge(last, &current) {
                last.page_count += current.page_count;
            } else {
                punch(*last);
                merged = Some(current);
            }
        } else {
            merged = Some(current);
        }
    }

    if let Some(last) = merged {
        punch(last);
    }

    let final_map_size = write_i * desc_size;
    let final_buf = unsafe { slice::from_raw_parts_mut(dest_ptr, final_map_size) };

    MemoryMapRefMut::new(
        final_buf,
        MemoryMapMeta {
            desc_size,
            map_size: final_map_size,
            map_key: meta.map_key,
            desc_version: meta.desc_version,
        },
    )
    .expect("invalid ref")
}

pub fn create_page_descriptors() -> (Box<[PageDescriptor]>, Range<usize>) {
    let alloc = KALLOCATOR.page_alloc();

    let min = KernelAddressTranslator::dmap_to_phys(alloc.min_address() as *mut usize) as usize;
    let max = KernelAddressTranslator::dmap_to_phys(alloc.max_address() as *mut usize) as usize;
    let size = max - min;
    let pages = size / PAGE_SIZE;

    let mut uninit = Box::<[PageDescriptor]>::new_uninit_slice(pages);

    for slot in uninit.iter_mut() {
        slot.write(PageDescriptor {
            lock: RwLock::new(PtState { meta: None }),
        });
    }

    (
        unsafe { uninit.assume_init() },
        Range {
            start: min,
            end: max,
        },
    )
}
