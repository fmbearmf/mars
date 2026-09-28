use alloc::vec;
use klib::block::{BlockDevice, BlockError, Result as BlockResult};

use crate::{
    controller::HostController,
    usb::{MassStorageInterface, SetupPacket, UsbDevice, find_mass_storage_interface},
};

const MAX_CONFIGURATION_DESCRIPTOR_SIZE: usize = 4096;
const BOT_CBW_SIGNATURE: u32 = 0x4342_5355;
const BOT_CSW_SIGNATURE: u32 = 0x5342_5355;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_CAPACITY_16: u8 = 0x9e;
const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2a;
const SCSI_READ_16: u8 = 0x88;
const SCSI_WRITE_16: u8 = 0x8a;

pub struct MassStorage {
    controller_index: usize,
    device: UsbDevice,
    bulk_in_address: u8,
    bulk_out_address: u8,
    block_size: usize,
    block_count: u64,
    next_tag: u32,
}

enum DataPhase<'a> {
    None,
    In(&'a mut [u8]),
    Out(&'a [u8]),
}

enum BlockBuffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

impl MassStorage {
    pub fn probe(
        host: &mut HostController,
        controller_index: usize,
        mut device: UsbDevice,
    ) -> Result<Option<Self>, &'static str> {
        let slot_id = device.slot_id;
        let interface = match find_storage_interface(host, &mut device) {
            Ok(Some(interface)) => interface,
            Ok(None) => {
                host.disable_slot(slot_id)?;
                return Ok(None);
            }
            Err(error) => {
                let _ = host.disable_slot(slot_id);
                return Err(error);
            }
        };

        let packet_size_is_valid = |packet_size| match device.speed {
            crate::usb::UsbSpeed::Low => false,
            crate::usb::UsbSpeed::Full => matches!(packet_size, 8 | 16 | 32 | 64),
            crate::usb::UsbSpeed::High => packet_size == 512,
            crate::usb::UsbSpeed::Super | crate::usb::UsbSpeed::SuperPlus => packet_size == 1024,
        };
        if !packet_size_is_valid(interface.bulk_in_max_packet_size)
            || !packet_size_is_valid(interface.bulk_out_max_packet_size)
        {
            let _ = host.disable_slot(slot_id);
            return Err("USB bulk endpoint packet size is invalid for its speed");
        }

        if let Err(error) = device.set_bulk_endpoints(
            (interface.bulk_in_address, interface.bulk_in_max_packet_size),
            (
                interface.bulk_out_address,
                interface.bulk_out_max_packet_size,
            ),
        ) {
            let _ = host.disable_slot(slot_id);
            return Err(error);
        }

        let set_configuration =
            SetupPacket::new(0x00, 9, u16::from(interface.configuration_value), 0, 0);
        if let Err(error) = host.control_transfer(&mut device, set_configuration, &mut []) {
            let _ = host.disable_slot(slot_id);
            return Err(error);
        }
        if let Err(error) = host.configure_bulk_endpoints(&mut device) {
            let _ = host.disable_slot(slot_id);
            return Err(error);
        }

        let mut storage = Self {
            controller_index,
            device,
            bulk_in_address: interface.bulk_in_address,
            bulk_out_address: interface.bulk_out_address,
            block_size: 0,
            block_count: 0,
            next_tag: 1,
        };
        if let Err(error) = storage.initialize(host) {
            let _ = host.disable_slot(slot_id);
            return Err(error);
        }

        log::info!(
            "xhci: USB mass storage slot {} has {} blocks of {} bytes",
            slot_id,
            storage.block_count,
            storage.block_size
        );
        Ok(Some(storage))
    }

    fn initialize(&mut self, host: &mut HostController) -> Result<(), &'static str> {
        let inquiry_cdb = [0x12, 0, 0, 0, 36, 0];
        let mut inquiry = [0u8; 36];
        self.execute(host, &inquiry_cdb, DataPhase::In(&mut inquiry))?;

        let test_unit_ready = [0u8; 6];
        for attempt in 0..3 {
            if self
                .execute(host, &test_unit_ready, DataPhase::None)
                .is_ok()
            {
                break;
            }

            let request_sense = [0x03, 0, 0, 0, 18, 0];
            let mut sense = [0u8; 18];
            let _ = self.execute(host, &request_sense, DataPhase::In(&mut sense));
            if attempt == 2 {
                return Err("USB mass-storage medium is not ready");
            }
        }

        let capacity_cdb = [SCSI_READ_CAPACITY_10, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut capacity = [0u8; 8];
        if self.execute(host, &capacity_cdb, DataPhase::In(&mut capacity))? != capacity.len() {
            return Err("short SCSI READ CAPACITY response");
        }
        let last_lba = u32::from_be_bytes(capacity[..4].try_into().unwrap());
        let block_size = u32::from_be_bytes(capacity[4..].try_into().unwrap());

        let (last_lba, block_size) = if last_lba == u32::MAX {
            let mut cdb = [0u8; 16];
            cdb[0] = SCSI_READ_CAPACITY_16;
            cdb[1] = 0x10;
            cdb[10..14].copy_from_slice(&32u32.to_be_bytes());
            let mut response = [0u8; 32];
            if self.execute(host, &cdb, DataPhase::In(&mut response))? < 12 {
                return Err("short SCSI READ CAPACITY(16) response");
            }
            (
                u64::from_be_bytes(response[..8].try_into().unwrap()),
                u32::from_be_bytes(response[8..12].try_into().unwrap()),
            )
        } else {
            (u64::from(last_lba), block_size)
        };

        let block_size = usize::try_from(block_size).map_err(|_| "invalid SCSI block size")?;
        if block_size == 0 || block_size > crate::usb::MAX_TRANSFER_SIZE {
            return Err("unsupported SCSI block size");
        }

        self.block_size = block_size;
        self.block_count = last_lba.checked_add(1).ok_or("invalid SCSI block count")?;
        if self.block_count == 0 {
            return Err("USB mass-storage device has no blocks");
        }
        Ok(())
    }

    fn execute(
        &mut self,
        host: &mut HostController,
        cdb: &[u8],
        data: DataPhase<'_>,
    ) -> Result<usize, &'static str> {
        if cdb.is_empty() || cdb.len() > 16 {
            return Err("invalid SCSI command length");
        }

        let transfer_length = match &data {
            DataPhase::None => 0,
            DataPhase::In(buffer) => buffer.len(),
            DataPhase::Out(buffer) => buffer.len(),
        };
        if transfer_length > crate::usb::MAX_TRANSFER_SIZE {
            return Err("USB mass-storage transfer is too large");
        }

        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1);
        if self.next_tag == 0 {
            self.next_tag = 1;
        }

        let mut cbw = [0u8; 31];
        cbw[..4].copy_from_slice(&BOT_CBW_SIGNATURE.to_le_bytes());
        cbw[4..8].copy_from_slice(&tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&(transfer_length as u32).to_le_bytes());
        if matches!(&data, DataPhase::In(_)) {
            cbw[12] = 0x80;
        }
        cbw[13] = 0;
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        host.bulk_out(&mut self.device, self.bulk_out_address, &cbw)?;

        let data_transferred = match data {
            DataPhase::None => 0,
            DataPhase::In(buffer) => {
                host.bulk_in(&mut self.device, self.bulk_in_address, buffer)?
            }
            DataPhase::Out(buffer) => {
                host.bulk_out(&mut self.device, self.bulk_out_address, buffer)?;
                buffer.len()
            }
        };

        let mut csw = [0u8; 13];
        if host.bulk_in(&mut self.device, self.bulk_in_address, &mut csw)? != csw.len() {
            return Err("short USB mass-storage status wrapper");
        }
        let signature = u32::from_le_bytes(csw[..4].try_into().unwrap());
        let returned_tag = u32::from_le_bytes(csw[4..8].try_into().unwrap());
        let residue = u32::from_le_bytes(csw[8..12].try_into().unwrap()) as usize;
        if signature != BOT_CSW_SIGNATURE || returned_tag != tag || residue > transfer_length {
            return Err("invalid USB mass-storage status wrapper");
        }
        match csw[12] {
            0 if residue.checked_add(data_transferred) == Some(transfer_length) => {
                Ok(data_transferred)
            }
            0 => Err("inconsistent USB mass-storage residue"),
            1 => Err("SCSI command failed"),
            _ => Err("USB mass-storage phase error"),
        }
    }

    fn transfer_blocks(
        &mut self,
        host: &mut HostController,
        lba: u64,
        mut buffer: BlockBuffer<'_>,
    ) -> BlockResult<()> {
        let buffer_len = match &buffer {
            BlockBuffer::Read(bytes) => bytes.len(),
            BlockBuffer::Write(bytes) => bytes.len(),
        };
        if self.block_size == 0 {
            return Err(BlockError::InvalidBlockSize);
        }
        if buffer_len % self.block_size != 0 {
            return Err(BlockError::UnalignedBuffer);
        }

        let blocks = (buffer_len / self.block_size) as u64;
        let end = lba.checked_add(blocks).ok_or(BlockError::OutOfBounds)?;
        if lba > self.block_count || end > self.block_count {
            return Err(BlockError::OutOfBounds);
        }
        if blocks == 0 {
            return Ok(());
        }

        let blocks_per_command = (crate::usb::MAX_TRANSFER_SIZE / self.block_size).max(1);
        let mut current_lba = lba;
        let mut remaining = blocks;
        let mut byte_offset = 0;

        while remaining != 0 {
            let command_blocks = remaining.min(blocks_per_command as u64);
            let command_bytes = command_blocks as usize * self.block_size;
            let (cdb, cdb_length) = rw_cdb(
                current_lba,
                command_blocks as u32,
                matches!(&buffer, BlockBuffer::Write(_)),
            );
            let command_result = match &mut buffer {
                BlockBuffer::Read(bytes) => self.execute(
                    host,
                    &cdb[..cdb_length],
                    DataPhase::In(&mut bytes[byte_offset..byte_offset + command_bytes]),
                ),
                BlockBuffer::Write(bytes) => self.execute(
                    host,
                    &cdb[..cdb_length],
                    DataPhase::Out(&bytes[byte_offset..byte_offset + command_bytes]),
                ),
            };
            if command_result.map_err(|_| BlockError::HardwareError)? != command_bytes {
                return Err(BlockError::HardwareError);
            }

            current_lba = current_lba
                .checked_add(command_blocks)
                .ok_or(BlockError::OutOfBounds)?;
            remaining -= command_blocks;
            byte_offset += command_bytes;
        }

        Ok(())
    }
}

fn find_storage_interface(
    host: &mut HostController,
    device: &mut UsbDevice,
) -> Result<Option<MassStorageInterface>, &'static str> {
    let get_device_descriptor = SetupPacket::new(0x80, 6, 0x0100, 0, 8);
    let mut first_bytes = [0u8; 8];
    let received = host.control_transfer(device, get_device_descriptor, &mut first_bytes)?;
    if received != first_bytes.len() || first_bytes[0] < 18 || first_bytes[1] != 1 {
        return Err("invalid USB device descriptor");
    }

    let max_packet_size = device
        .speed
        .ep0_max_packet_size_from_descriptor(first_bytes[7])
        .ok_or("invalid USB control endpoint max packet size")?;
    if max_packet_size != device.ep0_max_packet_size {
        host.evaluate_ep0_max_packet_size(device, max_packet_size)?;
    }

    let get_full_device_descriptor = SetupPacket::new(0x80, 6, 0x0100, 0, 18);
    let mut device_descriptor = [0u8; 18];
    let received =
        host.control_transfer(device, get_full_device_descriptor, &mut device_descriptor)?;
    if received < device_descriptor.len()
        || device_descriptor[0] < device_descriptor.len() as u8
        || device_descriptor[1] != 1
    {
        return Err("invalid USB device descriptor");
    }
    if device_descriptor[17] == 0 {
        return Ok(None);
    }

    let get_configuration_header = SetupPacket::new(0x80, 6, 0x0200, 0, 9);
    let mut header = [0u8; 9];
    let received = host.control_transfer(device, get_configuration_header, &mut header)?;
    if received < header.len() || header[1] != 2 {
        return Err("invalid USB configuration descriptor");
    }

    let total_length = u16::from_le_bytes([header[2], header[3]]) as usize;
    if !(9..=MAX_CONFIGURATION_DESCRIPTOR_SIZE).contains(&total_length) {
        return Err("unsupported USB configuration descriptor size");
    }

    let get_configuration = SetupPacket::new(0x80, 6, 0x0200, 0, total_length as u16);
    let mut configuration = vec![0u8; total_length];
    let received = host.control_transfer(device, get_configuration, &mut configuration)?;
    if received < total_length {
        return Err("truncated USB configuration descriptor");
    }

    find_mass_storage_interface(&configuration)
}

fn rw_cdb(lba: u64, blocks: u32, write: bool) -> ([u8; 16], usize) {
    let mut cdb = [0u8; 16];
    let end_lba = lba.checked_add(u64::from(blocks));
    if blocks <= u32::from(u16::MAX) && end_lba.is_some_and(|end| end <= u64::from(u32::MAX) + 1) {
        cdb[0] = if write { SCSI_WRITE_10 } else { SCSI_READ_10 };
        cdb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        cdb[7..9].copy_from_slice(&(blocks as u16).to_be_bytes());
        (cdb, 10)
    } else {
        cdb[0] = if write { SCSI_WRITE_16 } else { SCSI_READ_16 };
        cdb[2..10].copy_from_slice(&lba.to_be_bytes());
        cdb[10..14].copy_from_slice(&blocks.to_be_bytes());
        (cdb, 16)
    }
}

impl BlockDevice for MassStorage {
    fn flush(&mut self) -> BlockResult<()> {
        let controller_index = self.controller_index;
        let result = crate::driver::with_controller(controller_index, |host| {
            let cdb = [0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            self.execute(host, &cdb, DataPhase::None)
        });
        match result {
            Some(Ok(_)) => Ok(()),
            Some(Err(_)) => Err(BlockError::HardwareError),
            None => Err(BlockError::NotReady),
        }
    }

    fn read_blocks(&mut self, lba: u64, buffer: &mut [u8]) -> BlockResult<()> {
        let controller_index = self.controller_index;
        let result = crate::driver::with_controller(controller_index, |host| {
            self.transfer_blocks(host, lba, BlockBuffer::Read(buffer))
        });
        result.unwrap_or(Err(BlockError::NotReady))
    }

    fn write_blocks(&mut self, lba: u64, buffer: &[u8]) -> BlockResult<()> {
        let controller_index = self.controller_index;
        let result = crate::driver::with_controller(controller_index, |host| {
            self.transfer_blocks(host, lba, BlockBuffer::Write(buffer))
        });
        result.unwrap_or(Err(BlockError::NotReady))
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }
}

#[cfg(test)]
mod tests {
    use super::{SCSI_READ_10, SCSI_READ_16, SCSI_WRITE_16, rw_cdb};

    #[test]
    fn uses_read10_when_the_request_fits() {
        let (cdb, length) = rw_cdb(0x1234_5678, 128, false);

        assert_eq!(length, 10);
        assert_eq!(cdb[0], SCSI_READ_10);
        assert_eq!(&cdb[2..6], &0x1234_5678u32.to_be_bytes());
        assert_eq!(&cdb[7..9], &128u16.to_be_bytes());
    }

    #[test]
    fn uses_read16_when_a_request_crosses_the_read10_lba_limit() {
        let (cdb, length) = rw_cdb(u64::from(u32::MAX), 2, false);

        assert_eq!(length, 16);
        assert_eq!(cdb[0], SCSI_READ_16);
        assert_eq!(&cdb[2..10], &u64::from(u32::MAX).to_be_bytes());
        assert_eq!(&cdb[10..14], &2u32.to_be_bytes());
    }

    #[test]
    fn uses_write16_for_large_lbas() {
        let (cdb, length) = rw_cdb(u64::from(u32::MAX) + 1, 1, true);

        assert_eq!(length, 16);
        assert_eq!(cdb[0], SCSI_WRITE_16);
    }
}
