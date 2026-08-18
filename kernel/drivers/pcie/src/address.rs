use core::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bdf {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Bdf {
    pub const fn new(segment: u16, bus: u8, device: u8, function: u8) -> Self {
        Self {
            segment,
            bus,
            device,
            function,
        }
    }

    /// the PCIe requester ID used for TLP routing
    pub const fn requester_id(&self) -> u32 {
        ((self.bus as u32) << 8) | ((self.device as u32) << 3) | (self.function as u32)
    }

    /// the DeviceID
    pub const fn device_id(&self) -> u32 {
        ((self.segment as u32) << 16) | (self.requester_id() as u32)
    }
}

impl fmt::Display for Bdf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "{:04x}:{:02x}:{:02x}.{}",
            self.segment, self.bus, self.device, self.function
        ))
    }
}
