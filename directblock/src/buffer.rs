use alloc::{sync::Arc, vec, vec::Vec};

// TODO: support non-contiguous memory
#[derive(Clone)]
pub enum IoBuffer {
    Unique(Vec<u8>),
    Shared(Arc<Vec<u8>>),
}

impl IoBuffer {
    pub fn zeroed(size: usize) -> Self {
        Self::Unique(vec![0u8; size])
    }

    pub fn from_vec(data: Vec<u8>) -> Self {
        Self::Unique(data)
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Unique(v) => v.as_slice(),
            Self::Shared(v) => v.as_slice(),
        }
    }

    pub fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        match self {
            Self::Unique(v) => Some(v.as_mut_slice()),
            Self::Shared(_) => None,
        }
    }

    pub fn to_shared(self) -> Self {
        match self {
            Self::Unique(v) => Self::Shared(Arc::new(v)),
            Self::Shared(s) => Self::Shared(s),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
