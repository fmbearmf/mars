use core::sync::atomic::{AtomicU64, Ordering};

use alloc::sync::Arc;

use super::{Result, VfsError, inode::DirEntry};
use crate::sync::RwLock;

pub struct File {
    pub dir_entry: Arc<RwLock<DirEntry>>,
    pub offset: AtomicU64,
    pub readable: bool,
    pub writable: bool,
}

impl File {
    pub fn new(dir_entry: Arc<RwLock<DirEntry>>, readable: bool, writable: bool) -> Self {
        Self {
            dir_entry,
            offset: AtomicU64::new(0),
            readable,
            writable,
        }
    }

    pub fn read(&self, buffer: &mut [u8]) -> Result<u64> {
        if !self.readable {
            return Err(VfsError::PermissionDenied);
        }

        let dir_entry = self.dir_entry.read();
        let current = self.offset.load(Ordering::Acquire);

        let bytes_count = dir_entry.inode.operations.read_at(current, buffer)?;
        self.offset.fetch_add(bytes_count, Ordering::AcqRel);

        Ok(bytes_count)
    }

    pub fn write(&self, buffer: &[u8]) -> Result<u64> {
        if !self.writable {
            return Err(VfsError::PermissionDenied);
        }

        let dir_entry = self.dir_entry.read();
        let current = self.offset.load(Ordering::Acquire);

        let bytes_count = dir_entry.inode.operations.write_at(current, buffer)?;
        self.offset.fetch_add(bytes_count, Ordering::AcqRel);

        let new_size = current + bytes_count;
        let mut old_size = dir_entry.inode.size.load(Ordering::Acquire);
        while new_size > old_size {
            match dir_entry.inode.size.compare_exchange_weak(
                old_size,
                new_size,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(real) => old_size = real,
            }
        }

        Ok(bytes_count)
    }
}
