use core::{
    slice::from_raw_parts_mut,
    sync::atomic::{AtomicUsize, Ordering},
    usize::MAX,
};

use mars_kernel::vm::{TABLE_ENTRIES, TTable};

use crate::busy_loop;

pub const MAX_TABLES: usize = 64;

static NEXT_TABLE: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" {
    static __pt_pool_start: u8;
    static __pt_pool_end: u8;
}

#[inline]
pub fn alloc_table(base: &mut [TTable<TABLE_ENTRIES>]) -> Option<&mut TTable<TABLE_ENTRIES>> {
    let i = NEXT_TABLE.fetch_add(1, Ordering::Relaxed);

    if i >= MAX_TABLES {
        return None;
    }

    unsafe { Some(&mut base[i]) }
}
