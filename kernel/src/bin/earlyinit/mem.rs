extern crate alloc;

use core::range::Range;

use crate::{KALLOCATOR, allocator::KernelAddressTranslator};
use alloc::boxed::Box;
use klib::{
    pm::page::mapper::AddressTranslator,
    sync::RwLock,
    vm::{
        PAGE_SIZE,
        user::{PageDescriptor, PtState},
    },
};

pub fn create_page_descriptors() -> (Box<[PageDescriptor]>, Range<usize>) {
    let alloc = KALLOCATOR.page_alloc();

    let min = KernelAddressTranslator::dmap_to_phys(alloc.min_address() as *mut usize) as usize;
    let max = KernelAddressTranslator::dmap_to_phys(alloc.max_address() as *mut usize) as usize;
    let size = max - min;
    let pages = size / PAGE_SIZE;

    let mut uninit = Box::<[PageDescriptor]>::new_uninit_slice(pages);

    for slot in uninit.iter_mut() {
        slot.write(PageDescriptor {
            lock: RwLock::new(PtState { meta: None }),
        });
    }

    (
        unsafe { uninit.assume_init() },
        Range {
            start: min,
            end: max,
        },
    )
}
