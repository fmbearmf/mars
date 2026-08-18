use crate::{address::Bdf, ecam::Ecam};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StandardCapabilityId {
    PowerManagement = 0x01,
    Agp = 0x02,
    Vpd = 0x03,
    SlotId = 0x04,
    Msi = 0x05,
    CompactPciHotSwap = 0x06,
    PciX = 0x07,
    HyperTransport = 0x08,
    VendorSpecific = 0x09,
    Debug = 0x0A,
    CompactPciCentralResource = 0x0B,
    PciHotPlug = 0x0C,
    PciBridgeSubsystemVendor = 0x0D,
    Agp8x = 0x0E,
    SecureDevice = 0x0F,
    PciExpress = 0x10,
    MsiX = 0x11,
    Sata = 0x12,
    AdvancedFeatures = 0x13,
    EnhancedAllocation = 0x14,
    Unknown(u8),
}

impl From<u8> for StandardCapabilityId {
    fn from(val: u8) -> Self {
        match val {
            0x01 => Self::PowerManagement,
            0x02 => Self::Agp,
            0x03 => Self::Vpd,
            0x04 => Self::SlotId,
            0x05 => Self::Msi,
            0x06 => Self::CompactPciHotSwap,
            0x07 => Self::PciX,
            0x08 => Self::HyperTransport,
            0x09 => Self::VendorSpecific,
            0x0A => Self::Debug,
            0x0B => Self::CompactPciCentralResource,
            0x0C => Self::PciHotPlug,
            0x0D => Self::PciBridgeSubsystemVendor,
            0x0E => Self::Agp8x,
            0x0F => Self::SecureDevice,
            0x10 => Self::PciExpress,
            0x11 => Self::MsiX,
            0x12 => Self::Sata,
            0x13 => Self::AdvancedFeatures,
            0x14 => Self::EnhancedAllocation,
            other => Self::Unknown(other),
        }
    }
}

pub struct StandardCapIter<'a> {
    ecam: &'a Ecam,
    bdf: Bdf,
    next_ptr: u8,
    visited: u64, // 64-bit bitset tracking visited byte offsets / 4
}

impl<'a> StandardCapIter<'a> {
    pub fn new(ecam: &'a Ecam, bdf: Bdf) -> Self {
        let status = ecam.read_u16(bdf, 0x06);
        let next_ptr = if (status & (1 << 4)) != 0 {
            ecam.read_u8(bdf, 0x34) & 0xFC
        } else {
            0
        };

        Self {
            ecam,
            bdf,
            next_ptr,
            visited: 0,
        }
    }
}

impl<'a> Iterator for StandardCapIter<'a> {
    type Item = (StandardCapabilityId, u8);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next_ptr;
        if current < 0x40 || current > 0xFC {
            return None;
        }

        // infinite loop
        let bit = 1u64 << ((current / 4) & 63);
        if (self.visited & bit) != 0 {
            return None;
        }
        self.visited |= bit;

        let cap_id = self.ecam.read_u8(self.bdf, current as u16);
        self.next_ptr = self.ecam.read_u8(self.bdf, current as u16 + 1) & 0xFC;

        Some((StandardCapabilityId::from(cap_id), current))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PowerManagementCap {
    pub offset: u8,
    pub pme_support: u16,
    pub d1_support: bool,
    pub d2_support: bool,
}

impl PowerManagementCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u8) -> Self {
        let pmc = ecam.read_u16(bdf, offset as u16 + 2);
        Self {
            offset,
            pme_support: (pmc >> 11) & 0x1F,
            d1_support: (pmc & (1 << 9)) != 0,
            d2_support: (pmc & (1 << 10)) != 0,
        }
    }

    pub fn set_power_state(&self, ecam: &Ecam, bdf: Bdf, state: u8) {
        let pmcsr = ecam.read_u16(bdf, self.offset as u16 + 4);
        let new_pmcsr = (pmcsr & !0x03) | (state as u16 & 0x03);
        ecam.write_u16(bdf, self.offset as u16 + 4, new_pmcsr);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MsiCap {
    pub offset: u8,
    pub is_64bit: bool,
    pub per_vector_masking: bool,
    pub multiple_message_capable: u8,
}

impl MsiCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u8) -> Self {
        let msg_ctrl = ecam.read_u16(bdf, offset as u16 + 2);
        Self {
            offset,
            is_64bit: (msg_ctrl & (1 << 7)) != 0,
            per_vector_masking: (msg_ctrl & (1 << 8)) != 0,
            multiple_message_capable: 1 << ((msg_ctrl >> 1) & 0x07),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MsixCap {
    pub offset: u8,
    pub table_size: u16,
    pub table_bir: u8,
    pub table_offset: u32,
    pub pba_bir: u8,
    pub pba_offset: u32,
}

impl MsixCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u8) -> Self {
        let msg_ctrl = ecam.read_u16(bdf, offset as u16 + 2);
        let table_raw = ecam.read_u32(bdf, offset as u16 + 4);
        let pba_raw = ecam.read_u32(bdf, offset as u16 + 8);

        Self {
            offset,
            table_size: (msg_ctrl & 0x07FF) + 1,
            table_bir: (table_raw & 0x07) as u8,
            table_offset: table_raw & !0x07,
            pba_bir: (pba_raw & 0x07) as u8,
            pba_offset: pba_raw & !0x07,
        }
    }

    pub fn enable(&self, ecam: &Ecam, bdf: Bdf) {
        let msg_ctrl = ecam.read_u16(bdf, self.offset as u16 + 2);
        // set MSI-X enable (15) and clear the function mask (14)
        ecam.write_u16(
            bdf,
            self.offset as u16 + 2,
            (msg_ctrl | (1 << 15)) & !(1 << 14),
        );
    }

    pub fn disable(&self, ecam: &Ecam, bdf: Bdf) {
        let msg_ctrl = ecam.read_u16(bdf, self.offset as u16 + 2);
        ecam.write_u16(bdf, self.offset as u16 + 2, msg_ctrl & !(1 << 15));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PcieCap {
    pub offset: u8,
    pub device_type: u8,
    pub max_payload_size_bytes: usize,
    pub max_read_request_bytes: usize,
    pub current_link_speed_gen: u8,
    pub current_link_width: u8,
}

impl PcieCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u8) -> Self {
        let pcie_flags = ecam.read_u16(bdf, offset as u16 + 2);
        let dev_ctl = ecam.read_u16(bdf, offset as u16 + 8);
        let link_status = ecam.read_u16(bdf, offset as u16 + 18);

        let mps_code = ((dev_ctl >> 5) & 0x07) as usize;
        let mrrs_code = ((dev_ctl >> 12) & 0x07) as usize;

        Self {
            offset,
            device_type: ((pcie_flags >> 4) & 0x0F) as u8,
            max_payload_size_bytes: 128 << mps_code,
            max_read_request_bytes: 128 << mrrs_code,
            current_link_speed_gen: (link_status & 0x0F) as u8,
            current_link_width: ((link_status >> 4) & 0x3F) as u8,
        }
    }
}
