use core::{ffi::c_void, fmt, mem, ptr, slice, str::from_utf8};

use getters::unaligned_getters;
use uefi::{system, table::cfg::ACPI2_GUID};

use super::{SdtHeader, checksum};

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Clone, Copy)]
pub struct Rsdp {
    pub sig: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub rev: u8,
    pub rsdt_addr: u32,
    pub len: u32,
    pub xsdt_addr: u64,
    pub ext_checksum: u8,
    pub reserved: [u8; 3],
}

impl Rsdp {
    pub fn find() -> Result<&'static Rsdp, &'static str> {
        let acpi2_addr: *const c_void = system::with_config_table(|slice| {
            for i in slice {
                if i.guid == ACPI2_GUID {
                    return i.address;
                }
            }
            core::ptr::null()
        });

        if acpi2_addr.is_null() {
            return Err("null acpi2 addr");
        }

        let rsdp = unsafe { &*(acpi2_addr as *const Rsdp) };

        if rsdp.sig != *b"RSD PTR " {
            return Err("invalid RSDP sig");
        }

        let rev = rsdp.rev();

        if rev < 2 {
            return Err("erm... whar?");
        }

        let len = rsdp.len() as usize;

        if len < mem::size_of::<Rsdp>() {
            return Err("rsdp too small");
        }

        let data = unsafe { slice::from_raw_parts(rsdp as *const _ as *const u8, len) };
        if checksum(data) != 0 {
            return Err("rsdp checksum fail");
        }

        Ok(rsdp)
    }

    pub fn xsdt(&self) -> Result<&'static SdtHeader, &'static str> {
        let xsdt_addr = self.xsdt_addr();

        if (xsdt_addr as *const ()).is_null() {
            return Err("xsdt addr null");
        }

        let xsdt = unsafe { &*(xsdt_addr as *const SdtHeader) };

        if xsdt.sig != *b"XSDT" {
            return Err("invalid xsdt sig");
        }

        let len = xsdt.len() as usize;
        let data = unsafe { slice::from_raw_parts(xsdt as *const _ as *const u8, len) };

        if checksum(data) != 0 {
            return Err("xsdt checksum fail");
        }

        Ok(xsdt)
    }
}

pub struct XsdtIter {
    base: *const u8,
    count: usize,
    curr: usize,
}

impl XsdtIter {
    pub fn new(xsdt: &SdtHeader) -> Self {
        let len = xsdt.len() as usize;
        let header_sz = mem::size_of::<SdtHeader>();

        if len <= header_sz {
            return Self {
                base: ptr::null(),
                count: 0,
                curr: 0,
            };
        }

        let data_len = len - header_sz;

        // bytes
        let count = data_len / 8;

        let base = unsafe { (xsdt as *const _ as *const u8).add(header_sz) };

        Self {
            base,
            count,
            curr: 0,
        }
    }
}

impl Iterator for XsdtIter {
    type Item = &'static SdtHeader;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr >= self.count {
            return None;
        }

        let entry_ptr = unsafe { self.base.add(self.curr * 8) } as *const u64;
        self.curr += 1;

        let table_addr = unsafe { ptr::read_unaligned(entry_ptr) };

        if table_addr == 0 {
            return self.next();
        }

        let header = unsafe { &*(table_addr as *const SdtHeader) };
        Some(header)
    }
}
