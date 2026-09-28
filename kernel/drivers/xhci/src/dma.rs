use core::{
    ops::{Deref, DerefMut},
    ptr::read_volatile,
    sync::atomic::{Ordering, compiler_fence},
};

use alloc::{vec, vec::Vec};
use klib::{
    allocator_support::KernelAddressTranslator, cache::clean_dcache_range,
    pm::page::mapper::AddressTranslator, vm::PAGE_SIZE,
};

/// Physically contiguous DMA memory with explicit cache synchronization.
pub struct DmaBuffer<T: ?Sized> {
    phys: u64,
    len: usize,
    byte_len: usize,
    _backing: Vec<u8>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy> DmaBuffer<[T]> {
    pub fn new_slice(len: usize, default: T) -> Result<Self, &'static str> {
        Self::new_slice_aligned(len, default, PAGE_SIZE)
    }

    pub fn new_slice_aligned(
        len: usize,
        default: T,
        alignment: usize,
    ) -> Result<Self, &'static str> {
        if len == 0 {
            return Err("DMA buffer must not be empty");
        }
        if alignment < PAGE_SIZE || !alignment.is_power_of_two() {
            return Err("DMA alignment must be a power of two of at least one page");
        }

        let byte_len = len
            .checked_mul(size_of::<T>())
            .ok_or("DMA buffer size overflow")?;
        let backing_len = byte_len
            .checked_add(alignment)
            .ok_or("DMA buffer allocation size overflow")?;
        let mut backing = vec![0u8; backing_len];
        let base = backing.as_mut_ptr();
        let base_phys = KernelAddressTranslator.dmap_to_phys(base) as usize;
        let aligned_phys = base_phys
            .checked_add(alignment - 1)
            .ok_or("DMA address overflow")?
            & !(alignment - 1);
        let offset = aligned_phys - base_phys;
        if offset
            .checked_add(byte_len)
            .is_none_or(|end| end > backing.len())
        {
            return Err("aligned DMA buffer exceeds its allocation");
        }

        let aligned_ptr = unsafe { base.add(offset) };
        let typed_slice = unsafe { core::slice::from_raw_parts_mut(aligned_ptr.cast::<T>(), len) };
        typed_slice.fill(default);

        Ok(Self {
            phys: aligned_phys as u64,
            len,
            byte_len,
            _backing: backing,
            _marker: core::marker::PhantomData,
        })
    }

    pub fn sync_for_device(&self) {
        compiler_fence(Ordering::Release);
        let addr = KernelAddressTranslator.phys_to_dmap(self.phys as usize);
        unsafe { clean_dcache_range(addr, self.byte_len) };
        compiler_fence(Ordering::SeqCst);
    }

    pub fn sync_for_cpu(&self) {
        let addr = KernelAddressTranslator.phys_to_dmap(self.phys as usize);
        unsafe { clean_dcache_range(addr, self.byte_len) };
        compiler_fence(Ordering::Acquire);
    }

    #[inline]
    pub fn read_volatile(&self, index: usize) -> T {
        assert!(index < self.len);
        let ptr = KernelAddressTranslator.phys_to_dmap(self.phys as usize) as *const T;
        unsafe { read_volatile(ptr.add(index)) }
    }

    #[inline]
    pub fn write_volatile(&mut self, index: usize, val: T) {
        assert!(index < self.len);
        let ptr = KernelAddressTranslator.phys_to_dmap(self.phys as usize) as *mut T;
        unsafe { core::ptr::write_volatile(ptr.add(index), val) }
    }
}

impl<T: Copy> Deref for DmaBuffer<[T]> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        let ptr = KernelAddressTranslator.phys_to_dmap(self.phys as usize) as *const T;
        unsafe { core::slice::from_raw_parts(ptr, self.len) }
    }
}

impl<T: Copy> DerefMut for DmaBuffer<[T]> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let ptr = KernelAddressTranslator.phys_to_dmap(self.phys as usize) as *mut T;
        unsafe { core::slice::from_raw_parts_mut(ptr, self.len) }
    }
}

impl<T: ?Sized> DmaBuffer<T> {
    pub fn phys_addr(&self) -> u64 {
        self.phys
    }
}
