use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};

use super::{PAGE_SIZE, backing::Backing};

#[derive(Debug, Clone)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub ap: AccessPermission,
    pub share: Shareability,
    pub uxn: bool,
    pub pxn: bool,
    pub attr_index: u64,
    pub backing: Backing,
}

impl Region {
    fn size(&self) -> usize {
        self.end - self.start
    }

    fn page_i_for_va(&self, va: usize) -> usize {
        (va - self.start) / PAGE_SIZE
    }
}
