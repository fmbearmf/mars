pub use clean_and_invalidate_data_cache as clean_dcache_range;
pub use clean_instruction_cache as clean_icache_range;

/// return the data-cache maintenance granule in bytes
pub fn data_line_size() -> usize {
    crate::arch::cache::line_size()
}

/// publish data writes to the point of coherency
///
/// safety:
/// map the range for this cpu and synchronize overlapping writes
pub unsafe fn clean_data_cache(address: *const u8, length: usize) {
    unsafe { crate::arch::clean_data_cache(address, length) }
}

/// clean and invalidate data lines before returning
///
/// safety:
/// map the range and synchronize cpu and device writes to every overlapping cache line
pub unsafe fn clean_and_invalidate_data_cache(address: *const u8, length: usize) {
    unsafe { crate::arch::clean_and_invalidate_data_cache(address, length) }
}

/// invalidate data lines after a device writes the range
///
/// safety:
/// map the range and exclude dirty cpu writes to every overlapping cache line
pub unsafe fn invalidate_data_cache(address: *const u8, length: usize) {
    unsafe { crate::arch::invalidate_data_cache(address, length) }
}

/// publish modified instructions to processors sharing the address space
///
/// safety:
/// map the range and exclude concurrent code modification and execution of changed instructions
pub unsafe fn clean_instruction_cache(address: *const u8, length: usize) {
    unsafe { crate::arch::clean_instruction_cache(address, length) }
}
