use core::{ffi::c_void, mem, ptr, slice};

use getters::unaligned_getters;
use uefi::{system, table::cfg::ConfigTableEntry};

use super::{
    super::vm::{is_kernel_address, phys_addr_to_dmap},
    SdtHeader, checksum,
};

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
                if i.guid == ConfigTableEntry::ACPI2_GUID {
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

    pub fn xsdt(&self, kernel: bool) -> Result<&'static SdtHeader, &'static str> {
        let xsdt_addr = if kernel {
            phys_addr_to_dmap(self.xsdt_addr())
        } else {
            self.xsdt_addr()
        };

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
    kernel: bool,
}

impl XsdtIter {
    pub fn new(xsdt: &SdtHeader, kernel: bool) -> Self {
        let len = xsdt.len() as usize;
        let header_sz = mem::size_of::<SdtHeader>();

        if len <= header_sz {
            return Self {
                base: ptr::null(),
                count: 0,
                curr: 0,
                kernel,
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
            kernel,
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

        let mut header = unsafe { &*(table_addr as *const SdtHeader) };
        if self.kernel {
            header =
                unsafe { &*(phys_addr_to_dmap(header as *const _ as u64) as *const SdtHeader) };
        }
        Some(header)
    }
}

pub fn find_rsdp_in_slice(slice: &[u8]) -> Option<&Rsdp> {
    const SIG: &[u8; 8] = b"RSD PTR ";

    // packed struct has no alignment requirements, therefore `STEP` = 1
    const STEP: usize = align_of::<Rsdp>();

    let mut i = 0;
    while i + size_of::<Rsdp>() <= slice.len() {
        let candidate = &slice[i..];

        if &candidate[..8] == SIG {
            let rsdp = unsafe { &*(candidate.as_ptr() as *const Rsdp) };

            if checksum(&candidate[..20]) != 0 {
                i += STEP;
                continue;
            }

            if rsdp.rev() >= 2 {
                let len = rsdp.len() as usize;
                if i + len > slice.len() || checksum(&candidate[..len]) != 0 {
                    i += STEP;
                    continue;
                }
            }

            return Some(rsdp);
        }

        i += STEP;
    }

    None
}
