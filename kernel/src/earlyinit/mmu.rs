pub fn init_cpu() {
    // run before registering cpu-local data or enabling interrupts
    unsafe { hal::boot::initialize_processor() }
}
