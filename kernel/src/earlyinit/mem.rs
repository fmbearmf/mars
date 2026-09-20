use core::{arch::asm, ptr::NonNull, range::Range};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::TTBR1_EL1,
};
use aarch64_cpu_ext::{
    asm::tlb::{VMALLE1, tlbi},
    structures::tte::{AccessPermission, Shareability},
};
use alloc::{boxed::Box, vec::Vec};
use klib::{
    allocator_support::KernelAddressTranslator,
    pm::page::{
        PageAllocator,
        mapper::{AddressTranslator, TableAllocator, clone_page_tables, map_page},
    },
    rangekeeper,
    smccc::print_psci_version,
    sync::RwLock,
    vm::{
        KALLOCATOR, MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, MAIR_NORMAL_WC_INDEX,
        MAIR_NORMAL_WT_INDEX, PAGE_SIZE, TABLE_ENTRIES, TTable, VmError, align_down, align_up,
        page_allocator::PhysicalPageAllocator,
        phys_addr_to_dmap,
        user::{PageDescriptor, PtState},
    },
};
use log::{debug, trace};
use uefi::boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS};

struct BootTempAllocator<'a>(pub &'a dyn PhysicalPageAllocator);

impl PhysicalPageAllocator for BootTempAllocator<'_> {
    fn alloc_phys_page(&self) -> Result<usize, VmError> {
        self.0.alloc_phys_page()
    }
    fn free_phys_page(&self, pa: usize) {
        self.0.free_phys_page(pa)
    }
}
impl TableAllocator for BootTempAllocator<'_> {
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
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        virt as _
    }
    fn phys_to_dmap(&self, phys: usize) -> *mut u8 {
        phys as *mut _
    }
}

/// check whether an entry is acceptable normal memory
fn is_normal_desc(desc: &MemoryDescriptor) -> bool {
    let att_bad = desc.att.contains(MemoryAttribute::RUNTIME);

    let ty_ok = desc.ty == MemoryType::CONVENTIONAL;

    !att_bad && ty_ok
}

fn can_merge(a: &MemoryDescriptor, b: &MemoryDescriptor) -> bool {
    if a.phys_start + (a.page_count * UEFI_PS as u64) != b.phys_start {
        return false;
    }

    a.ty == b.ty && a.att == b.att
}

pub fn create_page_descriptors() -> (Box<[PageDescriptor]>, Range<usize>) {
    let alloc = KALLOCATOR.page_alloc();

    let min = KernelAddressTranslator.dmap_to_phys(alloc.min_address() as *mut _) as usize;
    let max = KernelAddressTranslator.dmap_to_phys(alloc.max_address() as *mut _) as usize;
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

/// give the allocator a safe interim piece of memory.
pub fn populate_alloc_stage0() {
    let page_alloc = unsafe { KALLOCATOR.page_alloc_mut() };

    rangekeeper::for_each_range(|r| {
        log::info!("Rangekeeper range: {:#010x}..{:#010x}", r.start, r.end);
    });

    if let Some(entry) = rangekeeper::largest_range() {
        if (entry.end - entry.start) >= 4 * PAGE_SIZE {
            trace!("page allocator push stage0: {:#x?}", entry);
            page_alloc.add_range(&entry);
        }
    }
}

/// fully populate the allocator.
pub fn populate_alloc_stage1() {
    let page_alloc = unsafe { KALLOCATOR.page_alloc_mut() };

    rangekeeper::for_each_range(|range| {
        flush(page_alloc, range.start, range.end);
    });
}

fn flush(page_alloc: &mut PageAllocator, start: usize, end: usize) {
    let raw = Range { start, end };

    if let Some(overlap) = page_alloc.overlapping_range(&raw) {
        if overlap.start > start {
            add_subrange(page_alloc, start, overlap.start);
        }

        if overlap.end < end {
            add_subrange(page_alloc, overlap.end, end);
        }
    } else {
        add_subrange(page_alloc, start, end);
    }
}

fn add_subrange(page_alloc: &mut PageAllocator, start: usize, end: usize) {
    let start = align_up(start, PAGE_SIZE);
    let end = align_down(end, PAGE_SIZE);

    let start = KernelAddressTranslator.phys_to_dmap(start) as usize;
    let end = KernelAddressTranslator.phys_to_dmap(end) as usize;

    if end > start {
        let size = end - start;
        if size >= 4 * PAGE_SIZE {
            let range = Range { start, end };

            page_alloc.add_range(&range);
        }
    }
}

fn descriptor_to_meta(
    desc: &MemoryDescriptor,
) -> (AccessPermission, Shareability, bool, bool, u64) {
    let caching_mask = MemoryAttribute::UNCACHEABLE
        | MemoryAttribute::WRITE_COMBINE
        | MemoryAttribute::WRITE_THROUGH
        | MemoryAttribute::WRITE_BACK;

    let attr_index = match desc.att & caching_mask {
        MemoryAttribute::UNCACHEABLE => MAIR_DEVICE_INDEX,
        MemoryAttribute::WRITE_THROUGH => MAIR_NORMAL_WT_INDEX,
        MemoryAttribute::WRITE_COMBINE => MAIR_NORMAL_WC_INDEX,
        MemoryAttribute::WRITE_BACK => MAIR_NORMAL_INDEX,
        multiple => {
            // UEFI has advertised multiple supported caching attributes.
            // select the best caching mode, in the order of WB > WT > WC > UC
            if multiple.contains(MemoryAttribute::WRITE_BACK) {
                MAIR_NORMAL_INDEX
            } else if multiple.contains(MemoryAttribute::WRITE_THROUGH) {
                MAIR_NORMAL_WT_INDEX
            } else if multiple.contains(MemoryAttribute::WRITE_COMBINE) {
                MAIR_NORMAL_WC_INDEX
            } else {
                MAIR_DEVICE_INDEX
            }
        }
    };

    let (access, share, pxn) = match desc.ty {
        MemoryType::CONVENTIONAL
        | MemoryType::BOOT_SERVICES_DATA
        | MemoryType::BOOT_SERVICES_CODE => (
            AccessPermission::PrivilegedReadWrite,
            Shareability::InnerShareable,
            false,
        ),
        MemoryType::MMIO | MemoryType::RUNTIME_SERVICES_DATA | MemoryType::ACPI_NON_VOLATILE => (
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
        MemoryType::RESERVED => {
            use log::*;
            warn!(
                "invalid memory type: {:?}. this should've been caught",
                desc.ty
            );
            unimplemented!();
        }
        _ => (
            AccessPermission::PrivilegedReadOnly,
            Shareability::OuterShareable,
            true,
        ),
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

pub unsafe fn switch_to_new_page_tables<'a, F, I>(
    mmap_iterator_fn: F,
    allocator: &dyn PhysicalPageAllocator,
) -> NonNull<TTable<TABLE_ENTRIES>>
where
    F: Fn() -> I,
    I: Iterator<Item = &'a MemoryDescriptor>,
{
    let boot_alloc = BootTempAllocator(allocator);

    let pt = TTBR1_EL1.get_baddr() as *mut TTable<TABLE_ENTRIES>;
    let pt = unsafe { &*pt };

    let mut new_pt = clone_page_tables(pt, &boot_alloc);
    let root_table = unsafe { new_pt.as_mut() };

    map_all_dmap(root_table, mmap_iterator_fn, &boot_alloc, |desc| {
        descriptor_to_meta(desc)
    });

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

fn map_all_dmap<'a, F, FM, I>(
    root_table: &mut TTable<TABLE_ENTRIES>,
    memory_map: FM,
    boot_alloc: &BootTempAllocator<'_>,
    mut get_params: F,
) where
    FM: Fn() -> I,
    I: Iterator<Item = &'a MemoryDescriptor>,
    F: FnMut(&MemoryDescriptor) -> (AccessPermission, Shareability, bool, bool, u64),
{
    let (lower, _) = memory_map().size_hint();
    let mut ranges = Vec::with_capacity(lower.max(64));

    for desc in memory_map() {
        let start = desc.phys_start as usize;
        let end = start + (desc.page_count as usize * UEFI_PS);

        let start = align_down(start, PAGE_SIZE);
        let end = align_up(end, PAGE_SIZE);

        if start < end {
            ranges.push((start, end));
        }
    }

    for i in 1..ranges.len() {
        let mut j = i;
        while j > 0 && ranges[j - 1].0 > ranges[j].0 {
            ranges.swap(j - 1, j);
            j -= 1;
        }
    }
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

    let mut descs = memory_map().peekable();
    let mut active: Vec<&MemoryDescriptor> = Vec::new();

    for (start, end) in merged {
        let mut current_pa = start;

        while current_pa < end {
            let page_end = current_pa + PAGE_SIZE;

            while let Some(&desc) = descs.peek() {
                let desc_start = desc.phys_start as usize;
                if desc_start >= page_end {
                    break;
                }

                let desc_end = desc_start + (desc.page_count as usize * UEFI_PS);
                if desc_end > current_pa {
                    active.push(desc);
                }

                descs.next();
            }

            active.retain(|desc| {
                let desc_start = desc.phys_start as usize;
                let desc_end = desc_start + (desc.page_count as usize * UEFI_PS);
                desc_end > current_pa
            });

            let mut optimal: Option<&MemoryDescriptor> = None;

            for &desc in &active {
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

            if let Some(desc) = optimal {
                let (access, share, uxn, pxn, attr_index) = get_params(desc);
                let va = phys_addr_to_dmap(current_pa as _) as usize;

                map_page(
                    root_table,
                    current_pa,
                    va,
                    access,
                    share,
                    uxn,
                    pxn,
                    attr_index,
                    boot_alloc,
                    &IdentityTranslator,
                );
            }

            current_pa += PAGE_SIZE;
        }
    }
}
