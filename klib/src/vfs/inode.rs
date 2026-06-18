use core::sync::atomic::AtomicU64;

use alloc::{
    collections::btree_map::BTreeMap,
    string::String,
    sync::{Arc, Weak},
};

use crate::sync::RwLock;

use super::{FileType, Result};

pub struct Inode {
    pub number: u64,
    pub file_type: FileType,
    pub size: AtomicU64,
    pub operations: Arc<dyn InodeOperations + Send + Sync>,
}

pub trait InodeOperations {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64>;
    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<u64>;
    fn lookup_child(&self, name: &str) -> Result<Arc<Inode>>;
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<Inode>>;
    fn truncate(&self, size: u64) -> Result<()>;
}

/// cache for lookups
pub struct DirEntry {
    pub name: String,
    pub inode: Arc<Inode>,
    pub parent: Weak<RwLock<DirEntry>>,
    pub children: RwLock<BTreeMap<String, Arc<RwLock<DirEntry>>>>,
}

impl DirEntry {
    pub fn new(name: String, inode: Arc<Inode>, parent: Weak<RwLock<DirEntry>>) -> Self {
        Self {
            name,
            inode,
            parent,
            children: RwLock::new(BTreeMap::new()),
        }
    }
}
