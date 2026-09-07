use core::{
    ops::{Deref, DerefMut},
    ptr::read_volatile,
};

use alloc::{vec, vec::Vec};
use klib::{
    allocator_support::KernelAddressTranslator, pm::page::mapper::AddressTranslator, vm::PAGE_SIZE,
};

/// cache-coherent page-aligned memory for DMA
pub struct DmaBuffer<T: ?Sized> {
    phys: u64,
    len: usize,
    backing: Vec<u8>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy> DmaBuffer<[T]> {
    pub fn new_slice(len: usize, default: T) -> Result<Self, &'static str> {
        let size = len * size_of::<T>();
        let mut backing = vec![0u8; size + PAGE_SIZE];
        let ptr = backing.as_mut_ptr();
        let align_offset = ptr.align_offset(PAGE_SIZE);

        let aligned_ptr = unsafe { ptr.add(align_offset) };
        let phys = KernelAddressTranslator.dmap_to_phys(aligned_ptr as _) as u64;

        let typed_slice = unsafe { core::slice::from_raw_parts_mut(aligned_ptr.cast::<T>(), len) };
        typed_slice.fill(default);

        Ok(Self {
            phys,
            len,
            backing,
            _marker: core::marker::PhantomData,
        })
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
