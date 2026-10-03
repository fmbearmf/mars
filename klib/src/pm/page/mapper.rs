use hal::paging::{Entry, GEOMETRY, Level, MappingOptions, PageTable, valid_virtual_address};

use crate::vm::{PAGE_MASK, PAGE_SIZE, VmError};

/// allocations must contain an initialized, unpublished table with the HAL's layout
pub trait TableAllocator: Send + Sync {
    fn alloc_table(&self) -> Result<PageTable, VmError>;
    fn free_table(&self, table: PageTable);
}

pub trait AddressTranslator: Send + Sync {
    fn phys_to_dmap(&self, phys: usize) -> *mut u8;
    fn dmap_to_phys(&self, virt: *mut u8) -> usize;
}

/// provide allocation to HAL's address-space builder
pub struct MemoryProvider<'a> {
    allocator: &'a dyn TableAllocator,
    translator: &'a dyn AddressTranslator,
}

impl<'a> MemoryProvider<'a> {
    pub const fn new(
        allocator: &'a dyn TableAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self {
            allocator,
            translator,
        }
    }
}

impl hal::paging::TableMemory for MemoryProvider<'_> {
    fn alloc_table(&self) -> Result<PageTable, hal::paging::PagingError> {
        self.allocator.alloc_table().map_err(Into::into)
    }
    fn free_table(&self, table: PageTable) {
        self.allocator.free_table(table);
    }
    fn physical_address(&self, table: PageTable) -> usize {
        self.translator.dmap_to_phys(table.as_ptr())
    }
    unsafe fn table_at(&self, physical: usize) -> PageTable {
        unsafe { PageTable::from_ptr(self.translator.phys_to_dmap(physical)) }
    }
}

pub(crate) fn validate_range(va: usize, size: usize) -> Result<(), VmError> {
    if size == 0 {
        return Err(VmError::InvalidSize);
    }
    if (va | size) & PAGE_MASK != 0 {
        return Err(VmError::InvalidAlignment);
    }
    let last = va.checked_add(size - 1).ok_or(VmError::InvalidAddress)?;
    if !valid_virtual_address(va)
        || !valid_virtual_address(last)
        || (va >> (usize::BITS - 1)) != (last >> (usize::BITS - 1))
    {
        return Err(VmError::InvalidAddress);
    }
    Ok(())
}

unsafe fn translated_table(pa: usize, translator: &dyn AddressTranslator) -> PageTable {
    unsafe { PageTable::from_ptr(translator.phys_to_dmap(pa)) }
}

/// find or create a path. need exclusive access to the traversed subtree
unsafe fn walk_to(
    root: PageTable,
    va: usize,
    target: Level,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<PageTable, VmError> {
    let mut table = root;
    let mut level = Level::ROOT;
    while level != target {
        let index = level.index(va);
        table = match unsafe { table.read(index, level) } {
            Entry::Table { physical_address } => unsafe {
                translated_table(physical_address, translator)
            },
            Entry::Invalid => {
                let child = allocator.alloc_table()?;
                let entry = Entry::Table {
                    physical_address: translator.dmap_to_phys(child.as_ptr()),
                };
                if let Err(error) = unsafe { table.write(index, level, entry) } {
                    allocator.free_table(child);
                    return Err(error.into());
                }
                child
            }
            Entry::Mapping { .. } => return Err(VmError::Overlap),
        };
        level = level.child().ok_or(VmError::InvalidSize)?;
    }
    Ok(table)
}

unsafe fn mapping_at(
    root: PageTable,
    va: usize,
    translator: &dyn AddressTranslator,
) -> Option<(PageTable, Level, Entry)> {
    let mut table = root;
    let mut level = Level::ROOT;
    loop {
        match unsafe { table.read(level.index(va), level) } {
            Entry::Invalid => return None,
            Entry::Table { physical_address } => {
                table = unsafe { translated_table(physical_address, translator) };
                level = level.child()?;
            }
            entry @ Entry::Mapping { .. } => return Some((table, level, entry)),
        }
    }
}

/// map borrowed physical memory. on failure, remove the new mappings.
///
/// safety
/// caller must exclude access to the affected subtree for the entire operation.
/// the physical memory and translator must remain valid for the mapping's lifetime
pub unsafe fn map_region(
    root: PageTable,
    pa: usize,
    va: usize,
    size: usize,
    options: MappingOptions,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    validate_range(va, size)?;
    if pa & PAGE_MASK != 0 {
        return Err(VmError::InvalidAlignment);
    }
    pa.checked_add(size - 1).ok_or(VmError::InvalidAddress)?;
    for offset in (0..size).step_by(PAGE_SIZE) {
        if unsafe { mapping_at(root, va + offset, translator) }.is_some() {
            return Err(VmError::Overlap);
        }
    }
    for offset in (0..size).step_by(PAGE_SIZE) {
        if let Err(error) = unsafe {
            map_page(
                root,
                pa + offset,
                va + offset,
                options,
                allocator,
                translator,
            )
        } {
            for installed in (0..offset).step_by(PAGE_SIZE) {
                unsafe { unmap_page(root, va + installed, allocator, translator) }
                    .expect("rollback of installed mapping failed");
            }
            return Err(error);
        }
    }
    Ok(())
}

/// safety:
/// exclusive software access to the affected subtree and valid backing memory
pub unsafe fn map_page(
    root: PageTable,
    pa: usize,
    va: usize,
    options: MappingOptions,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    unsafe { map_block(root, pa, va, Level::LEAF, options, allocator, translator) }
}

/// safety:
/// requires exclusive access to the affected subtree and valid backing memory
pub unsafe fn map_block(
    root: PageTable,
    pa: usize,
    va: usize,
    level: Level,
    options: MappingOptions,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    let size = level.coverage();
    validate_range(va, size)?;
    if (pa | va) & (size - 1) != 0 {
        return Err(VmError::InvalidAlignment);
    }
    hal::paging::validate_mapping(pa, level, options)?;
    let table = unsafe { walk_to(root, va, level, allocator, translator) }?;
    let index = level.index(va);
    let entry = Entry::Mapping {
        physical_address: pa,
        options,
    };
    let previous = unsafe { table.read(index, level) };
    if previous == entry {
        return Ok(());
    }
    if previous != Entry::Invalid {
        return Err(VmError::Overlap);
    }
    unsafe { table.write(index, level, entry) }?;
    Ok(())
}

/// setup identity map covering one second-level span using L1 blocks.
///
/// safety:
/// root must be unpublished or owned exclusively by the caller
pub unsafe fn id_map(
    root: PageTable,
    options: MappingOptions,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    let block = Level::LEAF.parent().ok_or(VmError::Unsupported)?;
    let span = block.parent().ok_or(VmError::Unsupported)?.coverage();
    for address in (0..span).step_by(block.coverage()) {
        unsafe {
            map_block(
                root, address, address, block, options, allocator, translator,
            )
        }?;
    }
    Ok(())
}

/// safety:
/// requires exclusive access to the affected subtree
pub unsafe fn unmap_region(
    root: PageTable,
    va: usize,
    size: usize,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    validate_range(va, size)?;
    // make sure there isn't any partial block removal before changing mappings
    for offset in (0..size).step_by(PAGE_SIZE) {
        if let Some((_, level, _)) = unsafe { mapping_at(root, va + offset, translator) } {
            let base = (va + offset) & !(level.coverage() - 1);
            if base < va
                || base
                    .checked_add(level.coverage())
                    .ok_or(VmError::InvalidAddress)?
                    > va + size
            {
                return Err(VmError::InvalidSize);
            }
        }
    }
    let mut offset = 0;
    while offset < size {
        if let Some((table, level, _)) = unsafe { mapping_at(root, va + offset, translator) } {
            unsafe { table.write(level.index(va + offset), level, Entry::Invalid) }?;
            offset += level.coverage();
        } else {
            offset += PAGE_SIZE;
        }
    }
    let _ = allocator;
    Ok(())
}

/// safety:
/// requires exclusive access to the affected subtree
pub unsafe fn unmap_page(
    root: PageTable,
    va: usize,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<(), VmError> {
    unsafe { unmap_region(root, va, PAGE_SIZE, allocator, translator) }
}

/// copy table storage but retain borrowed leaf mappings. the source must be stable.
///
/// safety:
/// caller must exclude tree mutations and provide a working translator
pub unsafe fn clone_page_tables(
    root: PageTable,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<PageTable, VmError> {
    unsafe { clone_node(root, Level::ROOT, allocator, translator) }
}

unsafe fn clone_node(
    source: PageTable,
    level: Level,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) -> Result<PageTable, VmError> {
    let copy = allocator.alloc_table()?;
    for index in 0..GEOMETRY.entries_at(level) {
        let entry = unsafe { source.read(index, level) };
        let copied = match entry {
            Entry::Table { physical_address } => {
                let child_level = level.child().ok_or(VmError::InvalidSize)?;
                let source_child = unsafe { translated_table(physical_address, translator) };
                match unsafe { clone_node(source_child, child_level, allocator, translator) } {
                    Ok(child) => Entry::Table {
                        physical_address: translator.dmap_to_phys(child.as_ptr()),
                    },
                    Err(error) => {
                        unsafe { free_node(copy, level, allocator, translator) };
                        return Err(error);
                    }
                }
            }
            leaf => leaf,
        };
        if let Err(error) = unsafe { copy.write(index, level, copied) } {
            if let Entry::Table { physical_address } = copied {
                unsafe {
                    free_node(
                        translated_table(physical_address, translator),
                        level.child().unwrap(),
                        allocator,
                        translator,
                    )
                };
            }
            unsafe { free_node(copy, level, allocator, translator) };
            return Err(error.into());
        }
    }
    Ok(copy)
}

/// free the table storage, but not the borrowed mapped physical pages (that would be bad)
///
/// safety:
/// the tree must not be active on any CPU or read by software
pub unsafe fn free_tables(
    root: PageTable,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) {
    unsafe { free_node(root, Level::ROOT, allocator, translator) }
}

unsafe fn free_node(
    table: PageTable,
    level: Level,
    allocator: &dyn TableAllocator,
    translator: &dyn AddressTranslator,
) {
    for index in 0..GEOMETRY.entries_at(level) {
        let entry = unsafe { table.read(index, level) };
        if entry != Entry::Invalid {
            unsafe { table.write(index, level, Entry::Invalid) }
                .expect("clearing valid entry failed");
        }
        if let Entry::Table { physical_address } = entry {
            unsafe {
                free_node(
                    translated_table(physical_address, translator),
                    level.child().expect("table at leaf level"),
                    allocator,
                    translator,
                )
            };
        }
    }
    allocator.free_table(table);
}
