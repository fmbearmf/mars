use core::ptr::NonNull;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use klib::{
    allocator_support::KernelAddressTranslator,
    pm::page::mapper::{AddressTranslator, map_page},
    vm::{
        MAIR_DEVICE_INDEX, PAGE_SIZE, TABLE_ENTRIES, TTENATIVE, TTable,
        user::address_space::KERNEL_ADDRESS_SPACE,
    },
};

use crate::dma::barrier;

pub(crate) struct Registers {
    base: NonNull<u8>,
    size: usize,
}

// MMIO has no thread affinity. rhe controller owns and serializes access to its registers
unsafe impl Send for Registers {}

impl Registers {
    /// SAFETY: called during one core device initialization.
    /// the physical range must be a firmware-assigned device aperture
    pub(crate) unsafe fn map(physical: usize, size: usize) -> Result<Self, &'static str> {
        let end = physical
            .checked_add(size)
            .filter(|end| *end <= (1usize << 48))
            .ok_or("MMIO range exceeds direct map")?;
        if physical == 0 || size == 0 || physical % 8 != 0 || size > isize::MAX as usize {
            return Err("invalid MMIO range");
        }
        let first = physical & !(PAGE_SIZE - 1);
        let last = end
            .checked_add(PAGE_SIZE - 1)
            .ok_or("MMIO rounding overflow")?
            & !(PAGE_SIZE - 1);
        let root = unsafe { KERNEL_ADDRESS_SPACE.root_mut() };
        for phys in (first..last).step_by(PAGE_SIZE) {
            let virt = KernelAddressTranslator.phys_to_dmap(phys) as usize;
            let mut table: *mut TTable<TABLE_ENTRIES> = root;
            let mut already_mapped = false;
            for level in 0..=3 {
                let index = TTENATIVE::calculate_index(virt as u64, level);
                let entry = unsafe { (*table).entries[index] };
                if !entry.is_valid() {
                    break;
                }
                // never RAM block mappings.
                if !entry.is_table() {
                    return Err("MMIO overlaps a block mapping");
                }
                if level == 3 {
                    if entry.address() != phys as u64
                        || entry.attr_index() != MAIR_DEVICE_INDEX
                        || entry.access_permission() != AccessPermission::PrivilegedReadWrite
                    {
                        return Err("MMIO conflicts with an existing mapping");
                    }
                    already_mapped = true;
                } else {
                    table = KernelAddressTranslator
                        .phys_to_dmap(entry.address() as usize)
                        .cast();
                }
            }
            if !already_mapped {
                map_page(
                    root,
                    phys,
                    virt,
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::OuterShareable,
                    true,
                    true,
                    MAIR_DEVICE_INDEX,
                    &KERNEL_ADDRESS_SPACE.allocator,
                    &KernelAddressTranslator,
                );
            }
        }
        barrier();
        Ok(Self {
            base: NonNull::new(KernelAddressTranslator.phys_to_dmap(physical))
                .ok_or("null MMIO")?,
            size,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.size
    }

    pub(crate) fn read32(&self, offset: usize) -> u32 {
        assert!(offset % 4 == 0 && offset.checked_add(4).is_some_and(|end| end <= self.size));
        let value = unsafe { self.base.as_ptr().add(offset).cast::<u32>().read_volatile() };
        barrier();
        u32::from_le(value)
    }

    pub(crate) fn write32(&mut self, offset: usize, value: u32) {
        assert!(offset % 4 == 0 && offset.checked_add(4).is_some_and(|end| end <= self.size));
        barrier();
        unsafe {
            self.base
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .write_volatile(value.to_le())
        };
        barrier();
    }

    pub(crate) fn read64(&self, offset: usize) -> u64 {
        assert!(offset % 8 == 0 && offset.checked_add(8).is_some_and(|end| end <= self.size));
        let value = unsafe { self.base.as_ptr().add(offset).cast::<u64>().read_volatile() };
        barrier();
        u64::from_le(value)
    }

    pub(crate) fn write64(&mut self, offset: usize, value: u64) {
        assert!(offset % 8 == 0 && offset.checked_add(8).is_some_and(|end| end <= self.size));
        barrier();
        unsafe {
            self.base
                .as_ptr()
                .add(offset)
                .cast::<u64>()
                .write_volatile(value.to_le())
        };
        barrier();
    }
}
