//! NVMe command encoding and Identify parsing.
#![forbid(unsafe_code)]

/// cmd dwords in the native endianness
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Command {
    pub words: [u32; 16],
}

impl Command {
    pub fn identify(cid: u16, nsid: u32, cns: u8, prp: u64) -> Self {
        let mut command = Self::new(0x06, cid, nsid);
        command.set_prp(prp, 0);
        command.words[10] = cns as u32;
        command
    }

    pub fn create_cq(
        cid: u16,
        qid: u16,
        depth: u32,
        prp: u64,
        vector: Option<u16>,
    ) -> Result<Self, QueueError> {
        validate_queue(qid, depth)?;
        let mut command = Self::new(0x05, cid, 0);
        command.set_prp(prp, 0);
        command.words[10] = qid as u32 | ((depth - 1) << 16);
        // `None` means poll-only CQ.
        command.words[11] =
            1 | (u32::from(vector.is_some()) << 1) | ((vector.unwrap_or(0) as u32) << 16);
        Ok(command)
    }

    pub fn create_sq(
        cid: u16,
        qid: u16,
        depth: u32,
        prp: u64,
        cqid: u16,
    ) -> Result<Self, QueueError> {
        validate_queue(qid, depth)?;
        if cqid == 0 {
            return Err(QueueError::InvalidQueueId);
        }
        let mut command = Self::new(0x01, cid, 0);
        command.set_prp(prp, 0);
        command.words[10] = qid as u32 | ((depth - 1) << 16);
        command.words[11] = 1 | ((cqid as u32) << 16);
        Ok(command)
    }

    /// request one I/O submission queue and one I/O completion queue.
    pub fn set_number_of_queues(cid: u16) -> Self {
        let mut command = Self::new(0x09, cid, 0);
        command.words[10] = 0x07;
        command
    }

    pub fn read_write(
        cid: u16,
        nsid: u32,
        lba: u64,
        blocks: u32,
        prp1: u64,
        prp2: u64,
        write: bool,
    ) -> Result<Self, IoError> {
        validate_nsid(nsid)?;
        if blocks == 0 || blocks > 65536 {
            return Err(IoError::InvalidBlockCount);
        }
        lba.checked_add((blocks - 1) as u64)
            .ok_or(IoError::LbaOverflow)?;
        let mut command = Self::new(if write { 0x01 } else { 0x02 }, cid, nsid);
        command.set_prp(prp1, prp2);
        command.words[10] = lba as u32;
        command.words[11] = (lba >> 32) as u32;
        command.words[12] = blocks - 1;
        Ok(command)
    }

    /// flush one namespace; broadcast NSID isn't accepted
    pub fn flush(cid: u16, nsid: u32) -> Result<Self, IoError> {
        validate_nsid(nsid)?;
        Ok(Self::new(0x00, cid, nsid))
    }

    pub fn to_bytes(self) -> [u8; 64] {
        let mut bytes = [0; 64];
        for (word, chunk) in self.words.iter().zip(bytes.chunks_exact_mut(4)) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    fn new(opcode: u8, cid: u16, nsid: u32) -> Self {
        let mut command = Self::default();
        command.words[0] = opcode as u32 | ((cid as u32) << 16);
        command.words[1] = nsid;
        command
    }

    fn set_prp(&mut self, prp1: u64, prp2: u64) {
        self.words[6] = prp1 as u32;
        self.words[7] = (prp1 >> 32) as u32;
        self.words[8] = prp2 as u32;
        self.words[9] = (prp2 >> 32) as u32;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    InvalidQueueId,
    InvalidDepth,
}

fn validate_queue(qid: u16, depth: u32) -> Result<(), QueueError> {
    if qid == 0 {
        return Err(QueueError::InvalidQueueId);
    }
    if !(2..=65536).contains(&depth) {
        return Err(QueueError::InvalidDepth);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoError {
    InvalidNamespace,
    InvalidBlockCount,
    InvalidBlockSize,
    UnalignedBuffer,
    LbaOverflow,
    OutOfBounds,
}

fn validate_nsid(nsid: u32) -> Result<(), IoError> {
    if nsid == 0 || nsid == u32::MAX {
        return Err(IoError::InvalidNamespace);
    }
    Ok(())
}

/// validate the request before issuing any part of a split transfer.
/// empty requests at the end of the namespace are tolerated
pub fn validate_io(
    lba: u64,
    byte_len: usize,
    block_size: usize,
    block_count: u64,
) -> Result<u64, IoError> {
    if !block_size.is_power_of_two() || !(512..=16384).contains(&block_size) {
        return Err(IoError::InvalidBlockSize);
    }
    if byte_len % block_size != 0 {
        return Err(IoError::UnalignedBuffer);
    }
    let blocks = u64::try_from(byte_len / block_size).map_err(|_| IoError::LbaOverflow)?;
    let end = lba.checked_add(blocks).ok_or(IoError::LbaOverflow)?;
    if end > block_count {
        return Err(IoError::OutOfBounds);
    }
    Ok(blocks)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub result: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub status: u16,
    pub phase: bool,
}

impl Completion {
    pub fn parse(words: [u32; 4]) -> Self {
        let status_word = words[3] >> 16;
        Self {
            result: words[0],
            sq_head: words[2] as u16,
            sq_id: (words[2] >> 16) as u16,
            cid: words[3] as u16,
            status: (status_word >> 1) as u16,
            phase: status_word & 1 != 0,
        }
    }

    pub fn is_success(&self) -> bool {
        // SC and SCT indicate success
        self.status & 0x07ff == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerIdentify {
    pub mdts: u8,
    pub namespace_count: u32,
}

impl ControllerIdentify {
    pub fn parse(bytes: &[u8; 4096]) -> Self {
        Self {
            mdts: bytes[77],
            namespace_count: u32::from_le_bytes(bytes[516..520].try_into().unwrap()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceIdentify {
    pub block_count: u64,
    pub block_size: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamespaceError {
    NoCapacity,
    InvalidCapacity,
    InvalidLbaFormat,
    MetadataUnsupported,
    ProtectionUnsupported,
    InvalidBlockSize,
}

impl NamespaceIdentify {
    /// caller must establish its command set.
    pub fn parse(bytes: &[u8; 4096]) -> Result<Self, NamespaceError> {
        let block_count = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let capacity = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        if block_count == 0 || capacity == 0 {
            return Err(NamespaceError::NoCapacity);
        }
        if capacity > block_count {
            return Err(NamespaceError::InvalidCapacity);
        }
        let flbas = bytes[26];
        let format_index = ((flbas & 0x0f) | ((flbas & 0x60) >> 1)) as usize;
        let highest_format = bytes[25] as usize;
        if flbas & 0x80 != 0 || highest_format >= 64 || format_index > highest_format {
            return Err(NamespaceError::InvalidLbaFormat);
        }
        if bytes[29] & 0x07 != 0 {
            return Err(NamespaceError::ProtectionUnsupported);
        }
        let offset = 128 + format_index * 4;
        let metadata_size = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        if metadata_size != 0 {
            return Err(NamespaceError::MetadataUnsupported);
        }
        let lbads = bytes[offset + 2];
        if !(9..=14).contains(&lbads) {
            return Err(NamespaceError::InvalidBlockSize);
        }
        Ok(Self {
            block_count,
            block_size: 1u32 << lbads,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace() -> [u8; 4096] {
        let mut bytes = [0; 4096];
        bytes[0..8].copy_from_slice(&100u64.to_le_bytes());
        bytes[8..16].copy_from_slice(&80u64.to_le_bytes());
        bytes[130] = 12;
        bytes
    }

    #[test]
    fn identify_encoding_and_reserved_dwords() {
        let command = Command::identify(0x1234, 7, 1, 0x1122_3344_5566_7788);
        assert_eq!(
            command.words,
            [
                0x1234_0006,
                7,
                0,
                0,
                0,
                0,
                0x5566_7788,
                0x1122_3344,
                0,
                0,
                1,
                0,
                0,
                0,
                0,
                0,
            ]
        );
        assert_eq!(&command.to_bytes()[0..4], &[6, 0, 0x34, 0x12]);
        assert_eq!(
            &command.to_bytes()[24..32],
            &[0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
    }

    #[test]
    fn completion_queue_interrupt_enable_is_optional() {
        let polling = Command::create_cq(2, 3, 64, 0x1000, None).unwrap();
        assert_eq!(polling.words[10], (63 << 16) | 3);
        assert_eq!(polling.words[11], 1);
        let interrupt = Command::create_cq(2, 3, 64, 0x1000, Some(5)).unwrap();
        assert_eq!(interrupt.words[11], 0x0005_0003);
        assert_eq!(
            Command::create_cq(2, 3, 64, 0x1000, Some(0)).unwrap().words[11],
            3
        );
    }

    #[test]
    fn submission_queue_cqid_is_in_high_halfword() {
        let sq = Command::create_sq(4, 2, 8, 0x2000, 3).unwrap();
        assert_eq!(sq.words[0], 0x0004_0001);
        assert_eq!(sq.words[10], (7 << 16) | 2);
        assert_eq!(sq.words[11], 0x0003_0001);
    }

    #[test]
    fn queue_depth_limits() {
        for depth in [0, 1, 65537, u32::MAX] {
            assert_eq!(
                Command::create_cq(0, 1, depth, 0, None),
                Err(QueueError::InvalidDepth)
            );
            assert_eq!(
                Command::create_sq(0, 1, depth, 0, 1),
                Err(QueueError::InvalidDepth)
            );
        }
        for depth in [2, 65536] {
            assert_eq!(
                Command::create_cq(0, 1, depth, 0, None).unwrap().words[10] >> 16,
                depth - 1
            );
        }
        assert_eq!(
            Command::create_cq(0, 0, 2, 0, None),
            Err(QueueError::InvalidQueueId)
        );
        assert_eq!(
            Command::create_sq(0, 1, 2, 0, 0),
            Err(QueueError::InvalidQueueId)
        );
    }

    #[test]
    fn queue_count_is_zero_based() {
        let command = Command::set_number_of_queues(5);
        assert_eq!(command.words[0], 0x0005_0009);
        assert_eq!(command.words[10], 7);
        assert_eq!(command.words[11], 0);
    }

    #[test]
    fn read_write_encoding() {
        for write in [false, true] {
            let rw = Command::read_write(1, 9, 0x1_0000_0002, 2, 0x3000, 0x4000, write).unwrap();
            assert_eq!(rw.words[0], 0x10000 | if write { 1 } else { 2 });
            assert_eq!(rw.words[1], 9);
            assert_eq!(&rw.words[6..10], &[0x3000, 0, 0x4000, 0]);
            assert_eq!(&rw.words[10..13], &[2, 1, 1]);
        }
    }

    #[test]
    fn read_write_limits() {
        for blocks in [0, 65537, u32::MAX] {
            assert_eq!(
                Command::read_write(0, 1, 0, blocks, 0, 0, false),
                Err(IoError::InvalidBlockCount)
            );
        }
        assert_eq!(
            Command::read_write(0, 1, 0, 65536, 0, 0, false)
                .unwrap()
                .words[12],
            65535
        );
        assert!(Command::read_write(0, 1, u64::MAX, 1, 0, 0, false).is_ok());
        assert_eq!(
            Command::read_write(0, 1, u64::MAX, 2, 0, 0, false),
            Err(IoError::LbaOverflow)
        );
        for nsid in [0, u32::MAX] {
            assert_eq!(
                Command::read_write(0, nsid, 0, 1, 0, 0, false),
                Err(IoError::InvalidNamespace)
            );
            assert_eq!(Command::flush(0, nsid), Err(IoError::InvalidNamespace));
        }
    }

    #[test]
    fn flush_encoding() {
        let command = Command::flush(0x1234, 9).unwrap();
        assert_eq!(command.words[0], 0x1234_0000);
        assert_eq!(command.words[1], 9);
        assert_eq!(&command.words[2..], &[0; 14]);
    }

    #[test]
    fn request_bounds_before_splitting() {
        assert_eq!(validate_io(98, 8192, 4096, 100), Ok(2));
        assert_eq!(validate_io(99, 8192, 4096, 100), Err(IoError::OutOfBounds));
        assert_eq!(
            validate_io(u64::MAX, 4096, 4096, u64::MAX),
            Err(IoError::LbaOverflow)
        );
        assert_eq!(validate_io(100, 0, 4096, 100), Ok(0));
        assert_eq!(validate_io(101, 0, 4096, 100), Err(IoError::OutOfBounds));
        assert_eq!(validate_io(0, 513, 512, 100), Err(IoError::UnalignedBuffer));
        for size in [0, 256, 513, 32768, usize::MAX] {
            assert_eq!(validate_io(0, 0, size, 100), Err(IoError::InvalidBlockSize));
        }
    }

    #[test]
    fn completion_fields_and_phase() {
        let completion = Completion::parse([0x1234, 0, 0x5678_0009, 0xabcd_0042]);
        assert_eq!(
            completion,
            Completion {
                result: 0x1234,
                sq_head: 9,
                sq_id: 0x5678,
                cid: 0x42,
                status: 0x55e6,
                phase: true,
            }
        );
        assert!(!completion.is_success());
        assert!(!Completion::parse([0, 0, 0, 0]).phase);
        assert!(Completion::parse([0, 0, 0, 0x0001_0000]).is_success());
        assert!(Completion::parse([0, 0, 0, 0xc001_0000]).is_success());
        assert!(!Completion::parse([0, 0, 0, 0x0201_0000]).is_success());
    }

    #[test]
    fn controller_identify_fields() {
        let mut bytes = [0; 4096];
        bytes[77] = 5;
        bytes[516..520].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        assert_eq!(
            ControllerIdentify::parse(&bytes),
            ControllerIdentify {
                mdts: 5,
                namespace_count: 0x1234_5678,
            }
        );
    }

    #[test]
    fn namespace_uses_nsze_not_ncap() {
        assert_eq!(
            NamespaceIdentify::parse(&namespace()),
            Ok(NamespaceIdentify {
                block_count: 100,
                block_size: 4096,
            })
        );
    }

    #[test]
    fn namespace_capacity_validation() {
        let mut bytes = namespace();
        bytes[8..16].fill(0);
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::NoCapacity)
        );
        bytes[8..16].copy_from_slice(&101u64.to_le_bytes());
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::InvalidCapacity)
        );
        bytes[0..8].fill(0);
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::NoCapacity)
        );
    }

    #[test]
    fn namespace_rejects_metadata_and_protection() {
        let mut bytes = namespace();
        bytes[128] = 1;
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::MetadataUnsupported)
        );
        bytes[128] = 0;
        for protection in 1..=7 {
            bytes[29] = protection;
            assert_eq!(
                NamespaceIdentify::parse(&bytes),
                Err(NamespaceError::ProtectionUnsupported)
            );
        }
    }

    #[test]
    fn namespace_format_selection_including_high_bits() {
        let mut bytes = namespace();
        bytes[25] = 63;
        for index in 0..64usize {
            bytes[26] = (index as u8 & 0xf) | ((index as u8 & 0x30) << 1);
            bytes[128 + index * 4 + 2] = 9;
            assert_eq!(NamespaceIdentify::parse(&bytes).unwrap().block_size, 512);
        }
        bytes[25] = 62;
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::InvalidLbaFormat)
        );
        bytes[25] = 64;
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::InvalidLbaFormat)
        );
        bytes[26] = 0x80;
        assert_eq!(
            NamespaceIdentify::parse(&bytes),
            Err(NamespaceError::InvalidLbaFormat)
        );
    }

    #[test]
    fn namespace_block_size_exponents_are_checked_before_shifting() {
        let mut bytes = namespace();
        for exponent in 0..=u8::MAX {
            bytes[130] = exponent;
            let result = NamespaceIdentify::parse(&bytes);
            if (9..=14).contains(&exponent) {
                assert_eq!(result.unwrap().block_size, 1u32 << exponent);
            } else {
                assert_eq!(result, Err(NamespaceError::InvalidBlockSize));
            }
        }
    }
}
