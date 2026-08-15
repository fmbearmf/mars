use core::fmt::Display;

use alloc::{boxed::Box, string::String, sync::Arc};

use crate::{scheduler::GLOBAL_SCHEDULER, sync::SleepingMutex};

/// errors that can occur during block I/O
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// requested op goes past device capacity
    OutOfBounds,
    /// buffer len isn't a multiple of the block size
    UnalignedBuffer,
    HardwareError,
    /// tried to write to a read-only device
    ReadOnly,
    /// device isn't ready
    NotReady,
    /// lock requested but the device is being used
    InUse,
    /// partition bounds exceed parent capacity
    InvalidPartition,
    /// block size is 0
    InvalidBlockSize,
    /// access ref count dropped below zero
    AccessUnderflow,
    /// this device does not support the operation,
    NotSupported,
}

impl Display for BlockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfBounds => f.write_str("requested op goes past device capacity"),
            Self::UnalignedBuffer => f.write_str("buffer length is not a multiple of block size"),
            Self::HardwareError => f.write_str("hardware error occurred during I/O"),
            Self::ReadOnly => f.write_str("tried to write to a read-only device"),
            Self::NotReady => f.write_str("device is not ready"),
            Self::InUse => f.write_str("lock requested but device is currently in use"),
            Self::InvalidPartition => f.write_str("partition bounds exceed parent capacity"),
            Self::InvalidBlockSize => f.write_str("block size is invalid (must be > 0)"),
            Self::AccessUnderflow => f.write_str("access reference count dropped below zero"),
            Self::NotSupported => f.write_str("operation not supported by this device"),
        }
    }
}

impl core::error::Error for BlockError {}

pub type Result<T> = core::result::Result<T, BlockError>;

pub trait BlockDevice: Send {
    /// flush any caches to the backing device.
    fn flush(&mut self) -> self::Result<()> {
        Ok(())
    }
    /// read blocks starting from a given logical block address.
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> self::Result<()>;
    /// write blocks starting from a given logical block address.
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> self::Result<()>;
    /// unmap/TRIM `count` blocks starting at `lba`.
    /// default impl is no-op.
    fn delete_blocks(&mut self, _lba: u64, _count: u64) -> self::Result<()> {
        Err(BlockError::NotSupported)
    }
    /// block size, in bytes.
    fn block_size(&self) -> usize;
    /// total number of accessible blocks on the device
    fn block_count(&self) -> u64;
}

/// the type of operation and the associated data.
pub enum Command<'a> {
    Read {
        buf: &'a mut [u8],
    },
    Write {
        buf: &'a [u8],
    },
    Flush,
    /// i.e. unmap/TRIM
    Delete {
        blocks: u64,
    },
}

pub struct IoRequest<'a> {
    pub cmd: Command<'a>,
    /// starting LBA for the request
    pub lba: u64,
}

impl<'a> IoRequest<'a> {
    /// the number of blocks this `IoRequest` affects.
    pub fn blocks(&self, block_size: usize) -> self::Result<u64> {
        if block_size == 0 {
            return Err(BlockError::InvalidBlockSize);
        }

        let buf_len = match &self.cmd {
            Command::Read { buf } => buf.len(),
            Command::Write { buf } => buf.len(),
            Command::Flush | Command::Delete { .. } => 0,
        };

        if buf_len > 0 && buf_len % block_size != 0 {
            return Err(BlockError::UnalignedBuffer);
        }

        Ok(match &self.cmd {
            Command::Read { .. } | Command::Write { .. } => (buf_len / block_size) as u64,
            Command::Flush => 0,
            Command::Delete { blocks } => *blocks,
        })
    }
}

/// a node in the graph that provides block storage. receives `IoRequest` requests.
pub trait Provider: Send {
    fn name(&self) -> &str;
    fn block_size(&self) -> usize;
    fn block_count(&self) -> u64;

    /// open references (R/W/E).
    /// deltas are passed to update counts.
    fn access(&mut self, read: isize, write: isize, exclusive: isize) -> self::Result<()>;

    /// process a request
    fn request(&mut self, req: IoRequest<'_>) -> self::Result<()>;
}

/// attaches to a `Provider`.
/// sends requests down, tracks accesses, and does bounds checking.
pub struct Consumer {
    provider: Arc<SleepingMutex<'static, dyn Provider>>,
    read_count: isize,
    write_count: isize,
    exclusive_count: isize,
}

impl Consumer {
    pub fn attach(provider: Arc<SleepingMutex<'static, dyn Provider>>) -> Self {
        Self {
            provider,
            read_count: 0,
            write_count: 0,
            exclusive_count: 0,
        }
    }

    pub fn block_size(&self) -> usize {
        self.provider.lock(&GLOBAL_SCHEDULER).block_size()
    }

    pub fn block_count(&self) -> u64 {
        self.provider.lock(&GLOBAL_SCHEDULER).block_count()
    }

    /// open/close the `Provider`.
    pub fn access(&mut self, read: isize, write: isize, exclusive: isize) -> self::Result<()> {
        let new_r = self.read_count + read;
        let new_w = self.write_count + write;
        let new_e = self.exclusive_count + exclusive;

        if new_r < 0 || new_w < 0 || new_e < 0 {
            return Err(BlockError::AccessUnderflow);
        }

        self.provider
            .lock(&GLOBAL_SCHEDULER)
            .access(read, write, exclusive)?;

        self.read_count = new_r;
        self.write_count = new_w;
        self.exclusive_count = new_e;

        Ok(())
    }

    /// submit a request to the provider.
    pub fn request(&mut self, req: IoRequest<'_>) -> self::Result<()> {
        let (block_size, cap) = {
            let p = self.provider.lock(&GLOBAL_SCHEDULER);
            (p.block_size(), p.block_count())
        };

        let blocks = req.blocks(block_size)?;

        if !matches!(req.cmd, Command::Flush) {
            if cap == 0
                || req.lba > cap
                || req.lba.checked_add(blocks).ok_or(BlockError::OutOfBounds)? > cap
            {
                return Err(BlockError::OutOfBounds);
            }
        }

        match &req.cmd {
            Command::Read { buf } => {
                if buf.len() % block_size != 0 {
                    return Err(BlockError::UnalignedBuffer);
                }

                if self.read_count <= 0 {
                    return Err(BlockError::NotReady);
                }
            }
            Command::Write { buf } => {
                if buf.len() % block_size != 0 {
                    return Err(BlockError::UnalignedBuffer);
                }

                if self.write_count <= 0 {
                    return Err(BlockError::ReadOnly);
                }
            }
            Command::Delete { .. } => {
                if self.write_count <= 0 {
                    // R/O open
                    return Err(BlockError::ReadOnly);
                }
            }
            Command::Flush => {}
        }

        self.provider.lock(&GLOBAL_SCHEDULER).request(req)
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if self.read_count > 0 || self.write_count > 0 || self.exclusive_count > 0 {
            let _ = self.provider.lock(&GLOBAL_SCHEDULER).access(
                -self.read_count,
                -self.write_count,
                -self.exclusive_count,
            );
        }
    }
}

/// leaf node.
pub struct HardwareAdapter {
    name: String,
    device: Box<dyn BlockDevice>,
    read_count: isize,
    write_count: isize,
    exclusive_count: isize,
}

impl HardwareAdapter {
    pub fn new(name: impl Into<String>, device: Box<dyn BlockDevice>) -> Self {
        Self {
            name: name.into(),
            device,
            read_count: 0,
            write_count: 0,
            exclusive_count: 0,
        }
    }
}

impl Provider for HardwareAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn block_size(&self) -> usize {
        self.device.block_size()
    }

    fn block_count(&self) -> u64 {
        self.device.block_count()
    }

    fn access(&mut self, read: isize, write: isize, exclusive: isize) -> self::Result<()> {
        let new_r = self.read_count + read;
        let new_w = self.write_count + write;
        let new_e = self.exclusive_count + exclusive;

        if new_r < 0 || new_w < 0 || new_e < 0 {
            // yeah that would be bad
            return Err(BlockError::AccessUnderflow);
        }

        if exclusive > 0 {
            // can't request an exclusive lock if any other lock is being held
            if self.read_count > 0 || self.write_count > 0 || self.exclusive_count > 0 {
                return Err(BlockError::InUse);
            }
        }

        if self.exclusive_count > 0 && (read > 0 || write > 0 || exclusive > 0) {
            // same issue
            return Err(BlockError::InUse);
        }

        // the last writer gets dropped.
        if self.write_count > 0 && new_w <= 0 {
            self.device.flush()?;
        }

        self.read_count = new_r;
        self.write_count = new_w;
        self.exclusive_count = new_e;

        Ok(())
    }

    fn request(&mut self, req: IoRequest<'_>) -> self::Result<()> {
        match req.cmd {
            Command::Read { buf } => self.device.read_blocks(req.lba, buf),
            Command::Write { buf } => self.device.write_blocks(req.lba, buf),
            Command::Flush => self.device.flush(),
            Command::Delete { blocks } => self.device.delete_blocks(req.lba, blocks),
        }
    }
}

impl Drop for HardwareAdapter {
    fn drop(&mut self) {
        if self.write_count > 0 {
            let _ = self.device.flush();
        }
    }
}

pub struct Partition {
    name: String,
    parent: Consumer,
    start_lba: u64,
    block_count: u64,
}

impl Partition {
    pub fn new(
        name: impl Into<String>,
        parent: Consumer,
        start_lba: u64,
        block_count: u64,
    ) -> self::Result<Self> {
        let parent_cap = parent.block_count();

        if start_lba > parent_cap
            || start_lba
                .checked_add(block_count)
                .ok_or(BlockError::InvalidPartition)?
                > parent_cap
        {
            return Err(BlockError::InvalidPartition);
        }

        Ok(Self {
            name: name.into(),
            parent,
            start_lba,
            block_count,
        })
    }
}

impl Provider for Partition {
    fn name(&self) -> &str {
        &self.name
    }

    fn block_size(&self) -> usize {
        self.parent.block_size()
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    fn access(&mut self, read: isize, write: isize, exclusive: isize) -> self::Result<()> {
        self.parent.access(read, write, exclusive)
    }

    fn request(&mut self, mut req: IoRequest<'_>) -> self::Result<()> {
        if !matches!(req.cmd, Command::Flush) {
            let block_size = self.parent.block_size();
            let blocks = req.blocks(block_size)?;
            if req.lba > self.block_count
                || req.lba.checked_add(blocks).ok_or(BlockError::OutOfBounds)? > self.block_count
            {
                return Err(BlockError::OutOfBounds);
            }

            req.lba = req
                .lba
                .checked_add(self.start_lba)
                .ok_or(BlockError::OutOfBounds)?;
        }

        self.parent.request(req)
    }
}
