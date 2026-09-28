use crate::{dma::DmaBuffer, ring::TransferRing};

pub const MAX_TRANSFER_SIZE: usize = 64 * 1024;
const CONTEXT_COUNT: usize = 32;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum UsbSpeed {
    Full,
    Low,
    High,
    Super,
    SuperPlus,
}

impl UsbSpeed {
    pub fn from_port_speed(speed_id: u8) -> Option<Self> {
        match speed_id {
            1 => Some(Self::Full),
            2 => Some(Self::Low),
            3 => Some(Self::High),
            4 => Some(Self::Super),
            5 => Some(Self::SuperPlus),
            _ => None,
        }
    }

    pub const fn speed_id(self) -> u8 {
        match self {
            Self::Full => 1,
            Self::Low => 2,
            Self::High => 3,
            Self::Super => 4,
            Self::SuperPlus => 5,
        }
    }

    pub const fn default_ep0_max_packet_size(self) -> u16 {
        match self {
            Self::Low | Self::Full => 8,
            Self::High => 64,
            Self::Super | Self::SuperPlus => 512,
        }
    }

    pub const fn ep0_max_packet_size_from_descriptor(self, value: u8) -> Option<u16> {
        match self {
            Self::Low => {
                if value == 8 {
                    Some(8)
                } else {
                    None
                }
            }
            Self::Full => match value {
                8 | 16 | 32 | 64 => Some(value as u16),
                _ => None,
            },
            Self::High => {
                if value == 64 {
                    Some(64)
                } else {
                    None
                }
            }
            Self::Super | Self::SuperPlus => {
                if value == 9 {
                    Some(512)
                } else {
                    None
                }
            }
        }
    }
}

pub struct UsbEndpoint {
    pub address: u8,
    pub dci: u8,
    pub max_packet_size: u16,
    pub ring: TransferRing,
}

impl UsbEndpoint {
    pub fn new(address: u8, max_packet_size: u16) -> Result<Self, &'static str> {
        let endpoint_number = address & 0x0f;
        if endpoint_number == 0
            || address & 0x70 != 0
            || max_packet_size == 0
            || max_packet_size > 1024
        {
            return Err("invalid USB endpoint descriptor");
        }

        let direction_in = address & 0x80 != 0;
        let dci = endpoint_number * 2 + u8::from(direction_in);

        Ok(Self {
            address,
            dci,
            max_packet_size,
            ring: TransferRing::new(64)?,
        })
    }
}

pub struct UsbDevice {
    pub slot_id: u8,
    pub port_id: u8,
    pub speed: UsbSpeed,
    pub ep0_max_packet_size: u16,
    pub(crate) context_stride_words: usize,
    pub(crate) output_context: DmaBuffer<[u32]>,
    pub(crate) input_context: DmaBuffer<[u32]>,
    pub(crate) ep0_ring: TransferRing,
    pub(crate) transfer_buffer: DmaBuffer<[u8]>,
    pub(crate) bulk_in: Option<UsbEndpoint>,
    pub(crate) bulk_out: Option<UsbEndpoint>,
}

impl UsbDevice {
    pub fn new(
        slot_id: u8,
        port_id: u8,
        speed: UsbSpeed,
        context_size: usize,
    ) -> Result<Self, &'static str> {
        if slot_id == 0 || context_size < 32 || !context_size.is_power_of_two() {
            return Err("invalid xHCI device context configuration");
        }

        let context_stride_words = context_size / size_of::<u32>();
        let output_words = context_stride_words
            .checked_mul(CONTEXT_COUNT)
            .ok_or("xHCI output context size overflow")?;
        let input_words = context_stride_words
            .checked_mul(CONTEXT_COUNT + 1)
            .ok_or("xHCI input context size overflow")?;

        Ok(Self {
            slot_id,
            port_id,
            speed,
            ep0_max_packet_size: speed.default_ep0_max_packet_size(),
            context_stride_words,
            output_context: DmaBuffer::new_slice(output_words, 0)?,
            input_context: DmaBuffer::new_slice(input_words, 0)?,
            ep0_ring: TransferRing::new(64)?,
            transfer_buffer: DmaBuffer::new_slice_aligned(MAX_TRANSFER_SIZE, 0, MAX_TRANSFER_SIZE)?,
            bulk_in: None,
            bulk_out: None,
        })
    }

    pub fn set_bulk_endpoints(
        &mut self,
        bulk_in: (u8, u16),
        bulk_out: (u8, u16),
    ) -> Result<(), &'static str> {
        self.bulk_in = Some(UsbEndpoint::new(bulk_in.0, bulk_in.1)?);
        self.bulk_out = Some(UsbEndpoint::new(bulk_out.0, bulk_out.1)?);
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub const fn new(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> Self {
        Self {
            request_type,
            request,
            value,
            index,
            length,
        }
    }

    pub const fn parameter(self) -> u64 {
        (self.request_type as u64)
            | ((self.request as u64) << 8)
            | ((self.value as u64) << 16)
            | ((self.index as u64) << 32)
            | ((self.length as u64) << 48)
    }

    pub const fn direction_in(self) -> bool {
        self.request_type & 0x80 != 0
    }

    pub const fn transfer_type(self) -> u8 {
        if self.length == 0 {
            0
        } else if self.direction_in() {
            3
        } else {
            2
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MassStorageInterface {
    pub configuration_value: u8,
    pub bulk_in_address: u8,
    pub bulk_in_max_packet_size: u16,
    pub bulk_out_address: u8,
    pub bulk_out_max_packet_size: u16,
}

pub fn find_mass_storage_interface(
    configuration: &[u8],
) -> Result<Option<MassStorageInterface>, &'static str> {
    if configuration.len() < 9 || configuration[1] != 2 || configuration[0] < 9 {
        return Err("invalid USB configuration descriptor");
    }

    let total_length = u16::from_le_bytes([configuration[2], configuration[3]]) as usize;
    if total_length < 9 || total_length > configuration.len() {
        return Err("truncated USB configuration descriptor");
    }

    let configuration_value = configuration[5];
    if configuration_value == 0 {
        return Err("invalid USB configuration value");
    }
    let mut current_interface = false;
    let mut bulk_in = None;
    let mut bulk_out = None;
    let mut offset = 0;

    while offset < total_length {
        let length = *configuration
            .get(offset)
            .ok_or("truncated USB descriptor")? as usize;
        let descriptor_type = *configuration
            .get(offset + 1)
            .ok_or("truncated USB descriptor")?;
        if length < 2
            || offset
                .checked_add(length)
                .is_none_or(|end| end > total_length)
        {
            return Err("invalid USB descriptor length");
        }

        let descriptor = &configuration[offset..offset + length];
        match descriptor_type {
            4 if length >= 9 => {
                current_interface = descriptor[3] == 0
                    && descriptor[5] == 0x08
                    && descriptor[6] == 0x06
                    && descriptor[7] == 0x50;
                bulk_in = None;
                bulk_out = None;
            }
            5 if length >= 7 && current_interface => {
                if descriptor[3] & 0x03 == 2 {
                    let address = descriptor[2];
                    let raw_max_packet_size = u16::from_le_bytes([descriptor[4], descriptor[5]]);
                    let max_packet_size = raw_max_packet_size & 0x07ff;
                    if address & 0x70 != 0
                        || address & 0x0f == 0
                        || raw_max_packet_size & 0xf800 != 0
                        || max_packet_size == 0
                        || max_packet_size > 1024
                    {
                        return Err("invalid USB bulk endpoint descriptor");
                    }
                    if address & 0x80 != 0 {
                        if bulk_in.replace((address, max_packet_size)).is_some() {
                            return Err("USB interface has multiple bulk IN endpoints");
                        }
                    } else if bulk_out.replace((address, max_packet_size)).is_some() {
                        return Err("USB interface has multiple bulk OUT endpoints");
                    }
                }
            }
            _ => {}
        }

        if let (
            true,
            Some((bulk_in_address, bulk_in_max_packet_size)),
            Some((bulk_out_address, bulk_out_max_packet_size)),
        ) = (current_interface, bulk_in, bulk_out)
        {
            return Ok(Some(MassStorageInterface {
                configuration_value,
                bulk_in_address,
                bulk_in_max_packet_size,
                bulk_out_address,
                bulk_out_max_packet_size,
            }));
        }

        offset += length;
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{MassStorageInterface, find_mass_storage_interface};

    #[test]
    fn finds_bot_scsi_bulk_endpoints() {
        let configuration = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50, // configuration
            9, 4, 0, 0, 2, 8, 6, 0x50, 0, // BOT SCSI interface
            7, 5, 0x81, 2, 0, 2, 0, // bulk IN, 512-byte packets
            7, 5, 0x02, 2, 0, 2, 0, // bulk OUT, 512-byte packets
        ];

        assert_eq!(
            find_mass_storage_interface(&configuration),
            Ok(Some(MassStorageInterface {
                configuration_value: 1,
                bulk_in_address: 0x81,
                bulk_in_max_packet_size: 512,
                bulk_out_address: 0x02,
                bulk_out_max_packet_size: 512,
            }))
        );
    }

    #[test]
    fn rejects_truncated_configuration_descriptors() {
        let configuration = [9, 2, 32, 0, 1, 1, 0, 0x80, 50];
        assert_eq!(
            find_mass_storage_interface(&configuration),
            Err("truncated USB configuration descriptor")
        );
    }
}
