use core::{
    marker::PhantomData,
    ptr::{NonNull, read_volatile, write_volatile},
};

use alloc::vec::Vec;
use klib::{
    cpu_interface::CpuIdLogical, interrupt::singleton::get_interrupt_controller, per_cpu::PerCpu,
};

use crate::{
    address::Bdf,
    capability::{
        find_capability,
        standard::{MsixCap, StandardCapabilityId},
    },
    ecam::Ecam,
};

/// MMIO view of the MSI-X table
pub struct MsixTable {
    base: NonNull<u8>,
    vector_count: usize,
}

// SAFETY: MsixTable is MMIO. safe to be Send
unsafe impl Send for MsixTable {}

impl MsixTable {
    /// # SAFETY:
    /// - `base` must be valid & mapped virtual memory for the MSI-X table.
    /// - the memory must not be accessed concurrently
    pub const unsafe fn from_raw_parts(base: NonNull<u8>, vector_count: usize) -> Self {
        Self { base, vector_count }
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.vector_count
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.vector_count == 0
    }

    pub fn entry(&mut self, index: usize) -> Option<MsixTableEntry<'_>> {
        if index >= self.vector_count {
            return None;
        }

        let entry_ptr = unsafe { self.base.as_ptr().add(index * 16) };
        Some(MsixTableEntry {
            base: entry_ptr,
            _marker: PhantomData,
        })
    }

    pub fn iter_mut(&mut self) -> MsixTableIter<'_> {
        MsixTableIter {
            table: self,
            current: 0,
        }
    }
}

pub struct MsixTableIter<'a> {
    table: &'a mut MsixTable,
    current: usize,
}

impl<'a> Iterator for MsixTableIter<'a> {
    type Item = MsixTableEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.table.vector_count {
            return None;
        }

        let index = self.current;
        self.current += 1;

        let entry_ptr = unsafe { self.table.base.as_ptr().add(index * 16) };
        Some(MsixTableEntry {
            base: entry_ptr,
            _marker: PhantomData,
        })
    }
}

pub struct MsixTableEntry<'a> {
    base: *mut u8,
    _marker: PhantomData<&'a mut MsixTable>,
}

impl<'a> MsixTableEntry<'a> {
    const OFFSET_ADDR_LOW: usize = 0x00;
    const OFFSET_ADDR_HIGH: usize = 0x04;
    const OFFSET_DATA: usize = 0x08;
    const OFFSET_VEC_CTRL: usize = 0x0C;

    const VEC_CTRL_MASK_BIT: u32 = 1 << 0;

    pub fn configure(&mut self, doorbell: u64, event_id: u32, masked: bool) {
        self.set_address(doorbell);
        self.set_data(event_id);
        self.set_masked(masked);
    }

    #[inline]
    pub fn set_address(&mut self, addr: u64) {
        unsafe {
            write_volatile(
                self.base.add(Self::OFFSET_ADDR_LOW) as *mut u32,
                addr as u32,
            );
            write_volatile(
                self.base.add(Self::OFFSET_ADDR_HIGH) as *mut u32,
                (addr >> 32) as u32,
            );
        }
    }

    #[inline]
    pub fn address(&self) -> u64 {
        unsafe {
            let low = read_volatile(self.base.add(Self::OFFSET_ADDR_LOW) as *const u32) as u64;
            let high = read_volatile(self.base.add(Self::OFFSET_ADDR_HIGH) as *const u32) as u64;

            (high << 32) | low
        }
    }

    #[inline]
    pub fn set_data(&mut self, data: u32) {
        unsafe {
            write_volatile(self.base.add(Self::OFFSET_DATA) as *mut u32, data);
        }
    }

    #[inline]
    pub fn data(&self) -> u32 {
        unsafe { read_volatile(self.base.add(Self::OFFSET_DATA) as *const u32) }
    }

    #[inline]
    pub fn set_masked(&mut self, masked: bool) {
        unsafe {
            let ctrl_ptr = self.base.add(Self::OFFSET_VEC_CTRL) as *mut u32;
            let current = read_volatile(ctrl_ptr);
            let new_ctrl = if masked {
                current | Self::VEC_CTRL_MASK_BIT
            } else {
                current & !Self::VEC_CTRL_MASK_BIT
            };
            write_volatile(ctrl_ptr, new_ctrl);
        }
    }

    #[inline]
    pub fn is_masked(&self) -> bool {
        unsafe {
            let ctrl_ptr = self.base.add(Self::OFFSET_VEC_CTRL) as *const u32;
            (read_volatile(ctrl_ptr) & Self::VEC_CTRL_MASK_BIT) != 0
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct MsixInfo {
    pub cap_offset: u8,
    pub table_bir: u8,
    pub table_offset: u32,
    pub table_size: u16,
}

pub fn get_msix_info(ecam: &Ecam, bdf: Bdf) -> Option<MsixInfo> {
    let cap_offset = find_capability(ecam, bdf, StandardCapabilityId::MsiX)?;
    let msix = MsixCap::read(ecam, bdf, cap_offset);

    Some(MsixInfo {
        cap_offset,
        table_bir: msix.table_bir,
        table_offset: msix.table_offset,
        table_size: msix.table_size,
    })
}

pub fn enable_msix(ecam: &Ecam, bdf: Bdf, table: &mut MsixTable) -> Result<Vec<u32>, &'static str> {
    use log::*;

    let cap_offset = find_capability(ecam, bdf, StandardCapabilityId::MsiX)
        .ok_or("MSI-X capability not found")?;

    let msix = MsixCap::read(ecam, bdf, cap_offset);
    let num_vectors = (msix.table_size as usize).min(table.len());
    if num_vectors == 0 {
        return Err("MSI-X table capacity is 0");
    }

    ecam.enable_memory_space(bdf);
    ecam.enable_bus_master(bdf);

    let dev_id = bdf.device_id();
    let num_events_log2 = (msix.table_size as u32)
        .next_power_of_two()
        .trailing_zeros();

    let ctrl = get_interrupt_controller();

    if let Err(e) = ctrl.msi_register_device(dev_id, num_events_log2) {
        error!(
            "{}: Failed to register MSI device {:#x}: {:?}",
            bdf, dev_id, e
        );
        return Err("MSI device registration fail");
    }

    let doorbell = ctrl
        .msi_get_doorbell()
        .map_err(|_| "Failed to get MSI doorbell")?;

    // round-robin
    let num_cpus = (PerCpu::all().len() as u32).max(1);
    let mut mapped_lpis = Vec::with_capacity(num_vectors);

    for mut entry in table.iter_mut() {
        entry.set_masked(true);
    }

    for (event_id, mut entry) in table.iter_mut().take(num_vectors).enumerate() {
        let event_id = event_id as u32;
        let target_cpu = CpuIdLogical::new(event_id % num_cpus);

        match ctrl.msi_map_event(dev_id, event_id, None, target_cpu) {
            Ok(lpi) => {
                entry.configure(doorbell, event_id, false);
                mapped_lpis.push(lpi);
            }
            Err(e) => {
                // remain masked
                error!("{}: Failed to map EventID {}: {:?}", bdf, event_id, e);
            }
        }
    }

    if mapped_lpis.is_empty() {
        return Err("Failed to map any MSI-X vectors");
    }

    msix.enable(ecam, bdf);
    info!(
        "{}: Enabled {} MSI-X vectors across {} CPUs",
        bdf,
        mapped_lpis.len(),
        num_cpus
    );

    Ok(mapped_lpis)
}
