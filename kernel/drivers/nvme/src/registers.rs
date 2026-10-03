use core::ptr::NonNull;

use klib::vm::map_mmio;

use crate::dma::barrier;

pub(crate) struct Registers {
    base: NonNull<u8>,
    size: usize,
}

// mmio has no thread affinity. the controller owns and serializes register accesses
unsafe impl Send for Registers {}

impl Registers {
    /// safety:
    /// called during single-core device initialization for firmware-assigned device mmio
    pub(crate) unsafe fn map(physical: usize, size: usize) -> Result<Self, &'static str> {
        physical
            .checked_add(size)
            .filter(|end| *end <= (1usize << 48))
            .ok_or("MMIO range exceeds direct map")?;
        if physical == 0 || size == 0 || physical % 8 != 0 || size > isize::MAX as usize {
            return Err("invalid MMIO range");
        }
        let base = unsafe { map_mmio(physical, size) }.map_err(|_| "failed to map NVMe MMIO")?;
        barrier();
        Ok(Self {
            base: NonNull::new(base).ok_or("null MMIO")?,
            size,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.size
    }

    pub(crate) fn read32(&self, offset: usize) -> u32 {
        assert!(offset % 4 == 0 && offset.checked_add(4).is_some_and(|end| end <= self.size));
        let value = unsafe { self.base.as_ptr().add(offset).cast::<u32>().read_volatile() };
        barrier();
        u32::from_le(value)
    }

    pub(crate) fn write32(&mut self, offset: usize, value: u32) {
        assert!(offset % 4 == 0 && offset.checked_add(4).is_some_and(|end| end <= self.size));
        barrier();
        unsafe {
            self.base
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .write_volatile(value.to_le())
        };
        barrier();
    }

    pub(crate) fn read64(&self, offset: usize) -> u64 {
        assert!(offset % 8 == 0 && offset.checked_add(8).is_some_and(|end| end <= self.size));
        let value = unsafe { self.base.as_ptr().add(offset).cast::<u64>().read_volatile() };
        barrier();
        u64::from_le(value)
    }

    pub(crate) fn write64(&mut self, offset: usize, value: u64) {
        assert!(offset % 8 == 0 && offset.checked_add(8).is_some_and(|end| end <= self.size));
        barrier();
        unsafe {
            self.base
                .as_ptr()
                .add(offset)
                .cast::<u64>()
                .write_volatile(value.to_le())
        };
        barrier();
    }
}
