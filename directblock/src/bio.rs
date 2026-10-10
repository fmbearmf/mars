use alloc::boxed::Box;

use crate::{
    Completion, IoError, IoResult,
    buffer::{BufferLease, IoBuffer},
    device::DeviceInfo,
};

pub enum SubmitError {
    Full(Bio),
    Failed(IoError, Bio),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BioOp {
    Read,
    Write,
    Flush,
    Discard,
    WriteZeroes,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct BioFlags {
    pub fua: bool,
}

pub struct Bio {
    pub op: BioOp,
    pub lba: u64,
    pub blocks: u32,
    pub data: Option<IoBuffer>,
    pub flags: BioFlags,

    pub lease: Option<BufferLease>,
    pub completion: Completion,
}

impl Bio {
    pub fn validate(&self, info: DeviceInfo) -> IoResult {
        if info.block_size == 0 || info.max_transfer_blocks == 0 {
            return Err(IoError::GeometryMismatch);
        }

        if self.flags.fua {
            if self.op != BioOp::Write || !info.fua {
                return Err(IoError::Unsupported);
            }
        }

        match self.op {
            BioOp::Flush => {
                if !info.flush {
                    return Err(IoError::Unsupported);
                }

                if self.blocks != 0 || self.data.is_some() || self.lease.is_some() {
                    return Err(IoError::InvalidBuffer);
                }

                return Ok(());
            }
            BioOp::Write if info.read_only => {
                return Err(IoError::ReadOnly);
            }
            BioOp::Discard if !info.discard => {
                return Err(IoError::Unsupported);
            }
            BioOp::WriteZeroes if !info.write_zeroes => {
                return Err(IoError::Unsupported);
            }
            BioOp::Read | BioOp::Write | BioOp::Discard | BioOp::WriteZeroes => {}
        }

        if self.blocks == 0 || self.blocks > info.max_transfer_blocks {
            return Err(IoError::OutOfBounds);
        }

        let end = self
            .lba
            .checked_add(self.blocks as u64)
            .ok_or(IoError::OutOfBounds)?;

        if end > info.block_count {
            return Err(IoError::OutOfBounds);
        }

        match self.op {
            BioOp::Read | BioOp::Write => {
                let buffer = self.data.ok_or(IoError::InvalidBuffer)?;

                let lease = self.lease.as_ref().ok_or(IoError::InvalidBuffer)?;

                if lease.id() != buffer.id {
                    return Err(IoError::InvalidBuffer);
                }

                let bytes = (self.blocks as usize)
                    .checked_mul(info.block_size as usize)
                    .ok_or(IoError::OutOfBounds)?;

                if buffer.len != bytes {
                    return Err(IoError::InvalidBuffer);
                }

                let permitted = match self.op {
                    BioOp::Read => buffer.can_write(),
                    BioOp::Write => buffer.can_read(),
                    _ => unreachable!(),
                };

                if !permitted {
                    return Err(IoError::InvalidBuffer);
                }
            }
            BioOp::Discard | BioOp::WriteZeroes => {
                if self.data.is_some() || self.lease.is_some() {
                    return Err(IoError::InvalidBuffer);
                }
            }
            BioOp::Flush => unreachable!(),
        }

        Ok(())
    }

    /// consume an accepted request and deliver its completion.
    /// lease is alive until after callback execution
    pub fn complete(self, result: IoResult) {
        let Self {
            data,
            lease,
            completion,
            ..
        } = self;

        completion.complete(result, data);

        drop(lease);
    }
}
