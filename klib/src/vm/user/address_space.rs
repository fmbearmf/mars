use alloc::{boxed::Box, vec::Vec};
use core::{
    fmt,
    ops::Range,
    sync::atomic::{AtomicPtr, Ordering},
};
use hal::paging::{AddressSpaceContext, Entry, Level, PageTable};

use super::{PAGE_DESCRIPTORS, allocator::UserAllocator, cursor::Cursor};
use crate::{
    allocator_support::KernelAddressTranslator,
    pm::page::mapper::{self, AddressTranslator, MemoryProvider, TableAllocator},
    vm::{KPAGE_ALLOCATOR, KPT_ALLOCATOR, VmError, page_allocator::PhysicalPageAllocator},
};

pub static KERNEL_ADDRESS_SPACE: AddressSpace<'static> = AddressSpace::new_kernel();

/// holds an address space context and the table storage allocated for it
pub struct AddressSpace<'a> {
    context: AtomicPtr<AddressSpaceContext>,
    pub(crate) allocator: UserAllocator<'a>,
    translator: &'a dyn AddressTranslator,
    kernel: bool,
}

// the providers are synchronized. tree access uses descriptor locks
unsafe impl Sync for AddressSpace<'_> {}
unsafe impl Send for AddressSpace<'_> {}

impl fmt::Debug for AddressSpace<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AddressSpace")
            .field(
                "initialized",
                &(!self.context.load(Ordering::Acquire).is_null()),
            )
            .finish()
    }
}

impl<'a> AddressSpace<'a> {
    /// creates an empty application (i.e. user) address space with the active kernel mappings inherited
    pub fn new(
        tables: &'a dyn TableAllocator,
        pages: &'a dyn PhysicalPageAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Result<Self, VmError> {
        let space = Self::new_dangling(tables, pages, translator);
        space.init()?;
        Ok(space)
    }

    pub const fn new_dangling(
        tables: &'a dyn TableAllocator,
        pages: &'a dyn PhysicalPageAllocator,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self {
            context: AtomicPtr::new(core::ptr::null_mut()),
            allocator: UserAllocator(tables, pages, translator),
            translator,
            kernel: false,
        }
    }

    const fn new_kernel() -> AddressSpace<'static> {
        let mut space =
            AddressSpace::new_dangling(&KPT_ALLOCATOR, &KPAGE_ALLOCATOR, &KernelAddressTranslator);
        space.kernel = true;
        space
    }

    pub fn init(&self) -> Result<(), VmError> {
        if self.kernel {
            return Err(VmError::Unsupported);
        }
        if !self.context.load(Ordering::Acquire).is_null() {
            return Err(VmError::AlreadyInitialized);
        }
        let memory = self.memory();
        let context = unsafe { AddressSpaceContext::inherit_kernel(&memory) }?;
        if let Err(error) = unsafe { self.init_from_context(context) } {
            unsafe { context.destroy(&memory) };
            return Err(error);
        }
        Ok(())
    }

    /// transfers ownership of the context's allocated tables to this address space.
    ///
    /// safety:
    /// no other owner is allowed to destroy the context, and its tables must use this space's providers
    pub unsafe fn init_from_context(&self, context: AddressSpaceContext) -> Result<(), VmError> {
        let pointer = Box::into_raw(Box::new(context));
        match self.context.compare_exchange(
            core::ptr::null_mut(),
            pointer,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(_) => {
                unsafe { drop(Box::from_raw(pointer)) };
                Err(VmError::AlreadyInitialized)
            }
        }
    }

    fn context(&self) -> Result<AddressSpaceContext, VmError> {
        let pointer = self.context.load(Ordering::Acquire);
        if pointer.is_null() {
            return Err(VmError::NotInitialized);
        }
        Ok(unsafe { *pointer })
    }

    pub fn table_for(&self, address: usize) -> Result<PageTable, VmError> {
        Ok(self.context()?.table_for(address, &self.memory())?)
    }

    /// safety:
    /// this address space and its inherited kernel mappings must remain alive while active on any CPU.
    /// caller must switch away before dropping its final (owning) reference
    pub unsafe fn activate(&self) -> Result<(), VmError> {
        unsafe { self.context()?.activate() };
        Ok(())
    }

    fn memory(&self) -> MemoryProvider<'_> {
        MemoryProvider::new(&self.allocator, self.translator)
    }

    /// locks an owned address range while pinning its ancestors
    /// application cursors cannot mutate inherited kernel mappings
    ///
    /// # safety
    /// mutations through the cursor must preserve live rust references and executing code
    pub unsafe fn lock(&self, range: Range<usize>) -> Result<Cursor<'_>, VmError> {
        let size = range
            .end
            .checked_sub(range.start)
            .ok_or(VmError::InvalidSize)?;
        mapper::validate_range(range.start, size)?;
        if crate::vm::is_kernel_address(range.start) != self.kernel {
            return Err(VmError::InvalidAddress);
        }
        let mut table = self.table_for(range.start)?;
        let mut level = Level::ROOT;
        let mut reads = Vec::new();
        loop {
            if level == Level::LEAF {
                break;
            }
            let first = level.index(range.start);
            if first != level.index(range.end - 1) {
                break;
            }
            let physical = self.translator.dmap_to_phys(table.as_ptr());
            let guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.read();
            match unsafe { table.read(first, level) } {
                Entry::Table { physical_address } => {
                    reads.push(guard);
                    table = unsafe {
                        PageTable::from_ptr(self.translator.phys_to_dmap(physical_address))
                    };
                    level = level.child().unwrap();
                }
                _ => {
                    drop(guard);
                    break;
                }
            }
        }
        let physical = self.translator.dmap_to_phys(table.as_ptr());
        let guard = PAGE_DESCRIPTORS.get_page_descriptor(physical).lock.write();
        // don't need to validate again. the write locked node covers the requested range.
        // all of the ancestors are read locked, so another cursor can't detach it.
        Ok(Cursor::new(
            self,
            range,
            table,
            level,
            physical,
            reads,
            guard,
            self.translator,
        ))
    }
}

impl Drop for AddressSpace<'_> {
    fn drop(&mut self) {
        let pointer = self.context.load(Ordering::Acquire);
        if pointer.is_null() {
            return;
        }
        let context = unsafe { Box::from_raw(pointer) };
        // safety: owners cannot activate without being bound by the lifetime
        unsafe { context.destroy(&self.memory()) };
    }
}
