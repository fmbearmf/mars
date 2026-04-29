use crate::acpi::AcpiTableTrait;
use crate::impl_table;

use super::FromBytes;
use super::header::SdtHeader;
use hax_lib::{attributes, ensures, opaque, requires};
use mars_getters::unaligned_getters;

impl_table! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Gtdt {
        pub header: SdtHeader,
        pub cnt_control_base: u64,
        pub reserved: u32,
        pub secure_el1_gsiv: u32,
        pub secure_el1_flags: u32,
        pub ns_el1_gsiv: u32,
        pub ns_el1_flags: u32,
        pub virt_el1_gsiv: u32,
        pub virt_el1_flags: u32,
        pub ns_el2_gsiv: u32,
        pub ns_el2_flags: u32,
        pub cnt_read_base: u64,
        pub platform_timer_count: u32,
        pub platform_timer_offset: u32,
    }
}

#[attributes]
impl AcpiTableTrait for Gtdt {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}
