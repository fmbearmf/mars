use core::{fmt, mem, slice, str::from_utf8};

use super::checksum;
use mars_getters::unaligned_getters;
use zerocopy::{FromBytes, Immutable, IntoBytes, Unaligned};

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Clone, Copy, FromBytes, Immutable, Unaligned)]
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

impl SdtHeader {
    pub fn signature(&self) -> &str {
        from_utf8(&self.sig).unwrap_or("ERR ")
    }

    pub fn check(&self) -> Result<(), &'static str> {
        let bytes =
            unsafe { slice::from_raw_parts(self as *const _ as *const u8, self.len() as usize) };

        if checksum(bytes) != 0 {
            return Err("checksum wrong");
        }

        Ok(())
    }

    pub fn data_ptr(&self) -> *const u8 {
        unsafe { (self as *const _ as *const u8).add(mem::size_of::<SdtHeader>()) }
    }
}

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
