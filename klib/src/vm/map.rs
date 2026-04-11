extern crate alloc;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::{collections::BTreeMap, vec::Vec};

use super::{
    PAGE_SIZE, TABLE_ENTRIES, TTable, VmError, align_down, align_up, backing::Backing,
    page_allocator::PhysicalPageAllocator as PageAllocator, region::Region,
};

use crate::pm::page::mapper::{TableAllocator, map_region, unmap_region};

#[derive(Debug)]
pub struct Map {
    regions: BTreeMap<usize, Region>,
}

impl Map {
    pub const fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
        }
    }

    fn check_range(&self, va: usize, size: usize) -> Result<usize, VmError> {
        if size == 0 {
            return Err(VmError::InvalidSize);
        }

        if va != align_down(va, PAGE_SIZE) {
            return Err(VmError::InvalidAlignment);
        }

        let end = va
            .checked_add(align_up(size, PAGE_SIZE))
            .ok_or(VmError::InvalidAddress)?;

        if end <= va {
            return Err(VmError::InvalidAddress);
        }

        Ok(end)
    }

    fn overlaps(&self, va: usize, end: usize) -> bool {
        if let Some((_, prev)) = self.regions.range(..=va).next_back() {
            if prev.end > va {
                return true;
            }
        }

        self.regions.range(va..end).next().is_some()
    }

    fn map_pages<A: TableAllocator>(
        root: &mut TTable<TABLE_ENTRIES>,
        va: usize,
        pages: &[usize],
        ap: AccessPermission,
        share: Shareability,
        uxn: bool,
        pxn: bool,
        attr_index: u64,
        allocator: &A,
    ) -> Result<(), VmError> {
        for (i, &pa) in pages.iter().enumerate() {
            let page_va = va + i * PAGE_SIZE;

            map_region(
                root, pa, page_va, PAGE_SIZE, ap, share, uxn, pxn, attr_index, allocator,
            );
        }

        Ok(())
    }

    fn unmap_pages<A: TableAllocator>(
        root: &mut TTable<TABLE_ENTRIES>,
        va: usize,
        pages: usize,
        allocator: &A,
    ) {
        for i in 0..pages {
            let page_va = va + i * PAGE_SIZE;
            unmap_region(root, page_va, PAGE_SIZE, allocator);
        }
    }

    pub fn mmap_anonymous<A: TableAllocator, P: PageAllocator>(
        &mut self,
        root: &mut TTable<TABLE_ENTRIES>,
        va_hint: Option<usize>,
        size: usize,
        ap: AccessPermission,
        share: Shareability,
        uxn: bool,
        pxn: bool,
        attr_index: u64,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<usize, VmError> {
        let size = align_up(size, PAGE_SIZE);
        let pages = size / PAGE_SIZE;

        let va = match va_hint {
            Some(hint) => align_down(hint, PAGE_SIZE),
            None => self
                .find_space(0, size)
                .map(|s| align_down(s, PAGE_SIZE))
                .ok_or(VmError::OutOfMemory)?,
        };

        let end = va.checked_add(size).ok_or(VmError::InvalidAddress)?;
        if self.overlaps(va, end) {
            return Err(VmError::Overlap);
        }

        let mut owned_pages = Vec::with_capacity(pages);

        for _ in 0..pages {
            let pa = page_alloc.alloc_page()?;
            owned_pages.push(pa);
        }

        if let Err(e) = Self::map_pages(
            root,
            va,
            &owned_pages,
            ap,
            share,
            uxn,
            pxn,
            attr_index,
            table_alloc,
        ) {
            for pa in owned_pages.drain(..) {
                page_alloc.free_page(pa);
            }

            return Err(e);
        }

        let region = Region {
            start: va,
            end,
            ap,
            share,
            uxn,
            pxn,
            attr_index,
            backing: Backing::Owned { pages: owned_pages },
        };

        self.regions.insert(va, region);
        Ok(va)
    }

    pub fn enter_borrowed<A: TableAllocator>(
        &mut self,
        root: &mut TTable<TABLE_ENTRIES>,
        pa: usize,
        va: usize,
        size: usize,
        access: AccessPermission,
        share: Shareability,
        uxn: bool,
        pxn: bool,
        attr_index: u64,
        allocator: &A,
    ) -> Result<(), VmError> {
        let size = align_up(size, PAGE_SIZE);
        let va = align_down(va, PAGE_SIZE);
        let end = va.checked_add(size).ok_or(VmError::InvalidAddress)?;

        if self.overlaps(va, end) {
            return Err(VmError::Overlap);
        }

        let pages = size / PAGE_SIZE;
        let mut phys = Vec::with_capacity(pages);
        for i in 0..pages {
            phys.push(pa + i * PAGE_SIZE);
        }

        Self::map_pages(
            root, va, &phys, access, share, uxn, pxn, attr_index, allocator,
        )?;

        self.regions.insert(
            va,
            Region {
                start: va,
                end,
                ap: access,
                share,
                uxn,
                pxn,
                attr_index,
                backing: Backing::Shared { pa },
            },
        );

        Ok(())
    }

    pub fn remove<A: TableAllocator, P: PageAllocator>(
        &mut self,
        root: &mut TTable<TABLE_ENTRIES>,
        va: usize,
        size: usize,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<(), VmError> {
        if size == 0 {
            return Ok(());
        }

        if va != align_down(va, PAGE_SIZE) {
            return Err(VmError::InvalidAlignment);
        }

        let size = align_up(size, PAGE_SIZE);
        let end = va.checked_add(size).ok_or(VmError::InvalidAddress)?;

        let affected: Vec<usize> = self
            .regions
            .range(..end)
            .filter(|(_, r)| r.end > va && r.start < end)
            .map(|(&k, _)| k)
            .collect();

        let mut reinsert: Vec<Region> = Vec::new();

        for key in affected {
            let region = self.regions.remove(&key).ok_or(VmError::NotMapped)?;

            let r_start = region.start;
            let r_end = region.end;

            let unmap_start = r_start.max(va);
            let unmap_end = r_end.min(end);
            let unmap_size = unmap_end - unmap_start;
            let unmap_pages = unmap_size / PAGE_SIZE;

            if unmap_pages > 0 {
                let first = (unmap_start - r_start) / PAGE_SIZE;

                Self::unmap_pages(root, unmap_start, unmap_pages, table_alloc);

                if let Backing::Owned { pages } = region.backing {
                    for pa in pages[first..first + unmap_pages].iter().copied() {
                        page_alloc.free_page(pa);
                    }

                    let left = first;
                    let right = pages.len() - first - unmap_pages;

                    if left > 0 {
                        let left_backing = Backing::Owned {
                            pages: pages[..left].to_vec(),
                        };

                        reinsert.push(Region {
                            start: r_start,
                            end: unmap_start,
                            ap: region.ap,
                            share: region.share,
                            uxn: region.uxn,
                            pxn: region.pxn,
                            attr_index: region.attr_index,
                            backing: left_backing,
                        });
                    }

                    if right > 0 {
                        let right_backing = Backing::Owned {
                            pages: pages[first + unmap_pages..].to_vec(),
                        };

                        reinsert.push(Region {
                            start: unmap_end,
                            end: r_end,
                            ap: region.ap,
                            share: region.share,
                            uxn: region.uxn,
                            pxn: region.pxn,
                            attr_index: region.attr_index,
                            backing: right_backing,
                        });
                    }
                } else {
                    let left_size = unmap_start - r_start;
                    let right_size = r_end - unmap_end;

                    if left_size > 0 {
                        reinsert.push(Region {
                            start: r_start,
                            end: unmap_start,
                            ap: region.ap,
                            share: region.share,
                            uxn: region.uxn,
                            pxn: region.pxn,
                            attr_index: region.attr_index,
                            backing: Backing::Shared {
                                pa: match region.backing {
                                    Backing::Shared { pa } => pa,
                                    _ => unreachable!(),
                                },
                            },
                        });
                    }

                    if right_size > 0 {
                        let base_pa = match region.backing {
                            Backing::Shared { pa } => pa + (unmap_end - r_start),
                            _ => unreachable!(),
                        };

                        reinsert.push(Region {
                            start: unmap_end,
                            end: r_end,
                            ap: region.ap,
                            share: region.share,
                            uxn: region.uxn,
                            pxn: region.pxn,
                            attr_index: region.attr_index,
                            backing: Backing::Shared { pa: base_pa },
                        });
                    }
                }
            } else {
                reinsert.push(region);
            }
        }

        for region in reinsert {
            self.regions.insert(region.start, region);
        }

        Ok(())
    }

    pub fn lookup(&self, va: usize) -> Option<&Region> {
        if let Some((_, region)) = self.regions.range(..=va).next_back() {
            if va < region.end {
                return Some(region);
            }
        }
        None
    }

    pub fn find_space(&self, min_va: usize, size: usize) -> Option<usize> {
        if size == 0 {
            return Some(min_va);
        }

        let size = align_up(size, PAGE_SIZE);
        let mut current = align_up(min_va, PAGE_SIZE);

        if let Some((_, region)) = self.regions.range(..=current).next_back() {
            if region.end > current {
                current = region.end;
            }
        }

        for (&start, region) in self.regions.range(current..) {
            if start > current {
                let gap = start - current;
                if gap >= size {
                    return Some(current);
                }
            }
            current = region.end;
        }

        Some(current)
    }

    pub fn clear<A: TableAllocator, P: PageAllocator>(
        &mut self,
        root: &mut TTable<TABLE_ENTRIES>,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<(), VmError> {
        let regions: Vec<(usize, Region)> = self.regions.clone().into_iter().collect();
        self.regions.clear();

        for (_, region) in regions {
            let pages = (region.end - region.start) / PAGE_SIZE;

            Self::unmap_pages(root, region.start, pages, table_alloc);

            if let Backing::Owned { pages } = &region.backing {
                for &pa in pages {
                    page_alloc.free_page(pa);
                }
            }
        }

        Ok(())
    }

    pub fn regions_count(&self) -> usize {
        self.regions.len()
    }
}
