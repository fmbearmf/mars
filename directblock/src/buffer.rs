use alloc::{sync::Arc, vec, vec::Vec};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BufferId {
    pub slot: u32,
    pub generation: u32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BufferAccess {
    Read,
    Write,
    ReadWrite,
}

/// buffer descriptor. does not any carry ownership
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct IoBuffer {
    pub id: BufferId,
    pub offset: u64,
    pub len: usize,
    pub access: BufferAccess,
}

impl IoBuffer {
    pub fn slice(&self, offset: usize, len: usize) -> Option<Self> {
        let end = offset.checked_add(len)?;

        if end > self.len {
            return None;
        }

        Some(Self {
            id: self.id,
            offset: self.offset.checked_add(offset as u64)?,
            len,
            access: self.access,
        })
    }

    pub fn can_read(&self) -> bool {
        matches!(self.access, BufferAccess::Read | BufferAccess::ReadWrite)
    }

    pub fn can_write(&self) -> bool {
        matches!(self.access, BufferAccess::Write | BufferAccess::ReadWrite)
    }
}

/// keep the allocation alive
#[derive(Clone)]
pub struct BufferLease {
    id: BufferId,
    owner: Arc<dyn Send + Sync>,
}

impl BufferLease {
    pub fn new<T>(id: BufferId, owner: Arc<T>) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self { id, owner }
    }

    pub fn id(&self) -> BufferId {
        self.id
    }

    pub fn owner_is_shared(&self) -> bool {
        Arc::strong_count(&self.owner) > 1
    }
}
