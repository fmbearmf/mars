use crate::paging::{Entry, GEOMETRY, Level, PageTable, PagingError};

/// supply initialized table storage and address translation
pub trait TableMemory {
    fn alloc_table(&self) -> Result<PageTable, PagingError>;
    fn free_table(&self, table: PageTable);
    fn physical_address(&self, table: PageTable) -> usize;
    /// safety:
    /// address points to a live table that's reachable through this provider
    unsafe fn table_at(&self, physical: usize) -> PageTable;
}

/// opaque, non-owning context handle
#[derive(Copy, Clone, Debug)]
pub struct AddressSpaceContext {
    pub(crate) state: crate::arch::ContextState,
}

impl AddressSpaceContext {
    /// create an empty context for early mapping.
    ///
    /// safety:
    /// the allocated tables must stay live until the context is destroyed (exactly once)
    pub unsafe fn create(memory: &dyn TableMemory) -> Result<Self, PagingError> {
        Ok(Self {
            state: unsafe { crate::arch::create_context(memory) }?,
        })
    }
    /// create an empty application context that retains the active kernel mappings
    ///
    /// safety:
    /// inherited mappings must outlive this context, and its allocation has one owner
    pub unsafe fn inherit_kernel(memory: &dyn TableMemory) -> Result<Self, PagingError> {
        Ok(Self {
            state: unsafe { crate::arch::inherit_kernel_context(memory) }?,
        })
    }
    /// *borrow* the currently active native context
    ///
    /// safety:
    /// native paging must be initialized and the owner must keep its PTs live
    pub unsafe fn current() -> Self {
        Self {
            state: unsafe { crate::arch::current_context() },
        }
    }

    pub fn table_for(
        self,
        address: usize,
        memory: &dyn TableMemory,
    ) -> Result<PageTable, PagingError> {
        if !crate::paging::valid_virtual_address(address) {
            return Err(PagingError::InvalidAddress);
        }
        Ok(unsafe { memory.table_at(crate::arch::context_table(self.state, address)) })
    }

    /// safety:
    /// all context storage and inherited mappings must remain live while active on any core
    pub unsafe fn activate(self) {
        unsafe { crate::arch::activate_context(self.state) }
    }

    /// duplicate hardware table storage. borrow existing mapped backing memory.
    ///
    /// safety:
    /// the source descriptors must be stable. the returned copy's allocated tables have one owner
    pub unsafe fn clone(self, memory: &dyn TableMemory) -> Result<Self, PagingError> {
        Ok(Self {
            state: unsafe { crate::arch::clone_context(self.state, memory) }?,
        })
    }

    /// safety:
    /// called exactly once by the owner after excluding (software) readers and switching every core away.
    /// inherited table storage and mapped backing memory are not freed
    pub unsafe fn destroy(self, memory: &dyn TableMemory) {
        unsafe { crate::arch::destroy_context(self.state, memory) }
    }
}

pub(crate) unsafe fn clone_tree(
    source: PageTable,
    level: Level,
    memory: &dyn TableMemory,
) -> Result<PageTable, PagingError> {
    let destination = memory.alloc_table()?;
    for index in 0..GEOMETRY.entries_at(level) {
        let entry = unsafe { source.read(index, level) };
        let copied = match entry {
            Entry::Table { physical_address } => {
                let child_level = level.child().expect("table at leaf level");
                match unsafe { clone_tree(memory.table_at(physical_address), child_level, memory) }
                {
                    Ok(child) => Entry::Table {
                        physical_address: memory.physical_address(child),
                    },
                    Err(error) => {
                        unsafe { destroy_tree(destination, level, memory) };
                        return Err(error);
                    }
                }
            }
            leaf => leaf,
        };
        if let Err(error) = unsafe { destination.write(index, level, copied) } {
            if let Entry::Table { physical_address } = copied {
                unsafe {
                    destroy_tree(
                        memory.table_at(physical_address),
                        level.child().unwrap(),
                        memory,
                    )
                };
            }
            unsafe { destroy_tree(destination, level, memory) };
            return Err(error);
        }
    }
    Ok(destination)
}

pub(crate) unsafe fn destroy_tree(table: PageTable, level: Level, memory: &dyn TableMemory) {
    for index in 0..GEOMETRY.entries_at(level) {
        let entry = unsafe { table.read(index, level) };
        if entry != Entry::Invalid {
            unsafe { table.write(index, level, Entry::Invalid) }.expect("invalid descriptor");
        }
        if let Entry::Table { physical_address } = entry {
            unsafe {
                destroy_tree(
                    memory.table_at(physical_address),
                    level.child().expect("table at leaf level"),
                    memory,
                )
            };
        }
    }
    memory.free_table(table);
}
