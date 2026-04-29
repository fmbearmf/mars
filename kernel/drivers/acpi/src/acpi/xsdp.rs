use core::{marker::PhantomData, mem, num::NonZeroUsize, ptr, slice};

use super::FromBytes;
use hax_lib::{assume, attributes, ensures, include, loop_invariant, opaque, requires};

use crate::{
    acpi::{AcpiTableTrait, get_memory_slice, get_ref_addr, safe_table_cast},
    impl_table,
};

use super::{SdtHeader, checksum};

impl_table! {
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
}

#[attributes]
impl AcpiTableTrait for Xsdp {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}

#[attributes]
impl Xsdp {
    #[requires(xsdp_addr != 0)]
    #[ensures(|result| match result {
        Ok(xsdp) => xsdp.rev() >= 2 &&
                    xsdp.compute_checksum() == 0 &&
                    xsdp.len() as usize >= core::mem::size_of::<Xsdp>() &&
                    xsdp.sig() == *b"RSD PTR ",
        Err(_) => true
    })]
    pub fn try_from_addr(xsdp_addr: usize) -> Result<&'static Xsdp, &'static str> {
        let slice = get_memory_slice(xsdp_addr, mem::size_of::<Xsdp>());
        let xsdp: &Xsdp = safe_table_cast(slice)?;

        if xsdp.sig() != *b"RSD PTR " {
            return Err("bad signature");
        }

        if xsdp.rev() < 2 {
            return Err("bad revision");
        }

        let len = xsdp.len() as usize;
        if len < mem::size_of::<Xsdp>() {
            return Err("bad length");
        }

        if xsdp.compute_checksum() != 0 {
            return Err("bad checksum");
        }

        Ok(xsdp)
    }

    fn compute_checksum(&self) -> u8 {
        let addr = get_ref_addr(self);
        let len = self.len() as usize;

        let slice = get_memory_slice(addr, len);

        checksum(slice)
    }

    #[requires(self.xsdt_addr() as usize != 0)]
    pub fn xsdt(&self) -> Result<&'static SdtHeader, &'static str> {
        let xsdt_addr = self.xsdt_addr() as usize;

        if xsdt_addr == 0 {
            unreachable!();
        }

        let slice = get_memory_slice(xsdt_addr, mem::size_of::<SdtHeader>());
        let xsdt: &SdtHeader = safe_table_cast(slice)?;

        if xsdt.sig() != *b"XSDT" {
            return Err("bad sig");
        }

        xsdt.check()?;

        Ok(xsdt)
    }
}

pub struct XsdtIter<'a> {
    base: usize,
    count: usize,
    curr: usize,
    _marker: PhantomData<&'a SdtHeader>,
}

#[attributes]
impl<'a> XsdtIter<'a> {
    #[opaque]
    #[ensures(|result| result > 0)]
    fn get_base_addr(xsdt: &SdtHeader) -> usize {
        let base_ptr = xsdt as *const SdtHeader;
        unsafe { base_ptr.add(1) as usize }
    }

    #[requires(xsdt.len() as usize >= mem::size_of::<SdtHeader>())]
    #[ensures(|result| result.curr == 0 && result.count == (xsdt.len() as usize - mem::size_of::<SdtHeader>()) / 8)]
    pub fn new(xsdt: &SdtHeader) -> Self {
        let len = xsdt.len() as usize;
        let header_sz = mem::size_of::<SdtHeader>();

        if len < header_sz {
            panic!("wrong size!");
        }

        let data_len = len - header_sz;
        let byte_count = data_len / 8;

        Self {
            base: Self::get_base_addr(xsdt),
            count: byte_count,
            curr: 0,
            _marker: PhantomData,
        }
    }

    #[opaque]
    #[requires(index < self.count)]
    fn read_table_entry(&self, index: usize) -> u64 {
        unsafe {
            let entry_ptr = (self.base + index * mem::size_of::<usize>()) as *const u64;
            ptr::read_unaligned(entry_ptr)
        }
    }

    #[opaque]
    #[requires(addr != 0)]
    fn cast_addr_header(&self, addr: u64) -> &'a SdtHeader {
        unsafe { &*(addr as *const SdtHeader) }
    }
}

impl<'a> Iterator for XsdtIter<'a> {
    type Item = &'a SdtHeader;

    fn next(&mut self) -> Option<Self::Item> {
        while self.curr < self.count {
            loop_invariant!(self.curr <= self.count);

            let index = self.curr;

            if index < self.count {
                // it's always less than count, but F* is unable to infer that
                let table_addr = self.read_table_entry(index);
                self.curr += 1;

                if table_addr != 0 {
                    return Some(self.cast_addr_header(table_addr));
                }
            } else {
                unreachable!();
            }
        }
        None
    }
}
