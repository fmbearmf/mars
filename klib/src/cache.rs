use aarch64_cpu_ext::asm::barrier::{self, dsb, isb};

use crate::vm::{align_down, align_up};

fn get_dcache_line_size() -> usize {
    let ctr: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, ctr_el0",
            out(reg) ctr,
            options(nomem, nostack, preserves_flags)
        );
    }

    // log2 of size
    let dmin_line = (ctr >> 16) & 0xF;

    // 4 bytes * 2^dmin_line
    1 << (dmin_line + 2)
}

pub unsafe fn clean_dcache_range(addr: *const u8, len: usize) {
    let cache_line_size = get_dcache_line_size();

    let start = align_down(addr as usize, cache_line_size);
    let end = align_up(addr as usize + len, cache_line_size);

    for ptr in (start..end).step_by(cache_line_size) {
        unsafe {
            core::arch::asm!(
                "dc cvac, {}",
                in(reg) ptr,
                options(nomem, nostack, preserves_flags)
            );
        }
    }

    dsb(barrier::SY);
    isb(barrier::SY);
}
