use crate::{Completion, IoError, IoResult, buffer::IoBuffer, device::DeviceInfo};

pub enum SubmitError {
    Full(Bio),
    Failed(IoError, Bio),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BioOp {
    Read,
    Write,
    Flush,
}

pub struct Bio {
    pub op: BioOp,
    pub lba: u64,
    pub blocks: u32,
    pub data: Option<IoBuffer>,
    pub completion: Completion,
}

impl Bio {
    pub fn validate(&self, info: DeviceInfo) -> IoResult {
        if info.block_size == 0 {
            return Err(IoError::GeometryMismatch);
        }

        match self.op {
            BioOp::Flush => {
                if self.blocks != 0 || self.data.is_some() {
                    return Err(IoError::InvalidBuffer);
                }

                return Ok(());
            }
            BioOp::Read | BioOp::Write => {
                if self.blocks == 0 {
                    return Err(IoError::InvalidBuffer);
                }
            }
        }

        let end = self
            .lba
            .checked_add(self.blocks as u64)
            .ok_or(IoError::OutOfBounds)?;

        if end > info.block_count {
            return Err(IoError::OutOfBounds);
        }

        let bytes = (self.blocks as usize)
            .checked_mul(info.block_size as usize)
            .ok_or(IoError::OutOfBounds)?;

        let data = self.data.as_ref().ok_or(IoError::InvalidBuffer)?;

        if data.len() != bytes {
            return Err(IoError::InvalidBuffer);
        }

        if self.op == BioOp::Read {
            if !matches!(data, IoBuffer::Unique(_)) {
                return Err(IoError::InvalidBuffer);
            }
        }

        Ok(())
    }
}
