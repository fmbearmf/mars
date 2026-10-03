pub(crate) mod boot;
pub(crate) mod cache;
pub(crate) mod context;
pub(crate) mod cpu;
pub(crate) mod debug;
pub(crate) mod exception;
pub(crate) mod interrupt;
pub(crate) mod local_interrupt;
pub(crate) mod memory;
pub(crate) mod secondary;
pub(crate) mod timer;

use crate::paging::{
    Access, Entry, Geometry, Level, MappingOptions, MemoryType, PageTable, PagingError, TableMemory,
};
use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CurrentEL, MAIR_EL1, ReadWriteable, Readable, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1,
        Writeable,
    },
};
use core::sync::atomic::{AtomicU64, Ordering};

pub(crate) const GEOMETRY: Geometry = Geometry::new(14, 11, 4, 48, 16384, 16384);
const VALID: u64 = 1;
const TABLE: u64 = 2;
const ADDRESS: u64 = 0x0000_ffff_ffff_c000;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

pub(crate) fn supports_mapping(level: Level) -> bool {
    level != Level::ROOT
}

pub(crate) fn validate_mapping(
    physical: usize,
    level: Level,
    options: MappingOptions,
) -> Result<(), PagingError> {
    encode(
        Entry::Mapping {
            physical_address: physical,
            options,
        },
        level,
    )
    .map(|_| ())
}

fn encode(entry: Entry, level: Level) -> Result<u64, PagingError> {
    let (physical, alignment) = match entry {
        Entry::Invalid => return Ok(0),
        Entry::Table { physical_address } => (physical_address, GEOMETRY.table_layout().align()),
        Entry::Mapping {
            physical_address, ..
        } => (physical_address, level.coverage()),
    };
    if physical >> 48 != 0 {
        return Err(PagingError::InvalidAddress);
    }
    if physical & (alignment - 1) != 0 {
        return Err(PagingError::InvalidAlignment);
    }
    match entry {
        Entry::Invalid => unreachable!(),
        Entry::Table { .. } => {
            if level == Level::LEAF {
                return Err(PagingError::Unsupported);
            }
            Ok(physical as u64 | VALID | TABLE)
        }
        Entry::Mapping { options, .. } => {
            if level == Level::ROOT {
                return Err(PagingError::Unsupported);
            }
            if options.executable && options.memory_type == MemoryType::Device {
                return Err(PagingError::Unsupported);
            }
            let attribute = match options.memory_type {
                MemoryType::Device => 0,
                MemoryType::Normal => 1,
                MemoryType::WriteThrough => 2,
                MemoryType::Uncached => 3,
            };
            let permission = match options.access {
                Access::KernelReadWrite => 0,
                Access::UserReadWrite => 1,
                Access::KernelReadOnly => 2,
                Access::UserReadOnly => 3,
            };
            let shareability = if options.memory_type == MemoryType::Device {
                2
            } else {
                3
            };
            let mut descriptor = physical as u64
                | VALID
                | (attribute << 2)
                | (permission << 6)
                | (shareability << 8)
                | (1 << 10);
            if level == Level::LEAF {
                descriptor |= TABLE;
            }
            descriptor |= if options.executable {
                if options.access.user() { PXN } else { UXN }
            } else {
                PXN | UXN
            };
            Ok(descriptor)
        }
    }
}

fn decode(descriptor: u64, level: Level) -> Entry {
    if descriptor & VALID == 0 {
        return Entry::Invalid;
    }
    let physical_address = (descriptor & ADDRESS) as usize;
    if descriptor & TABLE != 0 && level != Level::LEAF {
        return Entry::Table { physical_address };
    }
    let access = match (descriptor >> 6) & 3 {
        0 => Access::KernelReadWrite,
        1 => Access::UserReadWrite,
        2 => Access::KernelReadOnly,
        _ => Access::UserReadOnly,
    };
    let memory_type = match (descriptor >> 2) & 7 {
        0 => MemoryType::Device,
        1 => MemoryType::Normal,
        2 => MemoryType::WriteThrough,
        _ => MemoryType::Uncached,
    };
    let execution_bit = if access.user() { UXN } else { PXN };
    Entry::Mapping {
        physical_address,
        options: MappingOptions {
            access,
            executable: descriptor & execution_bit == 0,
            memory_type,
        },
    }
}

unsafe fn slot<'a>(table: PageTable, index: usize) -> &'a AtomicU64 {
    // shared atomic access permits hardware updates without manufacturing an exclusive reference
    unsafe { AtomicU64::from_ptr(table.as_ptr().cast::<u64>().add(index)) }
}

pub(crate) unsafe fn zero_table(table: PageTable) {
    unsafe { core::ptr::write_bytes(table.as_ptr(), 0, GEOMETRY.table_layout().size()) };
    unsafe { cache::clean_data_cache(table.as_ptr(), GEOMETRY.table_layout().size()) };
}

pub(crate) unsafe fn read_entry(table: PageTable, index: usize, level: Level) -> Entry {
    decode(unsafe { slot(table, index) }.load(Ordering::Acquire), level)
}

pub(crate) unsafe fn write_entry(
    table: PageTable,
    index: usize,
    level: Level,
    entry: Entry,
) -> Result<(), PagingError> {
    let descriptor = encode(entry, level)?;
    let slot = unsafe { slot(table, index) };
    if slot.load(Ordering::Acquire) & VALID != 0 {
        slot.store(0, Ordering::Release);
        unsafe { cache::clean_data_cache(slot.as_ptr().cast(), size_of::<u64>()) };
        // break-before-make, including walk-cache invalidation on all sharing cpus
        dsb(barrier::ISHST);
        invalidate_translations();
    }
    slot.store(descriptor, Ordering::Release);
    unsafe { cache::clean_data_cache(slot.as_ptr().cast(), size_of::<u64>()) };
    dsb(barrier::ISHST);
    isb(barrier::SY);
    Ok(())
}

pub(crate) fn valid_virtual_address(address: usize) -> bool {
    address >> 48 == 0 || address >> 48 == 0xffff
}

fn invalidate_translations() {
    // vhe aliases the el1 translation registers when executing at el2
    if CurrentEL.read(CurrentEL::EL) == 2 {
        unsafe { core::arch::asm!("tlbi alle2is", options(nostack, preserves_flags)) };
    } else {
        unsafe { core::arch::asm!("tlbi vmalle1is", options(nostack, preserves_flags)) };
    }
    dsb(barrier::ISH);
    isb(barrier::SY);
}

/// hardware roots are private to the backend, not part of the hal contract
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContextState {
    roots: [usize; 2],
    owned: [bool; 2],
}

pub(crate) unsafe fn create_context(memory: &dyn TableMemory) -> Result<ContextState, PagingError> {
    let first = memory.alloc_table()?;
    let second = match memory.alloc_table() {
        Ok(table) => table,
        Err(error) => {
            memory.free_table(first);
            return Err(error);
        }
    };
    Ok(ContextState {
        roots: [
            memory.physical_address(first),
            memory.physical_address(second),
        ],
        owned: [true, true],
    })
}

pub(crate) unsafe fn current_context() -> ContextState {
    ContextState {
        roots: [
            (TTBR0_EL1.get() & ADDRESS) as usize,
            (TTBR1_EL1.get() & ADDRESS) as usize,
        ],
        owned: [false, false],
    }
}

pub(crate) unsafe fn inherit_kernel_context(
    memory: &dyn TableMemory,
) -> Result<ContextState, PagingError> {
    let table = memory.alloc_table()?;
    let current = unsafe { current_context() };
    Ok(ContextState {
        roots: [memory.physical_address(table), current.roots[1]],
        owned: [true, false],
    })
}

pub(crate) fn context_table(state: ContextState, address: usize) -> usize {
    state.roots[usize::from(address >> (usize::BITS - 1) != 0)]
}

pub(crate) unsafe fn clone_context(
    state: ContextState,
    memory: &dyn TableMemory,
) -> Result<ContextState, PagingError> {
    let mut copy = ContextState {
        roots: [0; 2],
        owned: [false; 2],
    };
    for index in 0..2 {
        let source = unsafe { memory.table_at(state.roots[index]) };
        match unsafe { crate::paging::context::clone_tree(source, Level::ROOT, memory) } {
            Ok(table) => {
                copy.roots[index] = memory.physical_address(table);
                copy.owned[index] = true;
            }
            Err(error) => {
                unsafe { destroy_context(copy, memory) };
                return Err(error);
            }
        }
    }
    Ok(copy)
}

pub(crate) unsafe fn destroy_context(state: ContextState, memory: &dyn TableMemory) {
    for index in 0..2 {
        if state.owned[index] {
            unsafe {
                crate::paging::context::destroy_tree(
                    memory.table_at(state.roots[index]),
                    Level::ROOT,
                    memory,
                )
            };
        }
    }
}

pub(crate) unsafe fn activate_context(state: ContextState) {
    for physical in state.roots {
        assert_eq!(physical & !ADDRESS as usize, 0, "invalid context root");
    }
    dsb(barrier::ISH);
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_noEarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr2_Normal_Outer::WriteThrough_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr2_Normal_Inner::WriteThrough_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr3_Normal_Outer::NonCacheable
            + MAIR_EL1::Attr3_Normal_Inner::NonCacheable,
    );
    TCR_EL1.modify(
        TCR_EL1::TBI0::Ignored
            + TCR_EL1::TBI1::Ignored
            + TCR_EL1::IPS::Bits_48
            + TCR_EL1::TG0::KiB_16
            + TCR_EL1::TG1::KiB_16
            + TCR_EL1::SH0::Inner
            + TCR_EL1::SH1::Inner
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD0::EnableTTBR0Walks
            + TCR_EL1::EPD1::EnableTTBR1Walks
            + TCR_EL1::T0SZ.val(16)
            + TCR_EL1::T1SZ.val(16),
    );
    TTBR0_EL1.set_baddr(state.roots[0] as u64);
    TTBR1_EL1.set_baddr(state.roots[1] as u64);
    isb(barrier::SY);
    invalidate_translations();
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
    isb(barrier::SY);
}

pub(crate) use cache::{
    clean_and_invalidate_data_cache, clean_data_cache, clean_instruction_cache,
    invalidate_data_cache,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attributes_round_trip() {
        for access in [
            Access::KernelReadOnly,
            Access::KernelReadWrite,
            Access::UserReadOnly,
            Access::UserReadWrite,
        ] {
            for executable in [false, true] {
                let entry = Entry::Mapping {
                    physical_address: 0x4000,
                    options: MappingOptions {
                        access,
                        executable,
                        memory_type: MemoryType::Normal,
                    },
                };
                assert_eq!(
                    decode(encode(entry, Level::LEAF).unwrap(), Level::LEAF),
                    entry
                );
            }
        }
    }
    #[test]
    fn upper_addresses_do_not_include_sign_extension_in_root_index() {
        assert_eq!(Level::ROOT.index(0xffff_0000_0000_0000), 0);
        assert_eq!(Level::ROOT.index(0xffff_8000_0000_0000), 1);
    }
}
