pub(crate) unsafe fn debug_exit(status: usize) -> ! {
    let block = [0x20026usize, status];
    unsafe {
        core::arch::asm!("hlt #0xf000", in("x0") 0x20usize, in("x1") block.as_ptr(), options(nostack))
    };
    loop {
        core::hint::spin_loop();
    }
}
