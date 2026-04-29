use core::{fmt, mem, slice, str::from_utf8};

use crate::acpi::AcpiTableTrait;
use crate::acpi::zerocopy::error::SizeError;
use crate::impl_table;

use super::FromBytes;
use super::checksum;
use hax_lib::{attributes, ensures, include, opaque, requires};
use mars_getters::unaligned_getters_hax;

impl_table! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct SdtHeader {
        pub sig: [u8; 4],
        pub len: u32,
        pub rev: u8,
        pub checksum: u8,
        pub oem_id: [u8; 6],
        pub oem_table_id: [u8; 8],
        pub oem_rev: u32,
        pub creator_id: u32,
        pub creator_rev: u32,
    }
}

#[attributes]
impl AcpiTableTrait for SdtHeader {
    #[ensures(|result| result.is_ok())]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = match Self::ref_from_prefix(slice) {
            Ok(re) => re,
            // the precondition is that slice >= the size of self
            // the only way that zerocopy would return an err is with misalignment
            // this struct is packed, so that should be impossible
            Err(_) => unreachable!(),
        };

        Ok(reference)
    }
}

impl SdtHeader {
    pub fn signature(&self) -> &str {
        from_utf8(&self.sig).unwrap_or("ERR ")
    }

    pub fn check(&self) -> Result<(), &'static str> {
        if self.compute_checksum() != 0 {
            return Err("incorrect checksum");
        }

        Ok(())
    }

    #[opaque]
    fn compute_checksum(&self) -> u8 {
        checksum(unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, self.len() as usize)
        })
    }

    #[opaque]
    pub fn data_ptr(&self) -> *const u8 {
        unsafe { (self as *const _ as *const u8).add(mem::size_of::<SdtHeader>()) }
    }
}

#[opaque]
impl fmt::Debug for SdtHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let alternate = f.alternate();
        let mut f = f.debug_struct("SdtHeader");
        let f = f.field("Sig", &self.signature()).field("Size", &self.len());

        let f = if alternate {
            let oem_id = &self.oem_id();
            let oem_table_id = &self.oem_table_id();
            let oem_rev = self.oem_rev();

            let oem_id_str = core::str::from_utf8(oem_id.as_slice()).unwrap_or("<???>");
            let oem_table_id_str = core::str::from_utf8(oem_table_id.as_slice()).unwrap_or("<???>");

            f.field("Revision", &self.rev())
                .field("Checksum", &self.checksum())
                .field("OEM ID", &oem_id_str.trim())
                .field("OEM Table ID", &oem_table_id_str.trim())
                .field("OEM Revision", &oem_rev)
        } else {
            f
        };

        f.finish()
    }
}
