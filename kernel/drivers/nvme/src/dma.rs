use core::ptr::NonNull;
use hal::{
    cache::{clean_and_invalidate_data_cache, invalidate_data_cache},
    memory::{acquire_from_device, publish_to_device},
};

use klib::{
    allocator_support::KernelAddressTranslator,
    pm::page::mapper::AddressTranslator,
    vm::{KPAGE_ALLOCATOR, PAGE_SIZE},
};

/// one owned physical page. don't take any references (UB)
pub(crate) struct DmaPage {
    ptr: NonNull<u8>,
    bus: u64,
    pinned: bool,
}

// controller access is serialized by the owner
unsafe impl Send for DmaPage {}

impl DmaPage {
    pub(crate) fn allocate(address_bits: u8) -> Result<Self, &'static str> {
        if !(1..=64).contains(&address_bits) {
            return Err("invalid DMA address width");
        }
        if hal::cache::data_line_size() > PAGE_SIZE {
            return Err("DMA cache line exceeds allocation granule");
        }
        let ptr = NonNull::new(KPAGE_ALLOCATOR.alloc_pages(0)).ok_or("DMA allocation failed")?;
        let phys = KernelAddressTranslator.dmap_to_phys(ptr.as_ptr()) as u64;
        let limit = u64::MAX >> (64 - address_bits);
        if phys & (PAGE_SIZE as u64 - 1) != 0
            || phys
                .checked_add(PAGE_SIZE as u64 - 1)
                .is_none_or(|end| end > limit)
        {
            KPAGE_ALLOCATOR.free_pages(ptr.as_ptr());
            return Err("DMA allocation exceeds PCI addressability");
        }
        unsafe { ptr.as_ptr().write_bytes(0, PAGE_SIZE) };
        Ok(Self {
            ptr,
            bus: phys,
            pinned: false,
        })
    }

    pub(crate) fn phys(&self) -> u64 {
        self.bus
    }

    pub(crate) fn pin(&mut self) {
        self.pinned = true;
    }

    /// controller must have stopped DMA to this allocation, including any pending commands
    pub(crate) unsafe fn unpin_after_dma_stopped(&mut self) {
        self.pinned = false;
    }

    pub(crate) fn prepare_for_device(&mut self) {
        unsafe { clean_and_invalidate_data_cache(self.ptr.as_ptr(), PAGE_SIZE) };
        publish_to_device();
    }

    pub(crate) fn invalidate_for_cpu(&mut self) {
        unsafe { invalidate_data_cache(self.ptr.as_ptr(), PAGE_SIZE) };
        acquire_from_device();
    }

    pub(crate) fn read_u32(&self, offset: usize) -> u32 {
        assert!(offset % 4 == 0 && offset <= PAGE_SIZE - 4);
        unsafe { u32::from_le(self.ptr.as_ptr().add(offset).cast::<u32>().read_volatile()) }
    }

    pub(crate) fn write_u32(&mut self, offset: usize, value: u32) {
        assert!(offset % 4 == 0 && offset <= PAGE_SIZE - 4);
        unsafe {
            self.ptr
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .write_volatile(value.to_le())
        };
    }

    pub(crate) fn copy_from(&mut self, bytes: &[u8]) {
        assert!(bytes.len() <= PAGE_SIZE);
        for (offset, byte) in bytes.iter().copied().enumerate() {
            unsafe { self.ptr.as_ptr().add(offset).write_volatile(byte) };
        }
    }

    pub(crate) fn copy_to(&self, bytes: &mut [u8]) {
        assert!(bytes.len() <= PAGE_SIZE);
        for (offset, byte) in bytes.iter_mut().enumerate() {
            *byte = unsafe { self.ptr.as_ptr().add(offset).read_volatile() };
        }
    }
}

impl Drop for DmaPage {
    fn drop(&mut self) {
        if self.pinned {
            // failed resets don't guarantee that DMA is done
            log::error!("nvme: quarantine DMA page at {:#x}", self.bus);
        } else {
            KPAGE_ALLOCATOR.free_pages(self.ptr.as_ptr());
        }
    }
}

pub(crate) fn barrier() {
    publish_to_device();
}
