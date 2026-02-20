use core::mem;

use super::header::SdtHeader;
use getters::unaligned_getters;

#[repr(C, packed)]
#[unaligned_getters]
pub struct Mcfg {
    pub header: SdtHeader,
    pub reserved: u64,
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Clone, Copy)]
pub struct McfgAllocation {
    pub base_addr: u64,
    pub pci_segment_group: u16,
    pub start_bus_num: u8,
    pub end_bus_num: u8,
    pub reserved: u32,
}

impl Mcfg {
    pub fn allocations(&self) -> McfgIter {
        unsafe {
            let start = self.header.data_ptr().add(8);
            let end = (self as *const _ as *const u8).add(self.header.len());
            McfgIter { ptr: start, end }
        }
    }
}

pub struct McfgIter {
    ptr: *const u8,
    end: *const u8,
}

impl Iterator for McfgIter {
    type Item = &'static McfgAllocation;
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr >= self.end {
            return None;
        }

        unsafe {
            let item = &*(self.ptr as *const McfgAllocation);
            self.ptr = self.ptr.add(mem::size_of::<McfgAllocation>());
            Some(item)
        }
    }
}
