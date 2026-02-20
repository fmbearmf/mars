use core::{fmt, mem, ptr, slice, str::from_utf8};

use super::checksum;
use getters::unaligned_getters;

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Clone, Copy)]
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
        f.debug_struct("SdtHeader")
            .field("Sig", &self.signature())
            .field("Len", &self.len())
            .finish()
    }
}
