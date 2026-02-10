use core::ptr::write_volatile;

use mars_klib::vm::{TTENATIVE, TTable};

pub const MAX_TABLES: usize = 512;

// not threadsafe. doesnt matter since this wont be used outside early boot
static mut NEXT_TABLE: usize = 0;

#[inline]
pub fn alloc_table<const N: usize>(base: *const [TTable<N>]) -> Option<*mut TTable<N>> {
    let i = unsafe { NEXT_TABLE.checked_add(1).unwrap_or(usize::MAX) };

    if i >= MAX_TABLES {
        return None;
    }

    debug_assert!(!base.is_empty());
    debug_assert!(i < MAX_TABLES);

    let ptr = unsafe { (base as *const () as usize).wrapping_add(i * size_of::<TTable<N>>()) };
    let table = &mut unsafe { *(ptr as *mut TTable<N>) };

    for j in 0..N {
        unsafe { write_volatile(&mut table.entries[j], TTENATIVE::invalid()) };
    }

    unsafe { NEXT_TABLE = i };

    unsafe { Some(ptr as *mut TTable<N>) }
}
