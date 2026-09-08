// hah. turb. trub.

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum TrbType {
    Normal = 1,
    SetupStage = 2,
    DataStage = 3,
    StatusStage = 4,
    Isoch = 5,
    Link = 6,
    EventData = 7,
    NoOp = 8,
    EnableSlotCommand = 9,
    DisableSlotCommand = 10,
    AddressDeviceCommand = 11,
    ConfigureEndpointCommand = 12,
    EvaluateContextCommand = 13,
    ResetEndpointCommand = 14,
    StopEndpointCommand = 15,
    SetTrDequeuePointerCommand = 16,
    ResetDeviceCommand = 17,
    NoOpCommand = 23,
    TransferEvent = 32,
    CommandCompletionEvent = 33,
    PortStatusChangeEvent = 34,
    BandwidthRequestEvent = 35,
    DoorbellEvent = 36,
    HostControllerEvent = 37,
    DeviceNotificationEvent = 38,
    MfindexWrapEvent = 39,
}

impl TrbType {
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(Self::Normal),
            2 => Some(Self::SetupStage),
            3 => Some(Self::DataStage),
            4 => Some(Self::StatusStage),
            5 => Some(Self::Isoch),
            6 => Some(Self::Link),
            7 => Some(Self::EventData),
            8 => Some(Self::NoOp),
            9 => Some(Self::EnableSlotCommand),
            10 => Some(Self::DisableSlotCommand),
            11 => Some(Self::AddressDeviceCommand),
            12 => Some(Self::ConfigureEndpointCommand),
            13 => Some(Self::EvaluateContextCommand),
            14 => Some(Self::ResetEndpointCommand),
            15 => Some(Self::StopEndpointCommand),
            16 => Some(Self::SetTrDequeuePointerCommand),
            17 => Some(Self::ResetDeviceCommand),
            23 => Some(Self::NoOpCommand),
            32 => Some(Self::TransferEvent),
            33 => Some(Self::CommandCompletionEvent),
            34 => Some(Self::PortStatusChangeEvent),
            35 => Some(Self::BandwidthRequestEvent),
            36 => Some(Self::DoorbellEvent),
            37 => Some(Self::HostControllerEvent),
            38 => Some(Self::DeviceNotificationEvent),
            39 => Some(Self::MfindexWrapEvent),
            _ => None,
        }
    }
}

/// 16-byte Transfer Request Block
#[derive(Debug, Copy, Clone)]
#[repr(C, align(16))]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn empty() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: 0,
        }
    }

    pub const fn link(next_ring_phys: u64, toggle_cycle: bool) -> Self {
        let cycle_bit = if toggle_cycle { 1 << 1 } else { 0 };
        Self {
            parameter: next_ring_phys,
            status: 0,
            control: ((TrbType::Link as u32) << 10) | cycle_bit, // bit 1 is Toggle Cycle
        }
    }

    pub const fn command(cmd_type: TrbType) -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: ((cmd_type as u32) << 10),
        }
    }

    #[inline]
    pub fn is_cycle_state(&self, state: bool) -> bool {
        let cycle = (self.control & 1) != 0;
        cycle == state
    }

    #[inline]
    pub fn raw_trb_type(&self) -> u8 {
        ((self.control >> 10) & 0x3F) as u8
    }

    #[inline]
    pub fn trb_type(&self) -> Option<TrbType> {
        TrbType::from_u8(self.raw_trb_type())
    }

    #[inline]
    pub fn completion_code(&self) -> u8 {
        ((self.status >> 24) & 0xFF) as u8
    }

    #[inline]
    pub fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xFF) as u8
    }
}
