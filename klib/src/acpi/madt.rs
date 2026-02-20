use core::{ptr, slice};

use super::header::SdtHeader;

pub const MADT_GICC: u8 = 0x0B;
pub const MADT_GICD: u8 = 0x0C;
pub const MADT_GICR: u8 = 0x0E;
pub const MADT_ITS: u8 = 0x0F;

#[repr(C, packed)]
pub struct Madt {
    pub header: SdtHeader,
    pub local_interrupt_ctrl: u32,
    pub flags: u32,
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub len: u8,
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct GicCpuInterface {
    pub header: MadtEntryHeader,
    pub reserved: u16,
    pub cpu_interface_number: u32,
    pub acpi_cpu_uid: u32,
    pub flags: u32,
    pub parking_proto_ver: u32,
    pub perf_interrupt_gsiv: u32,
    pub parked_addr: u64,
    pub phys_base_addr: u64,
    pub gicv: u64,
    pub gich: u64,
    pub vgic_maint_int: u32,
    pub gicr_base_addr: u64,
    pub mpidr: u64,
    pub efficiency_class: u8,
    pub reserved2: [u8; 3],
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct GicDistributor {
    pub header: MadtEntryHeader,
    pub reserved: u16,
    pub gic_id: u32,
    pub phys_base: u64,
    pub system_vector_base: u32,
    pub reserved2: [u8; 3],
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct GicRedistributor {
    pub header: MadtEntryHeader,
    pub flags: u8,
    pub reserved: u8,
    pub discovery_range_base: u64,
    pub discovery_range_len: u32,
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct GicIts {
    pub header: MadtEntryHeader,
    pub flags: u8,
    pub reserved: u8,
    pub translation_id: u32,
    pub phys_base: u64,
    pub reserved2: u32,
}

pub struct MadtIter {
    ptr: *const u8,
    end: *const u8,
}

impl Iterator for MadtIter {
    type Item = (u8, &'static [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr >= self.end {
            return None;
        }

        unsafe {
            let entry_type = ptr::read_unaligned(self.ptr);
            let len = ptr::read_unaligned(self.ptr.add(1));
            if len < 2 {
                return None;
            }
            let slice = slice::from_raw_parts(self.ptr, len as usize);
            self.ptr = self.ptr.add(len as usize);
            Some((entry_type, slice))
        }
    }
}

impl Madt {
    pub fn entries(&self) -> MadtIter {
        unsafe {
            let start = self.header.data_ptr().add(8);
            let end = (self as *const _ as *const u8).add(self.header.len());
            MadtIter { ptr: start, end }
        }
    }
}
