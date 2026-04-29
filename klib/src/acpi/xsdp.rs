use core::{mem, ptr, slice};

use mars_getters::unaligned_getters;

use super::{super::vm::phys_addr_to_dmap, SdtHeader, checksum};

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Clone, Copy)]
pub struct Xsdp {
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

impl Xsdp {
    pub unsafe fn try_from_ptr(xsdp_ptr: *const Xsdp) -> Result<&'static Xsdp, &'static str> {
        if xsdp_ptr.is_null() {
            return Err("null");
        }

        unsafe {
            if (*xsdp_ptr).sig != *b"RSD PTR " {
                return Err("bad signature");
            }

            let rev = (*xsdp_ptr).rev();

            if rev < 2 {
                return Err("bad revision");
            }

            let len = (*xsdp_ptr).len() as usize;

            if len < mem::size_of::<Xsdp>() {
                return Err("bad length");
            }

            let data = slice::from_raw_parts(xsdp_ptr as *const u8, len);
            if checksum(data) != 0 {
                return Err("bad checksum");
            }
        }

        Ok(unsafe { &*xsdp_ptr })
    }

    pub fn xsdt(&self) -> Result<&'static SdtHeader, &'static str> {
        let xsdt_addr = self.xsdt_addr();

        if (xsdt_addr as *const ()).is_null() {
            return Err("null");
        }

        let xsdt = xsdt_addr as *const SdtHeader;

        unsafe {
            if (*xsdt).sig != *b"XSDT" {
                return Err("bad sig");
            }

            let len = (*xsdt).len() as usize;
            let data = slice::from_raw_parts(xsdt as *const u8, len);

            if checksum(data) != 0 {
                return Err("bad checksum");
            }
        }

        Ok(unsafe { &*xsdt })
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

        let byte_count = data_len / 8;

        let base = unsafe { (xsdt as *const _ as *const u8).add(header_sz) };

        Self {
            base,
            count: byte_count,
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
        let header =
            unsafe { &*(phys_addr_to_dmap(header as *const _ as u64) as *const SdtHeader) };

        Some(header)
    }
}
