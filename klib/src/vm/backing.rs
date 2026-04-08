extern crate alloc;

use alloc::vec::Vec;

use super::PAGE_SIZE;

#[derive(Debug, Clone)]
pub enum Backing {
    Owned { pages: Vec<usize> },
    Shared { pa: usize },
}

impl Backing {
    fn base_pa(&self) -> usize {
        match self {
            Self::Owned { pages } => pages[0],
            Self::Shared { pa } => *pa,
        }
    }

    fn page_pa(&self, page_i: usize) -> usize {
        match self {
            Self::Owned { pages } => pages[page_i],
            Self::Shared { pa } => pa + page_i * PAGE_SIZE,
        }
    }

    fn page_count(&self) -> usize {
        match self {
            Self::Owned { pages } => pages.len(),
            Self::Shared { .. } => 0,
        }
    }

    fn is_owned(&self) -> bool {
        matches!(self, Self::Owned { .. })
    }
}
