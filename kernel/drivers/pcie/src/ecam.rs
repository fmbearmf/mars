use core::ptr::{read_volatile, write_volatile};

use klib::{allocator_support::KernelAddressTranslator, pm::page::mapper::AddressTranslator};

use crate::address::Bdf;

pub struct Ecam {
    pub phys_base: u64,
    pub segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

impl Ecam {
    pub const fn new(phys_base: u64, segment: u16, start_bus: u8, end_bus: u8) -> Self {
        Self {
            phys_base,
            segment,
            start_bus,
            end_bus,
        }
    }

    pub fn enable_bus_master(&self, bdf: Bdf) {
        const CMD_REG: u16 = 0x04;
        let cmd = self.read_u16(bdf, CMD_REG);
        self.write_u16(bdf, CMD_REG, cmd | (1 << 2));
    }

    pub fn enable_memory_space(&self, bdf: Bdf) {
        const CMD_REG: u16 = 0x04;
        let cmd = self.read_u16(bdf, CMD_REG);
        self.write_u16(bdf, CMD_REG, cmd | (1 << 1));
    }

    fn offset_ptr(&self, bdf: Bdf, offset: u16) -> Option<*mut u8> {
        if offset > 0x0FFF {
            return None;
        }

        if bdf.segment != self.segment || !(self.start_bus..=self.end_bus).contains(&bdf.bus) {
            return None;
        }

        let bus_offset = (bdf.bus - self.start_bus) as usize;
        let cfg_offset = (bus_offset << 20)
            | ((bdf.device as usize) << 15)
            | ((bdf.function as usize) << 12)
            | (offset as usize);

        let phys = self.phys_base as usize + cfg_offset;
        let dmap = KernelAddressTranslator.phys_to_dmap(phys) as *mut u8;

        Some(dmap)
    }

    pub fn read_u8(&self, bdf: Bdf, offset: u16) -> u8 {
        self.offset_ptr(bdf, offset)
            .map(|ptr| unsafe { read_volatile(ptr) })
            .unwrap_or(0xFF)
    }

    pub fn read_u16(&self, bdf: Bdf, offset: u16) -> u16 {
        assert_eq!(offset % 2, 0, "unaligned ECAM read_u16");
        self.offset_ptr(bdf, offset)
            .map(|ptr| unsafe { read_volatile(ptr as *const u16) })
            .unwrap_or(0xFFFF)
    }

    pub fn read_u32(&self, bdf: Bdf, offset: u16) -> u32 {
        assert_eq!(offset % 4, 0, "unaligned ECAM read_u32");
        self.offset_ptr(bdf, offset)
            .map(|ptr| unsafe { read_volatile(ptr as *const u32) })
            .unwrap_or(0xFFFF_FFFF)
    }

    pub fn write_u8(&self, bdf: Bdf, offset: u16, val: u8) {
        if let Some(ptr) = self.offset_ptr(bdf, offset) {
            unsafe { write_volatile(ptr, val) };
        }
    }

    pub fn write_u16(&self, bdf: Bdf, offset: u16, val: u16) {
        assert_eq!(offset % 2, 0, "unaligned ECAM write_u16");
        if let Some(ptr) = self.offset_ptr(bdf, offset) {
            unsafe { write_volatile(ptr as *mut u16, val) };
        }
    }

    pub fn write_u32(&self, bdf: Bdf, offset: u16, val: u32) {
        assert_eq!(offset % 4, 0, "unaligned ECAM write_u32");
        if let Some(ptr) = self.offset_ptr(bdf, offset) {
            unsafe { write_volatile(ptr as *mut u32, val) };
        }
    }
}
