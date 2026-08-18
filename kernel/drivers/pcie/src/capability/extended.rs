use crate::{address::Bdf, ecam::Ecam};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ExtendedCapabilityId {
    AdvancedErrorReporting = 0x0001,
    VirtualChannelNoMfvc = 0x0002,
    DeviceSerialNumber = 0x0003,
    PowerBudgeting = 0x0004,
    RootComplexLinkDeclaration = 0x0005,
    RootComplexInternalLinkControl = 0x0006,
    RootComplexEventCollectorEndpoint = 0x0007,
    MultiFunctionVirtualChannel = 0x0008,
    VirtualChannel = 0x0009,
    RcrbHeader = 0x000A,
    VendorSpecific = 0x000B,
    Acs = 0x000D,
    Ari = 0x000E,
    Ats = 0x000F,
    SingleRootIoVirt = 0x0010,
    MultiRootIoVirt = 0x0011,
    PageRequestInterface = 0x0013,
    ResizableBar = 0x0015,
    DynamicPowerAllocation = 0x0016,
    TphRequester = 0x0017,
    LatencyToleranceReporting = 0x0018,
    SecondaryPciExpress = 0x0019,
    DownstreamPortContainment = 0x001D,
    PrecisionTimeMeasurement = 0x001F,
    Unknown(u16),
}

impl From<u16> for ExtendedCapabilityId {
    fn from(val: u16) -> Self {
        match val {
            0x0001 => Self::AdvancedErrorReporting,
            0x0002 => Self::VirtualChannelNoMfvc,
            0x0003 => Self::DeviceSerialNumber,
            0x0004 => Self::PowerBudgeting,
            0x0005 => Self::RootComplexLinkDeclaration,
            0x0006 => Self::RootComplexInternalLinkControl,
            0x0007 => Self::RootComplexEventCollectorEndpoint,
            0x0008 => Self::MultiFunctionVirtualChannel,
            0x0009 => Self::VirtualChannel,
            0x000A => Self::RcrbHeader,
            0x000B => Self::VendorSpecific,
            0x000D => Self::Acs,
            0x000E => Self::Ari,
            0x000F => Self::Ats,
            0x0010 => Self::SingleRootIoVirt,
            0x0011 => Self::MultiRootIoVirt,
            0x0013 => Self::PageRequestInterface,
            0x0015 => Self::ResizableBar,
            0x0016 => Self::DynamicPowerAllocation,
            0x0017 => Self::TphRequester,
            0x0018 => Self::LatencyToleranceReporting,
            0x0019 => Self::SecondaryPciExpress,
            0x001D => Self::DownstreamPortContainment,
            0x001F => Self::PrecisionTimeMeasurement,
            other => Self::Unknown(other),
        }
    }
}

/// (0x100..0xFFF)
pub struct ExtendedCapIter<'a> {
    ecam: &'a Ecam,
    bdf: Bdf,
    next_ptr: u16,
    count: usize,
}

impl<'a> ExtendedCapIter<'a> {
    pub fn new(ecam: &'a Ecam, bdf: Bdf) -> Self {
        Self {
            ecam,
            bdf,
            next_ptr: 0x100,
            count: 0,
        }
    }
}

impl<'a> Iterator for ExtendedCapIter<'a> {
    type Item = (ExtendedCapabilityId, u16);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next_ptr;
        if current < 0x100 || current > 0xFFC || self.count >= 64 {
            return None;
        }

        let header = self.ecam.read_u32(self.bdf, current);
        if header == 0 || header == 0xFFFF_FFFF {
            return None;
        }

        let cap_id = (header & 0xFFFF) as u16;
        let next_offset = ((header >> 20) & 0xFFF) as u16;

        self.next_ptr = next_offset;
        self.count += 1;

        Some((ExtendedCapabilityId::from(cap_id), current))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AerCap {
    pub offset: u16,
    pub uncorrectable_status: u32,
    pub correctable_status: u32,
}

impl AerCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u16) -> Self {
        Self {
            offset,
            uncorrectable_status: ecam.read_u32(bdf, offset + 0x04),
            correctable_status: ecam.read_u32(bdf, offset + 0x10),
        }
    }

    pub fn clear_errors(&self, ecam: &Ecam, bdf: Bdf) {
        let uncorr = ecam.read_u32(bdf, self.offset + 0x04);
        ecam.write_u32(bdf, self.offset + 0x04, uncorr);

        let corr = ecam.read_u32(bdf, self.offset + 0x10);
        ecam.write_u32(bdf, self.offset + 0x10, corr);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SriovCap {
    pub offset: u16,
    pub initial_vfs: u16,
    pub total_vfs: u16,
    pub num_vfs: u16,
    pub vf_stride: u16,
    pub vf_device_id: u16,
}

impl SriovCap {
    pub fn read(ecam: &Ecam, bdf: Bdf, offset: u16) -> Self {
        Self {
            offset,
            initial_vfs: ecam.read_u16(bdf, offset + 0x0C),
            total_vfs: ecam.read_u16(bdf, offset + 0x0E),
            num_vfs: ecam.read_u16(bdf, offset + 0x10),
            vf_stride: ecam.read_u16(bdf, offset + 0x16),
            vf_device_id: ecam.read_u16(bdf, offset + 0x1A),
        }
    }

    pub fn enable_vfs(&self, ecam: &Ecam, bdf: Bdf, num_vfs: u16) {
        ecam.write_u16(bdf, self.offset + 0x10, num_vfs);
        let ctrl = ecam.read_u16(bdf, self.offset + 0x08);
        ecam.write_u16(bdf, self.offset + 0x08, ctrl | 0x01); // VF enable bit
    }
}
