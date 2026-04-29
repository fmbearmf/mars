use core::{
    mem,
    slice::{self, Iter},
};

use crate::{acpi::AcpiTableTrait, impl_table};

use super::FromBytes;
use super::header::SdtHeader;
use hax_lib::{attributes, ensures, opaque, requires};
use mars_getters::unaligned_getters_hax;

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct Mcfg {
        pub header: SdtHeader,
        pub reserved: u64,
    }
}

#[attributes]
impl AcpiTableTrait for Mcfg {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct McfgAllocation {
        pub base_addr: u64,
        pub pci_segment_group: u16,
        pub start_bus_num: u8,
        pub end_bus_num: u8,
        pub reserved: u32,
    }
}

#[attributes]
impl Mcfg {
    pub fn allocations(&'static self) -> McfgIter {
        let slice = self.get_allocations_slice();
        McfgIter(slice.iter())
    }

    #[opaque]
    #[requires(self.header().len() as usize >= mem::size_of::<Mcfg>())]
    #[ensures(|result| result.len() == (self.header().len() as usize - mem::size_of::<Mcfg>()) / mem::size_of::<McfgAllocation>())]
    fn get_allocations_slice(&'static self) -> &'static [McfgAllocation] {
        unsafe {
            let header_size = mem::size_of::<Mcfg>();
            let total_len = self.header().len() as usize;

            if total_len < header_size {
                return &[];
            }

            let data_ptr = (self as *const _ as *const u8).add(header_size);
            let count = (total_len - header_size) / mem::size_of::<McfgAllocation>();

            slice::from_raw_parts(data_ptr as *const McfgAllocation, count)
        }
    }
}

pub struct McfgIter(Iter<'static, McfgAllocation>);

impl Iterator for McfgIter {
    type Item = &'static McfgAllocation;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
