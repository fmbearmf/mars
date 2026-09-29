use core::{arch::asm, ptr::NonNull};

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
        if cache_line_size() > PAGE_SIZE {
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
        // prevent corrupting another allocation (partial cache lines)
        for address in (self.ptr.as_ptr() as usize..self.ptr.as_ptr() as usize + PAGE_SIZE)
            .step_by(cache_line_size())
        {
            unsafe {
                asm!("dc civac, {address}", address = in(reg) address, options(nostack, preserves_flags))
            };
        }
        barrier();
    }

    pub(crate) fn invalidate_for_cpu(&mut self) {
        for address in (self.ptr.as_ptr() as usize..self.ptr.as_ptr() as usize + PAGE_SIZE)
            .step_by(cache_line_size())
        {
            unsafe {
                asm!("dc ivac, {address}", address = in(reg) address, options(nostack, preserves_flags))
            };
        }
        barrier();
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

fn cache_line_size() -> usize {
    let ctr: u64;
    unsafe {
        asm!("mrs {ctr}, ctr_el0", ctr = out(reg) ctr, options(nomem, nostack, preserves_flags))
    };
    4 << ((ctr >> 16) & 0xf)
}

pub(crate) fn barrier() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}
