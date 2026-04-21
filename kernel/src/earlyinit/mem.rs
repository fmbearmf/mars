extern crate alloc;

use core::{
    arch::asm,
    ops::Range,
    ptr::{self, NonNull},
    slice,
};

use crate::{
    KALLOCATOR, KSTACK, allocator::KernelAddressTranslator, busy_loop_ret, earlycon_writeln,
    earlycon_writeln_debug,
};
use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{TTBR1_EL1, Writeable},
};
use aarch64_cpu_ext::{
    asm::tlb::{VMALLE1, tlbi},
    structures::tte::{AccessPermission, Granule, OA, Shareability, TTE64},
};
use alloc::{boxed::Box, vec::Vec};
use klib::{
    pm::page::mapper::{AddressTranslator, TableAllocator, clone_page_tables, map_page},
    sync::RwLock,
    vm::{
        MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, PAGE_MASK, PAGE_SIZE, TABLE_ENTRIES, TTable, VmError,
        align_down, align_up,
        page_allocator::PhysicalPageAllocator,
        phys_addr_to_dmap,
        user::{PageDescriptor, PtState},
    },
};
use log::debug;
use protocol::BootInfo;
use tock_registers::LocalRegisterCopy;
use uefi::{
    boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMeta, MemoryMapOwned, MemoryMapRefMut},
};

struct BootTempAllocator<'a, P: PhysicalPageAllocator>(pub &'a P);

impl<'a, P: PhysicalPageAllocator> PhysicalPageAllocator for BootTempAllocator<'a, P> {
    fn alloc_phys_page<T: Into<usize> + From<usize>>(&self) -> Result<T, VmError> {
        self.0.alloc_phys_page()
    }
    fn free_phys_page<T: Into<usize> + From<usize>>(&self, pa: T) {
        self.0.free_phys_page(pa)
    }
}
impl<'a, P: PhysicalPageAllocator> TableAllocator for BootTempAllocator<'a, P> {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let pa: usize = self.alloc_phys_page().expect("OOM in boot alloc");
        let ptr = pa as *mut TTable<TABLE_ENTRIES>;

        unsafe {
            ptr.write_bytes(0, 1);
        };
        NonNull::new(ptr).expect("null ptr")
    }
    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        self.0.free_phys_page(table.as_ptr() as usize)
    }
}

struct IdentityTranslator;
impl AddressTranslator for IdentityTranslator {
    fn dmap_to_phys<T>(virt: *mut T) -> u64 {
        virt as _
    }
    fn phys_to_dmap<T>(phys: u64) -> *mut T {
        phys as *mut _
    }
}

macro_rules! kernel_address_space {
    () => {
        let guard = crate::KERNEL_ADDRESS_SPACE.read().unwrap();
    };
}

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
pub fn clone_and_process_mmap<T: MemoryMap>(map: &T) -> MemoryMapRefMut<'static> {
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

pub fn populate_alloc<T: MemoryMap>(map: &T) {
    let page_alloc = unsafe { KALLOCATOR.page_alloc_mut() };

    for entry in map.entries().filter(|x| {
        x.ty == MemoryType::CONVENTIONAL && x.page_count as usize * UEFI_PS >= 2 * PAGE_SIZE
    }) {
        let start = align_up(entry.phys_start as usize, PAGE_SIZE);
        let end = align_down(
            entry.phys_start as usize + (entry.page_count as usize * UEFI_PS),
            PAGE_SIZE,
        );

        let range = Range { start, end };

        page_alloc.add_range(&range);
    }
}

fn descriptor_to_meta(
    desc: &MemoryDescriptor,
) -> (AccessPermission, Shareability, bool, bool, u64) {
    let attr_index = match desc.att
        & (MemoryAttribute::UNCACHEABLE
            | MemoryAttribute::WRITE_BACK
            | MemoryAttribute::WRITE_THROUGH
            | MemoryAttribute::WRITE_COMBINE)
    {
        MemoryAttribute::UNCACHEABLE => MAIR_DEVICE_INDEX,
        MemoryAttribute::WRITE_BACK => MAIR_NORMAL_INDEX,
        MemoryAttribute::WRITE_THROUGH => todo!(),
        MemoryAttribute::WRITE_COMBINE => todo!(),
        _ => todo!(), // multiple
    };

    let (access, share, pxn) = match desc.ty {
        MemoryType::CONVENTIONAL => (
            AccessPermission::PrivilegedReadWrite,
            Shareability::InnerShareable,
            false,
        ),
        MemoryType::MMIO | MemoryType::RUNTIME_SERVICES_DATA => (
            AccessPermission::PrivilegedReadWrite,
            Shareability::OuterShareable,
            true,
        ),
        MemoryType::ACPI_RECLAIM => (
            AccessPermission::PrivilegedReadOnly,
            Shareability::InnerShareable,
            true,
        ),
        MemoryType::RUNTIME_SERVICES_CODE => (
            AccessPermission::PrivilegedReadOnly,
            Shareability::InnerShareable,
            false,
        ),
        _ => todo!(),
    };

    (access, share, true, pxn, attr_index)
}

#[inline(always)]
pub fn sp_get() -> usize {
    let x: usize;
    unsafe { asm!("mov {}, sp", out(reg) x, options(nomem, nostack, preserves_flags)) };
    x
}

#[inline(never)]
pub fn early_stack_size_check() {
    #[cfg(debug_assertions)]
    {
        #[allow(static_mut_refs, reason = "`KALLOCATOR` synchronizes access")]
        {
            use log::debug;

            let sym = unsafe {
                use crate::{KSTACK, KStack};
                &KSTACK as *const KStack
            };
            let sym_top = unsafe { sym.add(1) } as usize;
            let sp = sp_get();
            debug!(
                "stack usage: {}, bottom: {:#p}, sp: {:#x}",
                sym_top - sp,
                sym,
                sp
            );
        }
    }
}

pub unsafe fn switch_to_new_page_tables<M: MemoryMap, P: PhysicalPageAllocator>(
    memory_map: &M,
    allocator: &P,
    kernel_load_pa: usize,
    kernel_load_size: usize,
) -> NonNull<TTable<TABLE_ENTRIES>> {
    let boot_alloc = BootTempAllocator(allocator);

    let pt = TTBR1_EL1.get_baddr() as *mut TTable<TABLE_ENTRIES>;
    let pt = unsafe { &*pt };

    let mut new_pt = clone_page_tables::<_, IdentityTranslator>(pt, &boot_alloc);
    let root_table = unsafe { new_pt.as_mut() };

    map_all_dmap(root_table, memory_map, &boot_alloc, |desc| {
        descriptor_to_meta(desc)
    });

    debug_assert_eq!(
        kernel_load_pa & PAGE_MASK,
        0,
        "unaligned load physical address"
    );

    TTBR1_EL1.set_baddr(root_table as *const _ as _);

    dsb(barrier::ISHST);
    tlbi(VMALLE1);
    dsb(barrier::SY);
    isb(barrier::SY);

    NonNull::from_ref(root_table)
}

pub fn print_pt(root: &TTable<TABLE_ENTRIES>, verbose: bool) {
    let mut l0_tally = 0;
    let mut l1_tally = 0;
    let mut l2_tally = 0;
    let mut l3_tally = 0;

    for i0 in 0..2 {
        let l0_entry = &root.entries[i0];

        if !l0_entry.is_valid() {
            continue;
        }

        debug!(
            "L0 entry {} ({:#x}) -> {:#x}",
            i0,
            l0_entry.get(),
            l0_entry.address()
        );

        if l0_entry.is_table() {
            l0_tally += 1;

            let l1_table = l0_entry.address() as *const TTable<TABLE_ENTRIES>;
            let l1_table = unsafe { &*l1_table };

            debug!("TBL \\->");

            for i1 in 0..TABLE_ENTRIES {
                let l1_entry = &l1_table.entries[i1];

                if !l1_entry.is_valid() {
                    continue;
                }

                debug!(
                    "   L1 entry {} ({:#x}) -> {:#x}",
                    i1,
                    l1_entry.get(),
                    l1_entry.address()
                );

                if l1_entry.is_table() {
                    l1_tally += 1;

                    let l2_table = l1_entry.address() as *const TTable<TABLE_ENTRIES>;
                    let l2_table = unsafe { &*l2_table };

                    debug!("   TBL \\->");

                    for i2 in 0..TABLE_ENTRIES {
                        let l2_entry = &l2_table.entries[i2];

                        if !l2_entry.is_valid() {
                            continue;
                        }

                        debug!(
                            "      L2 entry {} ({:#x}) -> {:#x}",
                            i2,
                            l2_entry.get(),
                            l2_entry.address()
                        );

                        if l2_entry.is_table() {
                            l2_tally += 1;

                            let l3_table = l2_entry.address() as *const TTable<TABLE_ENTRIES>;
                            let l3_table = unsafe { &*l3_table };

                            debug!("      TBL \\->");

                            let mut local_tally = 0;

                            for i3 in 0..TABLE_ENTRIES {
                                let l3_entry = &l3_table.entries[i3];

                                if !l3_entry.is_valid() {
                                    continue;
                                }

                                if verbose {
                                    debug!(
                                        "         L3 entry {} (={:#x}) -> {:#x}",
                                        i3,
                                        l3_entry.get(),
                                        l3_entry.address(),
                                    );
                                }
                                local_tally += 1;
                            }
                            l3_tally += local_tally;

                            if !verbose {
                                debug!("         (...{} L3 entries)", local_tally);
                            }
                        }
                    }
                }
            }
        }
    }

    debug!(
        "l0, l1, l2, l3 tally: {}, {}, {}, {}",
        l0_tally, l1_tally, l2_tally, l3_tally
    );
    debug!(
        "total table memory size: {}",
        (l0_tally + l1_tally + l2_tally) * PAGE_SIZE
    )
}

fn map_all_dmap<M, P, F>(
    root_table: &mut TTable<TABLE_ENTRIES>,
    memory_map: &M,
    boot_alloc: &BootTempAllocator<'_, P>,
    mut get_params: F,
) where
    M: MemoryMap,
    P: PhysicalPageAllocator,
    F: FnMut(&MemoryDescriptor) -> (AccessPermission, Shareability, bool, bool, u64),
{
    let mut ranges = Vec::new();
    for desc in memory_map.entries() {
        let start = desc.phys_start as usize;
        let end = start + (desc.page_count as usize * UEFI_PS);

        let start = align_down(start, PAGE_SIZE);
        let end = align_up(end, PAGE_SIZE);

        if start < end {
            ranges.push((start, end));
        }
    }

    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();

    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if last.1 >= r.0 {
                if r.1 > last.1 {
                    last.1 = r.1;
                }
                continue;
            }
        }
        merged.push(r);
    }

    for (start, end) in merged {
        let mut current_pa = start;

        while current_pa < end {
            let mut optimal: Option<&MemoryDescriptor> = None;

            for desc in memory_map.entries() {
                let desc_start = desc.phys_start as usize;
                let desc_end = desc_start + (desc.page_count as usize * UEFI_PS);

                if desc_start < current_pa + PAGE_SIZE && desc_end > current_pa {
                    match optimal {
                        None => optimal = Some(desc),
                        Some(current) => {
                            // resolve conflict by
                            // preferring strictest attributes

                            let current_uncacheable =
                                current.att.contains(MemoryAttribute::UNCACHEABLE);
                            let desc_uncacheable = desc.att.contains(MemoryAttribute::UNCACHEABLE);

                            if desc_uncacheable && !current_uncacheable {
                                optimal = Some(desc);
                            }
                        }
                    }
                }
            }

            if let Some(desc) = optimal {
                let (access, share, uxn, pxn, attr_index) = get_params(desc);
                let va = phys_addr_to_dmap(current_pa as _) as usize;

                map_page::<BootTempAllocator<'_, P>, IdentityTranslator>(
                    root_table,
                    current_pa,
                    va,
                    access,
                    share,
                    uxn,
                    pxn,
                    attr_index,
                    &boot_alloc,
                );
            }

            current_pa += PAGE_SIZE;
        }
    }
}
