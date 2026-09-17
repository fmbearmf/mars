use core::range::Range;

use uefi::{
    boot::{MemoryAttribute, MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::MemoryMap,
};

use crate::{
    sync::RwLock,
    vm::{PAGE_SIZE, align_down, align_up},
};

pub const MAX_REGIONS: usize = 128;

pub struct RangeKeeper {
    regions: [Range<usize>; MAX_REGIONS],
    count: usize,
}

static RANGE_KEEPER: RwLock<RangeKeeper> = RwLock::new(RangeKeeper::new());

pub fn init_rangekeeper(map: &impl MemoryMap) {
    RANGE_KEEPER.write().init_from_mmap(map);
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

impl RangeKeeper {
    pub const fn new() -> Self {
        const EMPTY_RANGE: Range<usize> = Range { start: 0, end: 0 };
        Self {
            regions: [EMPTY_RANGE; MAX_REGIONS],
            count: 0,
        }
    }

    fn is_usable_desc(desc: &MemoryDescriptor) -> bool {
        let not_runtime = !desc.att.contains(MemoryAttribute::RUNTIME);
        let is_wb = desc.att.contains(MemoryAttribute::WRITE_BACK);
        let is_conv = desc.ty == MemoryType::CONVENTIONAL;
        not_runtime && is_conv && is_wb
    }

    pub fn add_and_merge(&mut self, mut range: Range<usize>) {
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

    pub fn init_from_mmap(&mut self, map: &impl MemoryMap) {
        self.count = 0;

        for desc in map.entries().filter(|d| Self::is_usable_desc(d)) {
            let start = align_up(desc.phys_start as usize, PAGE_SIZE);
            let end = align_down(
                desc.phys_start as usize + (desc.page_count as usize * UEFI_PS),
                PAGE_SIZE,
            );

            if end > start {
                self.add_and_merge(Range { start, end });
            }
        }

        use log::*;
        trace!("RangeKeeper populated with {} usable regions", self.count);
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
