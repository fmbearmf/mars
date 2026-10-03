use core::arch::asm;

pub(crate) fn line_size() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << ((ctr >> 16) & 15)
}

unsafe fn maintain(address: *const u8, length: usize, operation: unsafe fn(usize)) {
    if length == 0 {
        return;
    }
    let last = (address as usize)
        .checked_add(length - 1)
        .expect("cache range overflow");
    let line = line_size();
    let mut current = address as usize & !(line - 1);
    loop {
        unsafe { operation(current) };
        if last - current < line {
            break;
        }
        current += line;
    }
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
}

unsafe fn clean(address: usize) {
    unsafe { asm!("dc cvac, {0}", in(reg) address, options(nostack, preserves_flags)) };
}
unsafe fn invalidate(address: usize) {
    unsafe { asm!("dc ivac, {0}", in(reg) address, options(nostack, preserves_flags)) };
}
unsafe fn clean_invalidate(address: usize) {
    unsafe { asm!("dc civac, {0}", in(reg) address, options(nostack, preserves_flags)) };
}
unsafe fn unify(address: usize) {
    unsafe { asm!("dc cvau, {0}", in(reg) address, options(nostack, preserves_flags)) };
}

pub(crate) unsafe fn invalidate_data_cache(address: *const u8, length: usize) {
    unsafe { maintain(address, length, invalidate) };
}
pub(crate) unsafe fn clean_data_cache(address: *const u8, length: usize) {
    unsafe { maintain(address, length, clean) };
}
pub(crate) unsafe fn clean_and_invalidate_data_cache(address: *const u8, length: usize) {
    unsafe { maintain(address, length, clean_invalidate) };
    unsafe { asm!("isb", options(nostack, preserves_flags)) };
}
pub(crate) unsafe fn clean_instruction_cache(address: *const u8, length: usize) {
    unsafe { maintain(address, length, unify) };
    unsafe {
        asm!(
            "ic ialluis",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        )
    };
}
