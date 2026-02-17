use klib::vm::TTable;

#[inline]
pub fn alloc_table<const N: usize>() -> Option<*mut TTable<N>> {
    Some(0usize as *mut TTable<N>)
}
