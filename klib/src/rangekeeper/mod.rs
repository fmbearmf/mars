use core::range::Range;

use uefi::{
    boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::MemoryMap,
};

use crate::{
    sync::RwLock,
    vm::{PAGE_SIZE, align_down},
};

pub const MAX_REGIONS: usize = 128;

pub struct RangeKeeper {
    regions: [Range<usize>; MAX_REGIONS],
    count: usize,
    initialized: bool,
    reservations_open: bool,
}

static RANGE_KEEPER: RwLock<RangeKeeper> = RwLock::new(RangeKeeper::new());

pub fn init_rangekeeper(map: &impl MemoryMap) {
    let mut keeper = RANGE_KEEPER.write();
    assert!(!keeper.initialized, "RangeKeeper already initialized");
    keeper.init_from_mmap(map);
}

pub fn largest_range() -> Option<Range<usize>> {
    RANGE_KEEPER.read().largest_range()
}

pub fn for_each_range(mut f: impl FnMut(&Range<usize>)) {
    let guard = RANGE_KEEPER.read();
    for range in guard.ranges() {
        f(range);
    }
}

pub fn reserve(bytes: usize, alignment: usize) -> Range<usize> {
    let mut keeper = RANGE_KEEPER.write();

    assert!(keeper.initialized, "RangeKeeper is not initialized");
    assert!(
        keeper.reservations_open,
        "RangeKeeper reservations are closed"
    );

    keeper.reserve(bytes, alignment)
}

pub fn close_reservations() {
    let mut keeper = RANGE_KEEPER.write();
    assert!(keeper.initialized, "RangeKeeper is not initialized");
    keeper.reservations_open = false;
}

impl RangeKeeper {
    pub const fn new() -> Self {
        const EMPTY_RANGE: Range<usize> = Range { start: 0, end: 0 };
        Self {
            regions: [EMPTY_RANGE; MAX_REGIONS],
            count: 0,
            initialized: false,
            reservations_open: true,
        }
    }

    fn is_usable_desc(desc: &MemoryDescriptor) -> bool {
        let not_runtime = !desc.att.contains(MemoryAttribute::RUNTIME);
        let is_wb = desc.att.contains(MemoryAttribute::WRITE_BACK);
        let is_conv = desc.ty == MemoryType::CONVENTIONAL;
        let is_writable = !desc.att.contains(MemoryAttribute::READ_ONLY);

        not_runtime && is_conv && is_wb && is_writable
    }

    fn add_and_merge(&mut self, mut range: Range<usize>) {
        if range.start >= range.end {
            return;
        }

        let mut i = 0;
        while i < self.count {
            let current = &mut self.regions[i];

            if range.start <= current.end && range.end >= current.start {
                range.start = range.start.min(current.start);
                range.end = range.end.max(current.end);

                for j in i..(self.count - 1) {
                    self.regions[j] = self.regions[j + 1].clone();
                }
                self.count -= 1;
                continue;
            } else if range.end < current.start {
                break;
            }
            i += 1;
        }

        assert!(
            self.count < MAX_REGIONS,
            "RangeKeeper capacity exceeded!!!!!"
        );

        for j in (i..self.count).rev() {
            self.regions[j + 1] = self.regions[j].clone();
        }

        self.regions[i] = range;
        self.count += 1;
    }

    fn init_from_mmap(&mut self, map: &impl MemoryMap) {
        assert!(!self.initialized, "RangeKeeper already initialized");
        self.initialized = true;
        self.count = 0;

        for desc in map.entries().filter(|d| Self::is_usable_desc(d)) {
            let raw_start = usize::try_from(desc.phys_start).expect("physical address overflow");
            let size = usize::try_from(desc.page_count)
                .expect("memory descriptor size overflow")
                .checked_mul(UEFI_PS)
                .expect("memory descriptor size overflow");
            let raw_end = raw_start
                .checked_add(size)
                .expect("memory descriptor end overflow");
            let start = raw_start
                .checked_add(PAGE_SIZE - 1)
                .expect("memory descriptor alignment overflow")
                & !(PAGE_SIZE - 1);
            let end = align_down(raw_end, PAGE_SIZE);

            if end > start {
                self.add_and_merge(Range { start, end });
            }
        }

        use log::*;
        trace!("RangeKeeper populated with {} usable regions", self.count);
    }

    fn reserve(&mut self, bytes: usize, alignment: usize) -> Range<usize> {
        assert!(bytes > 0, "cannot reserve an empty range");
        assert!(alignment.is_power_of_two(), "invalid reservation alignment");

        let candidate = self.ranges().iter().enumerate().find_map(|(index, range)| {
            let start = range.start.checked_add(alignment - 1)? & !(alignment - 1);
            let end = start.checked_add(bytes)?;
            (end <= range.end).then_some((index, Range { start, end }))
        });
        let (index, reserved) = candidate.expect("no usable range for page descriptors");
        let original = self.regions[index].clone();
        match (reserved.start > original.start, reserved.end < original.end) {
            (true, true) => {
                assert!(self.count < MAX_REGIONS, "RangeKeeper capacity exceeded");
                for i in (index + 1..self.count).rev() {
                    self.regions[i + 1] = self.regions[i].clone();
                }

                self.regions[index] = Range {
                    start: original.start,
                    end: reserved.start,
                };

                self.regions[index + 1] = Range {
                    start: reserved.end,
                    end: original.end,
                };

                self.count += 1;
            }
            (true, false) => self.regions[index].end = reserved.start,
            (false, true) => self.regions[index].start = reserved.end,
            (false, false) => {
                for i in index..self.count - 1 {
                    self.regions[i] = self.regions[i + 1].clone();
                }
                self.count -= 1;
            }
        }

        reserved
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.regions[..self.count]
    }

    pub fn largest_range(&self) -> Option<Range<usize>> {
        self.ranges()
            .iter()
            .max_by_key(|r| r.end - r.start)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::RangeKeeper;

    #[test]
    fn reservation_splits_and_removes_backing() {
        let mut keeper = RangeKeeper::new();
        keeper.add_and_merge(core::range::Range {
            start: 0x1001,
            end: 0x9000,
        });

        let reserved = keeper.reserve(0x2000, 0x1000);

        assert_eq!(reserved.start, 0x2000);
        assert_eq!(reserved.end, 0x4000);
        assert_eq!(keeper.ranges().len(), 2);
        assert_eq!(keeper.ranges()[0].start, 0x1001);
        assert_eq!(keeper.ranges()[0].end, 0x2000);
        assert_eq!(keeper.ranges()[1].start, 0x4000);
        assert_eq!(keeper.ranges()[1].end, 0x9000);
    }
}
